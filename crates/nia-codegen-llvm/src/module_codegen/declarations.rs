// SPDX-License-Identifier: GPL-3.0-or-later
use super::{AbiParam, AbiReturn, FunctionSignature, ModuleCodegen};
use nia_backend_ir::{
    BackendFunction, BackendFunctionInstance, BackendParam, BackendTraitObjectVtableEntry,
    BackendTraitObjectVtableFunction,
};
use nia_diagnostic::Diagnostic;
use nia_ids::{GlobalDefId, InternedTyId, ModuleId};
use nia_llvm::{Attribute, AttributeLoc, module::Linkage, values::FunctionValue};
use nia_span::Span;
use nia_ty::{ConstGenericArg, TyKind};

enum AdapterFunction<'a> {
    Function(&'a BackendFunction),
    Instance(&'a BackendFunctionInstance),
}

struct TraitObjectAdapterTarget<'ctx, 'a> {
    def_id: GlobalDefId,
    arg_module_id: ModuleId,
    self_arg: Option<InternedTyId>,
    args: &'a [InternedTyId],
    const_args: &'a [ConstGenericArg],
    function: FunctionValue<'ctx>,
}

impl<'a> AdapterFunction<'a> {
    fn params(&self) -> &'a [BackendParam] {
        match self {
            AdapterFunction::Function(function) => &function.params,
            AdapterFunction::Instance(instance) => &instance.params,
        }
    }

    fn return_type(&self) -> InternedTyId {
        match self {
            AdapterFunction::Function(function) => function.return_type,
            AdapterFunction::Instance(instance) => instance.return_type,
        }
    }
}

impl<'ctx, 'a> ModuleCodegen<'ctx, 'a> {
    pub(super) fn declare_structs(&mut self) -> Result<(), Diagnostic> {
        for item in self.program.structs.values() {
            let name = self.struct_symbol_name(item.def_id, item.name);
            let ty = self
                .context
                .opaque_struct_type(&name)
                .map_err(Self::diagnostic_from_llvm_error)?;
            self.structs.insert(item.def_id, ty);
        }
        for item in self.program.struct_instances_by_def.values().flatten() {
            let ty = self
                .context
                .opaque_struct_type(&item.symbol)
                .map_err(Self::diagnostic_from_llvm_error)?;
            self.struct_instances
                .entry(item.def_id)
                .or_default()
                .insert((item.args.clone(), item.const_args.clone()), ty);
            self.struct_instances_by_def
                .entry(item.def_id)
                .or_default()
                .push((item.args.clone(), item.const_args.clone(), ty));
            self.struct_instance_type_lookups.borrow_mut().clear();
        }
        for item in self.program.unions.values() {
            let name = self.struct_symbol_name(item.def_id, item.name);
            let ty = self
                .context
                .opaque_struct_type(&name)
                .map_err(Self::diagnostic_from_llvm_error)?;
            self.unions.insert(item.def_id, ty);
        }
        for item in self.program.union_instances_by_def.values().flatten() {
            let ty = self
                .context
                .opaque_struct_type(&item.symbol)
                .map_err(Self::diagnostic_from_llvm_error)?;
            self.union_instances
                .entry(item.def_id)
                .or_default()
                .insert((item.args.clone(), item.const_args.clone()), ty);
            self.union_instances_by_def
                .entry(item.def_id)
                .or_default()
                .push((item.args.clone(), item.const_args.clone(), ty));
            self.union_instance_type_lookups.borrow_mut().clear();
        }
        Ok(())
    }

    pub(super) fn define_struct_bodies(&mut self) -> Result<(), Diagnostic> {
        for item in self.program.structs.values() {
            let Some(struct_ty) = self.structs.get(&item.def_id).copied() else {
                return Err(self.error(
                    item.span,
                    format!(
                        "missing LLVM struct for `{}`",
                        self.symbol_debug_name(item.name)
                    ),
                ));
            };
            let mut fields = Vec::new();
            for field in self.physical_struct_fields(item.def_id, &[], &[], item.span)? {
                fields.push(self.llvm_basic_type_in(field.ty, field.span)?);
            }
            struct_ty.set_body(&fields, false);
        }
        for item in self.program.struct_instances_by_def.values().flatten() {
            let Some(struct_ty) = self
                .struct_instances
                .get(&item.def_id)
                .and_then(|instances| instances.get(&(item.args.clone(), item.const_args.clone())))
                .copied()
            else {
                return Err(self.error(item.span, "missing LLVM struct instance"));
            };
            let mut fields = Vec::new();
            for field in
                self.physical_struct_fields(item.def_id, &item.args, &item.const_args, item.span)?
            {
                fields.push(self.llvm_basic_type_in(field.ty, field.span)?);
            }
            struct_ty.set_body(&fields, false);
        }
        for item in self.program.unions.values() {
            let Some(union_ty) = self.unions.get(&item.def_id).copied() else {
                return Err(self.error(
                    item.span,
                    format!(
                        "missing LLVM union for `{}`",
                        self.symbol_debug_name(item.name)
                    ),
                ));
            };
            union_ty.set_body(
                &self.union_storage_fields(item.def_id, &[], &[], item.span)?,
                false,
            );
        }
        for item in self.program.union_instances_by_def.values().flatten() {
            let Some(union_ty) = self
                .union_instances
                .get(&item.def_id)
                .and_then(|instances| instances.get(&(item.args.clone(), item.const_args.clone())))
                .copied()
            else {
                return Err(self.error(item.span, "missing LLVM union instance"));
            };
            union_ty.set_body(
                &self.union_storage_fields(item.def_id, &item.args, &item.const_args, item.span)?,
                false,
            );
        }
        Ok(())
    }

