// SPDX-License-Identifier: GPL-3.0-or-later
//! The memory routines that LLVM calls by name.
//!
//! LLVM expands a memory intrinsic inline only up to a target-chosen size and
//! lowers the rest to calls to `memcpy`, `memmove`, and `memset`, even in
//! freestanding code. Nia links no C library, so the compiler defines these
//! three in every native image, like its other compiler builtins. Darwin
//! targets add the routine LLVM calls there to zero memory instead of
//! `memset`; see [`zero_fill_symbol`]. They are ordinary definitions: a
//! library linked into the same image that defines them as well collides at
//! link time, as any other duplicate symbol does.
//!
//! The bodies are byte loops. The module pipeline vectorizes them, and the
//! `no-builtins` attribute it puts on every function keeps it from recognizing
//! a loop as a copy and calling the routine from inside itself.

use nia_diagnostic::Diagnostic;
use nia_llvm::{
    AddressSpace, Context, IntPredicate,
    basic_block::BasicBlock,
    builder::Builder,
    module::{Linkage, Module},
    types::IntType,
    values::{BasicValue, FunctionValue, IntValue, PointerValue},
};

use super::{diagnostic_from_llvm_error, hide_builtin};

/// Symbols that [`emit_memory_routines`] defines on every target.
pub(super) const MEMORY_ROUTINES: [&str; 3] = ["memcpy", "memmove", "memset"];

/// The routine that LLVM's code generator calls to zero memory on `target`
/// when it is not `memset`.
///
/// LLVM lowers a `memset` of zero to `bzero` on AArch64 Darwin and to
/// `__bzero` on x86-64 macOS from 10.6 on (its `RuntimeLibcalls` rules); no
/// function attribute turns this off. Every maintained macOS release is newer
/// than 10.6.
pub(super) fn zero_fill_symbol(target: nia_target::TargetConfig) -> Option<&'static str> {
    match (target.os(), target.arch()) {
        (nia_target::Os::Macos, nia_target::Arch::Aarch64) => Some("bzero"),
        (nia_target::Os::Macos, nia_target::Arch::X86_64) => Some("__bzero"),
        _ => None,
    }
}

pub(super) fn emit_memory_routines<'ctx>(
    context: &'ctx Context,
    module: &Module<'ctx>,
    pointer_bits: u32,
    zero_fill: Option<&str>,
) -> Result<(), Diagnostic> {
    let routines = MemoryRoutines::new(context, module, pointer_bits)?;
    routines.emit_memcpy()?;
    routines.emit_memmove()?;
    routines.emit_memset()?;
    if let Some(name) = zero_fill {
        routines.emit_zero_fill(name)?;
    }
    Ok(())
}

struct MemoryRoutines<'m, 'ctx> {
    context: &'ctx Context,
    module: &'m Module<'ctx>,
    size: IntType<'ctx>,
}

// One routine's parameters and the blocks its loops branch between.
struct Routine<'ctx> {
    function: FunctionValue<'ctx>,
    builder: Builder<'ctx>,
    dest: PointerValue<'ctx>,
    len: IntValue<'ctx>,
    returns_dest: bool,
}

impl<'m, 'ctx> MemoryRoutines<'m, 'ctx> {
    fn new(
        context: &'ctx Context,
        module: &'m Module<'ctx>,
        pointer_bits: u32,
    ) -> Result<Self, Diagnostic> {
        Ok(Self {
            context,
            module,
            size: context
                .custom_width_int_type(pointer_bits)
                .map_err(diagnostic_from_llvm_error)?,
        })
    }

    // `void *memcpy(void *dest, const void *src, size_t n)`
    fn emit_memcpy(&self) -> Result<(), Diagnostic> {
        let routine = self.declare("memcpy", Shape::Copy)?;
        let src = routine.pointer_param(1)?;
        let done = self.block(&routine, "done")?;
        self.copy_loop(&routine, src, Direction::Forward, done)?;
        routine.builder.position_at_end(done);
        routine.finish()
    }

    // `void *memmove(void *dest, const void *src, size_t n)`
    //
    // Copying forward is safe unless the destination starts inside the source.
    // `dest - src` wraps when the destination comes first, so one unsigned
    // comparison with `n` decides the direction.
    fn emit_memmove(&self) -> Result<(), Diagnostic> {
        let routine = self.declare("memmove", Shape::Copy)?;
        let src = routine.pointer_param(1)?;
        let forward = self.block(&routine, "forward")?;
        let backward = self.block(&routine, "backward")?;
        let done = self.block(&routine, "done")?;
        let builder = &routine.builder;
        let dest_address = builder
            .build_ptr_to_int(routine.dest, self.size, "dest.address")
            .map_err(diagnostic_from_llvm_error)?;
        let src_address = builder
            .build_ptr_to_int(src, self.size, "src.address")
            .map_err(diagnostic_from_llvm_error)?;
        let distance = builder
            .build_int_sub(dest_address, src_address, "distance")
            .map_err(diagnostic_from_llvm_error)?;
        let no_overlap = builder
            .build_int_compare(IntPredicate::UGE, distance, routine.len, "no.overlap")
            .map_err(diagnostic_from_llvm_error)?;
        builder
            .build_conditional_branch(no_overlap, forward, backward)
            .map_err(diagnostic_from_llvm_error)?;
        builder.position_at_end(forward);
        self.copy_loop(&routine, src, Direction::Forward, done)?;
        routine.builder.position_at_end(backward);
        self.copy_loop(&routine, src, Direction::Backward, done)?;
        routine.builder.position_at_end(done);
        routine.finish()
    }

