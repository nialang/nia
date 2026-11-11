// SPDX-License-Identifier: GPL-3.0-or-later
//! A checked entry has only the build's features. Its implementation has the
//! additional features, and must never be inlined into that entry. Pointers
//! always name the checked entry; feature-compatible always-inline calls may
//! name the implementation directly.

use super::ModuleCodegen;
use nia_backend_ir::BackendFunctionAttribute;
use nia_diagnostic::Diagnostic;
use nia_llvm::{Attribute, AttributeLoc, module::Linkage, values::FunctionValue};

pub(crate) fn implementation_symbol(entry: String) -> nia_ice::IceResult<String> {
    nia_mangle::mangle_derived_symbol_canonical(
        nia_mangle::COMPILER_GENERATED_PACKAGE_IDENTITY,
        nia_mangle::MangleModuleId::from_normalized_source_path("nia:cpu-features"),
        "implementation",
        "cpu_features",
        nia_mangle::MangleSymbolKind::Function,
        [entry],
    )
}

impl<'ctx> ModuleCodegen<'ctx, '_> {
    pub(super) fn declare_cpu_implementation(
        &mut self,
        entry: FunctionValue<'ctx>,
        attributes: &[BackendFunctionAttribute],
    ) -> Result<(), Diagnostic> {
        let checked = attributes.iter().any(|attribute| {
            matches!(
                attribute,
                BackendFunctionAttribute::CpuFeatures { checked: true, .. }
            )
        });
        if !checked {
            if attributes.contains(&BackendFunctionAttribute::InlineAlways)
                && attributes.iter().any(|attribute| {
                    matches!(attribute, BackendFunctionAttribute::CpuFeatures { .. })
                })
            {
                self.cpu_inline_implementations.insert(
                    entry,
                    (entry, nia_backend_ir::function_cpu_features(attributes)),
                );
            }
            return self.apply_function_attributes(entry, attributes);
        }
        let symbol = implementation_symbol(entry.name()).map_err(Diagnostic::from)?;
        let implementation = self
            .module
            .add_function(
                &symbol,
                entry.get_type().map_err(Self::diagnostic_from_llvm_error)?,
                Some(Linkage::External),
            )
            .map_err(Self::diagnostic_from_llvm_error)?;
        implementation
            .as_global_value()
            .set_visibility(nia_llvm::module::Visibility::Hidden);
        self.apply_function_attributes(implementation, attributes)?;
        self.cpu_implementations.insert(entry, implementation);
        if attributes.contains(&BackendFunctionAttribute::InlineAlways) {
            self.cpu_inline_implementations.insert(
                entry,
                (
                    implementation,
                    nia_backend_ir::function_cpu_features(attributes),
                ),
            );
        }
        // The checked entry is a real ABI boundary, including under LTO.
        let noinline = Attribute::get_named_enum_kind_id("noinline");
        entry.add_attribute(
            AttributeLoc::Function,
            self.context
                .create_enum_attribute(noinline, 0)
                .map_err(Self::diagnostic_from_llvm_error)?,
        );
        Ok(())
    }

    pub(crate) fn direct_call_entry(
        &self,
        entry: FunctionValue<'ctx>,
        owner: &nia_backend_ir::BackendClosureEntryOwner,
    ) -> (FunctionValue<'ctx>, bool) {
        let Some(&(implementation, required)) = self.cpu_inline_implementations.get(&entry) else {
            return (entry, false);
        };
        let attributes = match owner {
            nia_backend_ir::BackendClosureEntryOwner::Source(def_id) => self
                .program
                .function(*def_id)
                .map(|function| function.attributes.as_slice()),
            nia_backend_ir::BackendClosureEntryOwner::FunctionInstance(key) => self
                .program
                .function_instance(
                    key.def_id,
                    key.arg_module_id,
                    key.self_arg,
                    &key.args,
                    &key.const_args,
                )
                .map(|function| function.attributes.as_slice()),
        }
        .unwrap_or_default();
        if required.is_subset_of(
            nia_backend_ir::function_cpu_features(attributes).union(self.build_cpu_features),
        ) {
            (implementation, true)
        } else {
            (entry, false)
        }
    }

    pub(super) fn implementation_entry(&self, entry: FunctionValue<'ctx>) -> FunctionValue<'ctx> {
        self.cpu_implementations
            .get(&entry)
            .copied()
            .unwrap_or(entry)
    }

    pub(super) fn emit_cpu_wrapper(
        &self,
        entry: FunctionValue<'ctx>,
        attributes: &[BackendFunctionAttribute],
        return_type: nia_ids::InternedTyId,
        c_params: Option<&[nia_backend_ir::BackendParam]>,
    ) -> Result<(), Diagnostic> {
        let Some(&implementation) = self.cpu_implementations.get(&entry) else {
            return Ok(());
        };
        if entry.get_first_basic_block().is_some() {
            return Ok(());
        }
        let features = nia_backend_ir::function_cpu_features(attributes);
        let builder = self
            .context
            .create_builder()
            .map_err(Self::diagnostic_from_llvm_error)?;
        builder.position_at_end(self.context.append_basic_block(entry, "cpu.check")?);
        let check_type = self
            .context
            .void_type()
            .fn_type(&[self.context.i32_type().into()], false)
            .map_err(Self::diagnostic_from_llvm_error)?;
        let check = match self
            .module
            .get_function("__nia_check_cpu_features")
            .map_err(Self::diagnostic_from_llvm_error)?
        {
            Some(check) => check,
            None => self
                .module
                .add_function(
                    "__nia_check_cpu_features",
                    check_type,
                    Some(Linkage::External),
                )
                .map_err(Self::diagnostic_from_llvm_error)?,
        };
        if check.get_type().map_err(Self::diagnostic_from_llvm_error)? != check_type {
            return Err(Diagnostic::user_error(
                nia_diagnostic::codes::LLVM_CODEGEN,
                "`__nia_check_cpu_features` must have C signature `void(uint32_t)`",
            )
            .finish());
        }
        // This routine must remain baseline even when a checked entry is
        // imported by LTO into a feature-specific unit.
        let noinline = Attribute::get_named_enum_kind_id("noinline");
        check.add_attribute(
            AttributeLoc::Function,
            self.context
                .create_enum_attribute(noinline, 0)
                .map_err(Self::diagnostic_from_llvm_error)?,
        );
        builder
            .build_call(
                check,
                &[self
                    .context
                    .i32_type()
                    .const_int(u64::from(features.bits()), false)
                    .map_err(Self::diagnostic_from_llvm_error)?
                    .into()],
                "",
            )
            .map_err(Self::diagnostic_from_llvm_error)?;
        let ty = entry.get_type().map_err(Self::diagnostic_from_llvm_error)?;
        let mut args = Vec::new();
        for index in 0..ty
            .get_param_types()
            .map_err(Self::diagnostic_from_llvm_error)?
            .len()
        {
            args.push(
                entry
                    .get_nth_param(index as u32)
                    .ok_or_else(|| {
                        self.error(nia_span::Span::default(), "missing checked-entry parameter")
                    })?
                    .map_err(Self::diagnostic_from_llvm_error)?,
            );
        }
        let call = builder
            .build_call(implementation, &args, "cpu.call")
            .map_err(Self::diagnostic_from_llvm_error)?;
        if let Some(params) = c_params {
            for (location, attribute) in self.c_signature_attributes(
                params.iter().map(|param| (param.passing_ty, param.span)),
                return_type,
                nia_span::Span::default(),
            )? {
                call.add_attribute(location, attribute);
            }
        }
        // A generated pointer entry is exempt from the source-level direct
        // always-inline promise. It cannot inline the specialized body.
        call.add_attribute(
            AttributeLoc::Function,
            self.context
                .create_enum_attribute(noinline, 0)
                .map_err(Self::diagnostic_from_llvm_error)?,
        );
        if matches!(
            self.ty_kind(return_type),
            Some(nia_ty::TyKind::Primitive(nia_ty::PrimitiveTy::Never))
        ) {
            builder
                .build_unreachable()
                .map_err(Self::diagnostic_from_llvm_error)?;
            return Ok(());
        }
        match call.try_as_basic_value().basic() {
            Some(value) => {
                builder.build_return(Some(&value.map_err(Self::diagnostic_from_llvm_error)?))
            }
            None => builder.build_return(None),
        }
        .map_err(Self::diagnostic_from_llvm_error)?;
        Ok(())
    }
}