    pub(super) fn declare_functions(&mut self) -> Result<(), Diagnostic> {
        for function in self.program.functions.values() {
            if !function.generics.is_empty() {
                continue;
            }
            let ty = self.function_type_in(function)?;
            let is_local = function.def_id.module_id == self.source.id;
            let linkage = if function.is_extern {
                Some(Linkage::External)
            } else if is_local {
                None
            } else {
                Some(Linkage::External)
            };
            let value = self
                .module
                .add_function(&self.function_symbol_name(function), ty, linkage)
                .map_err(Self::diagnostic_from_llvm_error)?;
            self.apply_function_attributes(value, &function.attributes);
            self.functions.insert(function.def_id, value);
        }
        for instance in self.program.function_instances_by_def.values().flatten() {
            let ty = self.function_signature_type_in(FunctionSignature {
                param_tys: instance
                    .params
                    .iter()
                    .map(|param| (param.passing_ty, param.span)),
                return_type: instance.return_type,
                is_extern: instance.is_extern,
                is_variadic: instance.is_variadic,
                span: instance.span,
            })?;
            let value = self
                .module
                .add_function(
                    &instance.symbol,
                    ty,
                    if instance.is_extern {
                        Some(Linkage::External)
                    } else {
                        None
                    },
                )
                .map_err(Self::diagnostic_from_llvm_error)?;
            self.apply_function_attributes(value, &instance.attributes);
            self.function_instances
                .entry((instance.def_id, instance.arg_module_id))
                .or_default()
                .insert(
                    (
                        instance.self_arg,
                        instance.args.clone(),
                        instance.const_args.clone(),
                    ),
                    value,
                );
            self.function_instances_by_def
                .entry(instance.def_id)
                .or_default()
                .push((
                    instance.arg_module_id,
                    instance.self_arg,
                    instance.args.clone(),
                    instance.const_args.clone(),
                    value,
                ));
            self.function_instance_value_lookups.borrow_mut().clear();
        }
        Ok(())
    }

    fn apply_function_attributes(
        &self,
        value: nia_llvm::values::FunctionValue<'_>,
        attributes: &[nia_backend_ir::BackendFunctionAttribute],
    ) {
        for attribute in attributes {
            match attribute {
                nia_backend_ir::BackendFunctionAttribute::Naked => {
                    let kind = Attribute::get_named_enum_kind_id("naked");
                    if kind != 0 {
                        value.add_attribute(
                            AttributeLoc::Function,
                            self.context.create_enum_attribute(kind, 0),
                        );
                    }
                }
            }
        }
    }