    // `void *memset(void *s, int c, size_t n)`; only the low byte of `c` is
    // stored.
    fn emit_memset(&self) -> Result<(), Diagnostic> {
        let routine = self.declare("memset", Shape::Fill)?;
        let byte = routine
            .function
            .get_nth_param(1)
            .ok_or_else(|| missing_param("memset"))?
            .map_err(diagnostic_from_llvm_error)?
            .into_int_value()
            .map_err(diagnostic_from_llvm_error)?;
        let byte = routine
            .builder
            .build_int_truncate(byte, self.context.i8_type(), "byte")
            .map_err(diagnostic_from_llvm_error)?;
        let done = self.block(&routine, "done")?;
        self.byte_loop(&routine, Direction::Forward, done, |builder, index| {
            let dest = self.byte_at(builder, routine.dest, index, "dest.byte")?;
            builder
                .build_store(dest, byte)
                .map_err(diagnostic_from_llvm_error)?;
            Ok(())
        })?;
        routine.builder.position_at_end(done);
        routine.finish()
    }

    // `void bzero(void *s, size_t n)`, under the name the target calls it.
    fn emit_zero_fill(&self, name: &str) -> Result<(), Diagnostic> {
        let routine = self.declare(name, Shape::ZeroFill)?;
        let zero = self
            .context
            .i8_type()
            .const_int(0, false)
            .map_err(diagnostic_from_llvm_error)?;
        let done = self.block(&routine, "done")?;
        self.byte_loop(&routine, Direction::Forward, done, |builder, index| {
            let dest = self.byte_at(builder, routine.dest, index, "dest.byte")?;
            builder
                .build_store(dest, zero)
                .map_err(diagnostic_from_llvm_error)?;
            Ok(())
        })?;
        routine.builder.position_at_end(done);
        routine.finish()
    }

