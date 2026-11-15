// SPDX-License-Identifier: GPL-3.0-or-later
//! The IR pass pipeline that runs before an object or LTO bitcode is written.
//!
//! Every Nia module goes through one pipeline builder in the C++ bridge. Its
//! output differs only by phase: a native object, ThinLTO pre-link bitcode, or
//! full-LTO pre-link bitcode. Modules are always optimized as freestanding code:
//! Nia never relies on a hosted C library, so no pass may assume that library
//! functions exist or introduce calls to them.

use std::ptr::NonNull;

use llvm_sys::prelude::LLVMModuleRef;
use llvm_sys::target_machine::LLVMTargetMachineRef;

use super::lto::{copy_bytes, copy_text};
use super::{LlvmError, LlvmResult, Module, OptimizationLevel, TargetMachine};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// LLVM's per-module optimization pipeline, from unoptimized to size-focused.
pub enum PipelineLevel {
    /// Only stack-slot promotion.
    O0,
    /// Cheap, local optimization.
    O1,
    /// LLVM's default optimization.
    O2,
    /// Optimization that spends more compile time and code size on speed.
    O3,
    /// The default pipeline with every function marked `optsize`.
    Os,
    /// The default pipeline with every function marked `optsize` and `minsize`.
    Oz,
}

impl PipelineLevel {
    pub(super) fn tag(self) -> u32 {
        match self {
            Self::O0 => 0,
            Self::O1 => 1,
            Self::O2 => 2,
            Self::O3 => 3,
            Self::Os => 4,
            Self::Oz => 5,
        }
    }

    /// The code-generation tier that turns this pipeline's output into machine
    /// code. `Os` keeps the default tier and `Oz` the lighter one; their size
    /// goals reach code generation through function attributes.
    pub fn codegen_level(self) -> OptimizationLevel {
        match self {
            Self::O0 => OptimizationLevel::None,
            Self::O1 | Self::Oz => OptimizationLevel::Less,
            Self::O2 | Self::Os => OptimizationLevel::Default,
            Self::O3 => OptimizationLevel::Aggressive,
        }
    }

    /// The level's conventional name, for reports.
    pub fn name(self) -> &'static str {
        match self {
            Self::O0 => "O0",
            Self::O1 => "O1",
            Self::O2 => "O2",
            Self::O3 => "O3",
            Self::Os => "Os",
            Self::Oz => "Oz",
        }
    }

    /// Stable identity of the pipeline at this level, for products that cache
    /// optimized output. It names how the pipeline is assembled; the LLVM
    /// version that implements it is identified separately. `nia-checks-N`
    /// names the revision of Nia's own passes (llvm_nia_passes.cpp), which
    /// any change to what they decide must bump.
    pub fn identity(self) -> &'static str {
        match self {
            Self::O0 => "freestanding:mem2reg+o0-default",
            Self::O1 => "freestanding:pass-builder-default:O1+nia-checks-1",
            Self::O2 => "freestanding:pass-builder-default:O2+nia-checks-1",
            Self::O3 => "freestanding:pass-builder-default:O3+nia-checks-1",
            Self::Os => "freestanding:pass-builder-default:O2+optsize+nia-checks-1",
            Self::Oz => "freestanding:pass-builder-default:O2+optsize+minsize+nia-checks-1",
        }
    }
}

enum FfiOwnedBuffer {}

unsafe extern "C" {
    fn nia_llvm_optimize_module(
        module: LLVMModuleRef,
        target: LLVMTargetMachineRef,
        level: u32,
    ) -> *mut FfiOwnedBuffer;
    fn nia_llvm_emit_thin_lto_bitcode(
        module: LLVMModuleRef,
        target: LLVMTargetMachineRef,
        level: u32,
    ) -> *mut FfiOwnedBuffer;
    fn nia_llvm_emit_full_lto_bitcode(
        module: LLVMModuleRef,
        target: LLVMTargetMachineRef,
        level: u32,
    ) -> *mut FfiOwnedBuffer;
    fn nia_llvm_owned_buffer_data(buffer: *const FfiOwnedBuffer) -> *const u8;
    fn nia_llvm_owned_buffer_len(buffer: *const FfiOwnedBuffer) -> usize;
    fn nia_llvm_owned_buffer_error(buffer: *const FfiOwnedBuffer) -> *const u8;
    fn nia_llvm_owned_buffer_error_len(buffer: *const FfiOwnedBuffer) -> usize;
    fn nia_llvm_owned_buffer_free(buffer: *mut FfiOwnedBuffer);
}

struct OwnedBufferHandle(NonNull<FfiOwnedBuffer>);