    pub(super) fn declare_globals(&mut self) -> Result<(), Diagnostic> {
        for global in self.program.globals.values() {
            let ty = self.llvm_basic_type_in(global.ty, global.span)?;
            let value = self
                .module
                .add_global(ty, None, &self.global_symbol_name(global))
                .map_err(Self::diagnostic_from_llvm_error)?;
            let is_local = global.def_id.module_id == self.source.id;
            if global.is_extern || !is_local {
                value.set_linkage(Linkage::External);
            }
            if global.is_let {
                value.set_constant(true);
            }
            self.globals.insert(global.def_id, value);
        }
        for global in self.program.global_instances_by_def.values().flatten() {
            let ty = self.llvm_basic_type_in(global.ty, global.span)?;
            let value = self
                .module
                .add_global(ty, None, &global.symbol)
                .map_err(Self::diagnostic_from_llvm_error)?;
            if global.def_id.module_id != self.source.id {
                value.set_linkage(Linkage::External);
            }
            if global.is_let {
                value.set_constant(true);
            }
            self.global_instances.insert(
                (
                    global.def_id,
                    global.arg_module_id,
                    global.args.clone(),
                    global.const_args.clone(),
                ),
                value,
            );
        }
        for global in self.program.globals.values() {
            if global.def_id.module_id != self.source.id || global.is_extern {
                continue;
            }
            let ty = self.llvm_basic_type_in(global.ty, global.span)?;
            let Some(value) = self.globals.get(&global.def_id).copied() else {
                return Err(self.error(global.span, "missing global declaration"));
            };
            let init = match &global.init {
                Some(init) => self.static_init_value_in(global.ty, init, global.span)?,
                None => ty.const_zero().map_err(Self::diagnostic_from_llvm_error)?,
            };
            let init_ty = init.get_type().map_err(Self::diagnostic_from_llvm_error)?;
            if init_ty != ty {
                return Err(self.error(
                    global.span,
                    format!(
                        "global `{}` initializer type does not match declaration: expected {ty:?}, got {init_ty:?}",
                        self.symbol_debug_name(global.name)
                    ),
                ));
            }
            value.set_initializer(&init);
        }
        for global in self.program.global_instances_by_def.values().flatten() {
            if global.def_id.module_id != self.source.id {
                continue;
            }
            let ty = self.llvm_basic_type_in(global.ty, global.span)?;
            let Some(value) = self
                .global_instances
                .get(&(
                    global.def_id,
                    global.arg_module_id,
                    global.args.clone(),
                    global.const_args.clone(),
                ))
                .copied()
            else {
                return Err(self.error(global.span, "missing global instance declaration"));
            };
            let init = match &global.init {
                Some(init) => self.static_init_value_in(global.ty, init, global.span)?,
                None => ty.const_zero().map_err(Self::diagnostic_from_llvm_error)?,
            };
            let init_ty = init.get_type().map_err(Self::diagnostic_from_llvm_error)?;
            if init_ty != ty {
                return Err(self.error(
                    global.span,
                    format!(
                        "global instance `{}` initializer type does not match declaration: expected {ty:?}, got {init_ty:?}",
                        self.symbol_debug_name(global.name)
                    ),
                ));
            }
            value.set_initializer(&init);
        }
        Ok(())
    }

    pub(super) fn declare_trait_object_vtables(&mut self) -> Result<(), Diagnostic> {
        let ptr_ty = self.context.ptr_type(Default::default());
        let mut inserted_vtable = false;
        for module in self.program.modules.values() {
            for vtable in &module.trait_object_vtables {
                if self
                    .trait_object_vtables
                    .contains_key(&(vtable.key.self_ty, vtable.key.object_ty))
                {
                    continue;
                }
                let array_ty = ptr_ty.array_type(vtable.entries.len() as u32);
                let global = self
                    .module
                    .add_global(
                        array_ty.into(),
                        None,
                        &self.trait_object_vtable_symbol(vtable.key.self_ty, vtable.key.object_ty),
                    )
                    .map_err(Self::diagnostic_from_llvm_error)?;
                if module.id != self.source.id {
                    global.set_linkage(Linkage::External);
                } else {
                    let mut values = Vec::new();
                    for entry in &vtable.entries {
                        let value = self
                            .trait_object_vtable_entry_function(
                                vtable.key.self_ty,
                                entry,
                                vtable.span,
                            )?
                            .as_global_value()
                            .as_pointer_value();
                        values.push(value);
                    }
                    global.set_initializer(&ptr_ty.const_array(&values));
                    global.set_constant(true);
                }
                self.trait_object_vtables
                    .insert((vtable.key.self_ty, vtable.key.object_ty), global);
                inserted_vtable = true;
            }
        }
        if inserted_vtable {
            self.trait_object_vtable_lookups.borrow_mut().clear();
        }
        Ok(())
    }