    // Declares a routine of `shape` and positions a builder at its entry
    // block.
    fn declare(&self, name: &str, shape: Shape) -> Result<Routine<'ctx>, Diagnostic> {
        let pointer = self.context.ptr_type(AddressSpace(0));
        let fn_ty = match shape {
            Shape::Copy => {
                pointer.fn_type(&[pointer.into(), pointer.into(), self.size.into()], false)
            }
            Shape::Fill => pointer.fn_type(
                &[
                    pointer.into(),
                    self.context.i32_type().into(),
                    self.size.into(),
                ],
                false,
            ),
            Shape::ZeroFill => self
                .context
                .void_type()
                .fn_type(&[pointer.into(), self.size.into()], false),
        }
        .map_err(diagnostic_from_llvm_error)?;
        let function = self
            .module
            .add_function(name, fn_ty, Some(Linkage::External))
            .map_err(diagnostic_from_llvm_error)?;
        hide_builtin(function);
        let builder = self
            .context
            .create_builder()
            .map_err(diagnostic_from_llvm_error)?;
        let entry = self
            .context
            .append_basic_block(function, "entry")
            .map_err(diagnostic_from_llvm_error)?;
        builder.position_at_end(entry);
        let dest = function
            .get_nth_param(0)
            .ok_or_else(|| missing_param(name))?
            .map_err(diagnostic_from_llvm_error)?
            .into_pointer_value()
            .map_err(diagnostic_from_llvm_error)?;
        let len_index = match shape {
            Shape::Copy | Shape::Fill => 2,
            Shape::ZeroFill => 1,
        };
        let len = function
            .get_nth_param(len_index)
            .ok_or_else(|| missing_param(name))?
            .map_err(diagnostic_from_llvm_error)?
            .into_int_value()
            .map_err(diagnostic_from_llvm_error)?;
        Ok(Routine {
            function,
            builder,
            dest,
            len,
            returns_dest: !matches!(shape, Shape::ZeroFill),
        })
    }

    fn block(&self, routine: &Routine<'ctx>, name: &str) -> Result<BasicBlock<'ctx>, Diagnostic> {
        self.context
            .append_basic_block(routine.function, name)
            .map_err(diagnostic_from_llvm_error)
    }

    fn copy_loop(
        &self,
        routine: &Routine<'ctx>,
        src: PointerValue<'ctx>,
        direction: Direction,
        exit: BasicBlock<'ctx>,
    ) -> Result<(), Diagnostic> {
        self.byte_loop(routine, direction, exit, |builder, index| {
            let from = self.byte_at(builder, src, index, "src.byte")?;
            let to = self.byte_at(builder, routine.dest, index, "dest.byte")?;
            let byte = builder
                .build_load(self.context.i8_type(), from, "byte")
                .map_err(diagnostic_from_llvm_error)?;
            builder
                .build_store(to, byte)
                .map_err(diagnostic_from_llvm_error)?;
            Ok(())
        })
    }

    // Visits every index below `len` once, ascending or descending, then
    // branches to `exit`. The builder must be positioned where the loop
    // starts.
    fn byte_loop(
        &self,
        routine: &Routine<'ctx>,
        direction: Direction,
        exit: BasicBlock<'ctx>,
        visit: impl Fn(&Builder<'ctx>, IntValue<'ctx>) -> Result<(), Diagnostic>,
    ) -> Result<(), Diagnostic> {
        let builder = &routine.builder;
        let preheader = builder
            .get_insert_block()
            .ok_or_else(|| missing_param("memory routine block"))?;
        let check = self.block(routine, "check")?;
        let body = self.block(routine, "body")?;
        let zero = self
            .size
            .const_int(0, false)
            .map_err(diagnostic_from_llvm_error)?;
        let one = self
            .size
            .const_int(1, false)
            .map_err(diagnostic_from_llvm_error)?;
        builder
            .build_unconditional_branch(check)
            .map_err(diagnostic_from_llvm_error)?;

        // Forward: index runs 0..len. Backward: remaining runs len..0 and the
        // body visits remaining - 1.
        builder.position_at_end(check);
        let cursor = builder
            .build_phi(self.size, "cursor")
            .map_err(diagnostic_from_llvm_error)?;
        let cursor_value = cursor
            .as_basic_value()
            .and_then(|value| value.into_int_value())
            .map_err(diagnostic_from_llvm_error)?;
        let finished = builder
            .build_int_compare(
                IntPredicate::EQ,
                cursor_value,
                match direction {
                    Direction::Forward => routine.len,
                    Direction::Backward => zero,
                },
                "finished",
            )
            .map_err(diagnostic_from_llvm_error)?;
        builder
            .build_conditional_branch(finished, exit, body)
            .map_err(diagnostic_from_llvm_error)?;

        builder.position_at_end(body);
        let (index, next) = match direction {
            Direction::Forward => {
                let next = builder
                    .build_int_add(cursor_value, one, "next")
                    .map_err(diagnostic_from_llvm_error)?;
                (cursor_value, next)
            }
            Direction::Backward => {
                let previous = builder
                    .build_int_sub(cursor_value, one, "previous")
                    .map_err(diagnostic_from_llvm_error)?;
                (previous, previous)
            }
        };
        visit(builder, index)?;
        let latch = builder
            .get_insert_block()
            .ok_or_else(|| missing_param("memory routine block"))?;
        builder
            .build_unconditional_branch(check)
            .map_err(diagnostic_from_llvm_error)?;
        let start = match direction {
            Direction::Forward => zero,
            Direction::Backward => routine.len,
        };
        cursor
            .add_incoming(&[(&start, preheader), (&next, latch)])
            .map_err(diagnostic_from_llvm_error)
    }

    fn byte_at(
        &self,
        builder: &Builder<'ctx>,
        base: PointerValue<'ctx>,
        index: IntValue<'ctx>,
        name: &str,
    ) -> Result<PointerValue<'ctx>, Diagnostic> {
        // SAFETY: an i8 GEP with one index is valid for any pointer and index.
        unsafe { builder.build_gep(self.context.i8_type(), base, &[index], name) }
            .map_err(diagnostic_from_llvm_error)
    }
}

impl<'ctx> Routine<'ctx> {
    fn pointer_param(&self, index: u32) -> Result<PointerValue<'ctx>, Diagnostic> {
        self.function
            .get_nth_param(index)
            .ok_or_else(|| missing_param("memory routine"))?
            .map_err(diagnostic_from_llvm_error)?
            .into_pointer_value()
            .map_err(diagnostic_from_llvm_error)
    }

    // The C routines return their destination; `bzero` returns nothing.
    fn finish(&self) -> Result<(), Diagnostic> {
        let dest: &dyn BasicValue<'ctx> = &self.dest;
        self.builder
            .build_return(self.returns_dest.then_some(dest))
            .map(|_| ())
            .map_err(diagnostic_from_llvm_error)
    }
}

/// The C signature of a memory routine.
#[derive(Clone, Copy)]
enum Shape {
    /// `void *(void *dest, const void *src, size_t n)`
    Copy,
    /// `void *(void *dest, int c, size_t n)`
    Fill,
    /// `void (void *dest, size_t n)`
    ZeroFill,
}

#[derive(Clone, Copy)]
enum Direction {
    Forward,
    Backward,
}

fn missing_param(routine: &str) -> Diagnostic {
    diagnostic_from_llvm_error(nia_llvm::LlvmError::ice(format!(
        "{routine} has no expected parameter"
    )))
}