impl Drop for OwnedBufferHandle {
    fn drop(&mut self) {
        unsafe { nia_llvm_owned_buffer_free(self.0.as_ptr()) };
    }
}

// Takes ownership of a bridge result and returns its bytes or its error.
fn finish(result: *mut FfiOwnedBuffer, label: &str) -> LlvmResult<Vec<u8>> {
    let result = OwnedBufferHandle(
        NonNull::new(result)
            .ok_or_else(|| LlvmError::error(format!("LLVM returned no {label} result")))?,
    );
    let error = unsafe {
        copy_text(
            nia_llvm_owned_buffer_error(result.0.as_ptr()),
            nia_llvm_owned_buffer_error_len(result.0.as_ptr()),
            label,
        )?
    };
    if !error.is_empty() {
        return Err(LlvmError::error(error));
    }
    unsafe {
        copy_bytes(
            nia_llvm_owned_buffer_data(result.0.as_ptr()),
            nia_llvm_owned_buffer_len(result.0.as_ptr()),
            label,
        )
    }
}

/// Optimizes a module in place before native object emission.
pub fn optimize_module(
    module: &Module<'_>,
    target: &TargetMachine,
    level: PipelineLevel,
) -> LlvmResult<()> {
    let result = unsafe { nia_llvm_optimize_module(module.raw, target.raw, level.tag()) };
    finish(result, "module optimization").map(|_| ())
}

/// Runs the ThinLTO pre-link pipeline and emits summary-bearing bitcode.
pub fn emit_thin_lto_bitcode(
    module: &Module<'_>,
    target: &TargetMachine,
    level: PipelineLevel,
) -> LlvmResult<Vec<u8>> {
    let result = unsafe { nia_llvm_emit_thin_lto_bitcode(module.raw, target.raw, level.tag()) };
    finish(result, "ThinLTO pre-link")
}

/// Runs the full-LTO pre-link pipeline and emits regular bitcode.
pub fn emit_full_lto_bitcode(
    module: &Module<'_>,
    target: &TargetMachine,
    level: PipelineLevel,
) -> LlvmResult<Vec<u8>> {
    let result = unsafe { nia_llvm_emit_full_lto_bitcode(module.raw, target.raw, level.tag()) };
    finish(result, "full-LTO pre-link")
}

#[cfg(test)]
mod tests {
    use super::super::{
        Context, OptimizationLevel, TargetMachine, parse_test_ir, test_host_identity,
    };
    use super::{PipelineLevel, optimize_module};

    fn optimized(ir: &str) -> String {
        let context = Context::create().expect("create LLVM context");
        let target = TargetMachine::for_identity(&test_host_identity(), OptimizationLevel::Default)
            .expect("create host target machine");
        let module = parse_test_ir(&context, "pipeline-test", ir, &target);
        optimize_module(&module, &target, PipelineLevel::O2).expect("optimize module");
        module.ir_string().expect("print module")
    }

    // A loop that writes an element per iteration into a room it grows when
    // full: the capacity check's rare path grows the room and checks again,
    // and the write checks its index against the room of either path, which
    // the loop carries to the next iteration.
    const GROW_THEN_WRITE: &str = r#"
declare void @llvm.trap()
declare i64 @grow(i64)

define i64 @fill(ptr %p, i64 %room0, i64 %n) {
entry:
  %any = icmp ne i64 %n, 0
  br i1 %any, label %loop, label %done

loop:
  %i = phi i64 [ 0, %entry ], [ %i.next, %write ]
  %room = phi i64 [ %room0, %entry ], [ %len, %write ]
  %fits = icmp ult i64 %i, %room
  br i1 %fits, label %store, label %grow

grow:
  %grown = call i64 @grow(i64 %i)
  %fits.grown = icmp ult i64 %i, %grown
  br i1 %fits.grown, label %store, label %full

store:
  %len = phi i64 [ %room, %loop ], [ %grown, %grow ]
  %in.bounds = icmp ult i64 %i, %len
  br i1 %in.bounds, label %write, label %trap

write:
  %slot = getelementptr i8, ptr %p, i64 %i
  store i8 1, ptr %slot
  %i.next = add nuw i64 %i, 1
  %more = icmp ult i64 %i.next, %n
  br i1 %more, label %loop, label %done

done:
  %count = phi i64 [ 0, %entry ], [ %i.next, %write ]
  ret i64 %count

trap:
  call void @llvm.trap()
  unreachable

full:
  ret i64 0
}
"#;