    fn trait_object_vtable_entry_function(
        &self,
        self_ty: InternedTyId,
        entry: &BackendTraitObjectVtableEntry,
        span: Span,
    ) -> Result<FunctionValue<'ctx>, Diagnostic> {
        let (def_id, arg_module_id, self_arg, args, const_args, function) = match &entry.function {
            BackendTraitObjectVtableFunction::Function(def_id) => {
                let function = self
                    .function(*def_id)
                    .ok_or_else(|| self.error(span, "missing vtable method function"))?;
                (
                    *def_id,
                    self.source.id,
                    None,
                    Vec::new(),
                    Vec::new(),
                    function,
                )
            }
            BackendTraitObjectVtableFunction::FunctionInstance {
                def_id,
                arg_module_id,
                self_arg,
                args,
                const_args,
            } => {
                let function = self
                    .function_instance_value(*def_id, *arg_module_id, *self_arg, args, const_args)
                    .ok_or_else(|| self.error(span, "missing vtable method function instance"))?;
                (
                    *def_id,
                    *arg_module_id,
                    *self_arg,
                    args.clone(),
                    const_args.clone(),
                    function,
                )
            }
        };
        if !matches!(self.ty_kind(self_ty), Some(TyKind::SlicePointee { .. })) {
            return Ok(function);
        }
        self.trait_object_slice_adapter(
            self_ty,
            TraitObjectAdapterTarget {
                def_id,
                arg_module_id,
                self_arg,
                args: &args,
                const_args: &const_args,
                function,
            },
            span,
        )
    }

    fn trait_object_slice_adapter(
        &self,
        self_ty: InternedTyId,
        target: TraitObjectAdapterTarget<'ctx, '_>,
        span: Span,
    ) -> Result<FunctionValue<'ctx>, Diagnostic> {
        let TraitObjectAdapterTarget {
            def_id,
            arg_module_id,
            self_arg,
            args,
            const_args,
            function: target,
        } = target;
        let key = (
            self_ty,
            def_id,
            arg_module_id,
            self_arg,
            args.to_vec(),
            const_args.to_vec(),
        );
        if let Some(function) = self.trait_object_adapters.borrow().get(&key).copied() {
            return Ok(function);
        }
        let Some(item) = (if self_arg.is_none() && args.is_empty() && const_args.is_empty() {
            self.function_item(def_id).map(AdapterFunction::Function)
        } else {
            self.function_instance_item_with_arg_module(
                def_id,
                arg_module_id,
                self_arg,
                args,
                const_args,
            )
            .map(AdapterFunction::Instance)
        }) else {
            return Err(self.error(span, "missing vtable adapter target"));
        };
        let params = item.params();
        let Some(receiver) = params.first() else {
            return Err(self.error(span, "vtable adapter target has no receiver"));
        };
        let value_params = params
            .iter()
            .skip(1)
            .map(|param| param.passing_ty)
            .collect::<Vec<_>>();
        let function_ty =
            self.dynamic_trait_method_type(self_ty, &value_params, item.return_type(), span)?;
        let name = format!(
            "nia__traitobj_adapter__{}__{}__{}__{}",
            self.mangle_ty(self_ty),
            def_id.module_id.0,
            def_id.def_id.0,
            self.trait_object_adapters.borrow().len()
        );
        let adapter = self.add_internal_helper_function(&name, function_ty)?;
        let builder = self.context.create_builder();
        let entry = self.context.append_basic_block(adapter, "entry")?;
        builder.position_at_end(entry);
        let mut param_index = 0;
        let mut call_args = Vec::new();
        if let AbiReturn::IndirectOut(_) = self.classify_function_return(item.return_type()) {
            let out_ptr = adapter
                .get_nth_param(param_index)
                .ok_or_else(|| self.error(span, "missing vtable adapter out pointer"))?
                .map_err(Self::diagnostic_from_llvm_error)?;
            call_args.push(out_ptr);
            param_index += 1;
        }
        let self_ptr = adapter
            .get_nth_param(param_index)
            .ok_or_else(|| self.error(span, "missing vtable adapter self pointer"))?
            .map_err(Self::diagnostic_from_llvm_error)?
            .into_pointer_value()?;
        param_index += 1;
        let slice_ty = self.llvm_basic_type(receiver.passing_ty, receiver.span)?;
        let slice_value = builder
            .build_load(slice_ty, self_ptr, "traitobj.self")
            .map_err(|_| self.error(receiver.span, "failed to load trait object self"))?;
        call_args.push(slice_value);
        for classification in self.classify_function_params(value_params.iter().copied()) {
            match classification {
                AbiParam::Direct(_) | AbiParam::IndirectReadonly(_) => {
                    let arg = adapter
                        .get_nth_param(param_index)
                        .ok_or_else(|| self.error(span, "missing vtable adapter argument"))?
                        .map_err(Self::diagnostic_from_llvm_error)?;
                    call_args.push(arg);
                    param_index += 1;
                }
                AbiParam::Omit => {}
            }
        }
        let call = builder
            .build_call(target, &call_args, "traitobj.call")
            .map_err(|_| self.error(span, "failed to build vtable adapter call"))?;
        match self.classify_function_return(item.return_type()) {
            AbiReturn::Direct(_) => {
                let value = call
                    .try_as_basic_value()
                    .unwrap_basic()
                    .map_err(|_| self.error(span, "vtable adapter call did not return a value"))?;
                builder
                    .build_return(Some(&value))
                    .map_err(|_| self.error(span, "failed to return vtable adapter value"))?;
            }
            AbiReturn::Void | AbiReturn::IndirectOut(_) | AbiReturn::Never => {
                builder
                    .build_return(None)
                    .map_err(|_| self.error(span, "failed to return from vtable adapter"))?;
            }
        }
        self.trait_object_adapters.borrow_mut().insert(key, adapter);
        Ok(adapter)
    }
}