    /// A comparison that every path into a merge has proven for the value it
    /// brings is decided, although no single branch dominates it.
    #[test]
    fn decides_comparisons_every_incoming_path_proves() {
        let ir = optimized(GROW_THEN_WRITE);
        assert!(!ir.contains("@llvm.trap()\n"), "{ir}");
        assert!(!ir.contains("call void @llvm.trap"), "{ir}");
    }

    /// When one path has not proven the comparison it stays, whatever the
    /// others prove.
    #[test]
    fn keeps_comparisons_a_path_leaves_open() {
        let unproven = GROW_THEN_WRITE.replace(
            "br i1 %fits.grown, label %store, label %full",
            "br label %store",
        );
        let ir = optimized(&unproven);
        assert!(ir.contains("call void @llvm.trap"), "{ir}");
    }

    // A loop whose iterations either leave the next a position they have
    // checked, and say so in a flag, or one they have not. The next reads
    // at the position only when the flag says it was checked.
    const FLAGGED_POSITION: &str = r#"
declare void @llvm.trap()
declare i1 @more(i64)

define void @walk(ptr %p, i64 %n, i64 %start) {
entry:
  %ok = icmp ult i64 %start, %n
  br i1 %ok, label %head, label %out

head:
  %pos = phi i64 [ %start, %entry ], [ %next, %open ], [ %next, %close ]
  %checked = phi i1 [ true, %entry ], [ true, %open ], [ false, %close ]
  br i1 %checked, label %read, label %step

read:
  %in = icmp ult i64 %pos, %n
  br i1 %in, label %load, label %trap

load:
  %slot = getelementptr i8, ptr %p, i64 %pos
  store i8 0, ptr %slot
  br label %step

step:
  %next = add nuw i64 %pos, 1
  %go = call i1 @more(i64 %next)
  br i1 %go, label %choose, label %out

choose:
  %check = call i1 @more(i64 %pos)
  br i1 %check, label %bound, label %close

bound:
  %fits = icmp ult i64 %next, %n
  br i1 %fits, label %open, label %out

open:
  br label %head

close:
  br label %head

out:
  ret void

trap:
  call void @llvm.trap()
  unreachable
}
"#;

    /// An edge on which the path to a comparison cannot be taken does not
    /// need to prove it.
    #[test]
    fn decides_comparisons_on_the_edges_that_reach_them() {
        let ir = optimized(FLAGGED_POSITION);
        assert!(!ir.contains("call void @llvm.trap"), "{ir}");
    }

    /// When the iterations that leave a checked position do not say so, the
    /// read checks it.
    #[test]
    fn keeps_comparisons_an_unflagged_edge_reaches() {
        let unflagged = FLAGGED_POSITION.replace("[ false, %close ]", "[ true, %close ]");
        let ir = optimized(&unflagged);
        assert!(ir.contains("call void @llvm.trap"), "{ir}");
    }

    // A token of at most fifteen bytes after a position, read when at least
    // seventeen bytes remain from it: the end past the token and its closing
    // byte, `q + len + 2`, is computed by two checked additions.
    const ROOM_THEN_OFFSET: &str = r#"
declare void @llvm.trap()

define i64 @next(i64 %n, i64 %q, i64 %mask) {
entry:
  %inside = icmp ule i64 %q, %n
  br i1 %inside, label %sized, label %none

sized:
  %rest = sub nuw i64 %n, %q
  %short = icmp ult i64 %rest, ROOM
  br i1 %short, label %none, label %token

token:
  %len = and i64 %mask, 15
  %len.not = xor i64 %len, -1
  %wraps = icmp ugt i64 %q, %len.not
  br i1 %wraps, label %trap, label %first

first:
  %end = add nuw i64 %q, %len
  %wraps.2 = icmp ugt i64 %end, -3
  br i1 %wraps.2, label %trap, label %second

second:
  %next = add nuw i64 %end, 2
  ret i64 %next

none:
  ret i64 0

trap:
  call void @llvm.trap()
  unreachable
}
"#;

    /// An addition to a position cannot overflow when the room past the
    /// position is at least what is added.
    #[test]
    fn decides_sums_bounded_by_the_room_after_a_base() {
        let ir = optimized(&ROOM_THEN_OFFSET.replace("ROOM", "17"));
        assert!(!ir.contains("call void @llvm.trap"), "{ir}");
    }

    /// With less room than the largest token and its closing byte the
    /// second addition stays checked.
    #[test]
    fn keeps_sums_the_room_does_not_cover() {
        let ir = optimized(&ROOM_THEN_OFFSET.replace("ROOM", "16"));
        assert!(ir.contains("call void @llvm.trap"), "{ir}");
        assert!(ir.contains("icmp ugt i64 %end, -3"), "{ir}");
    }
}
