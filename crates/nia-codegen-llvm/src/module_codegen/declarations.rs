// SPDX-License-Identifier: GPL-3.0-or-later
use super::{FunctionSignature, ModuleCodegen};
use nia_backend_ir::BackendTraitObjectVtableFunction;
use nia_diagnostic::Diagnostic;
use nia_llvm::module::Linkage;

impl<'ctx, 'a> ModuleCodegen<'ctx, 'a> {
    pub(super) fn declare_structs(&mut self) -> Result<(), Diagnostic> {
        for item in self.program.structs.values() {
            let name = self.struct_symbol_name(item.def_id, &item.name);
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
                .insert(item.args.clone(), ty);
            self.struct_instances_by_def
                .entry(item.def_id)
                .or_default()
                .push((item.args.clone(), ty));
            self.struct_instance_type_lookups.borrow_mut().clear();
        }
        for item in self.program.unions.values() {
            let name = self.struct_symbol_name(item.def_id, &item.name);
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
                .insert(item.args.clone(), ty);
            self.union_instances_by_def
                .entry(item.def_id)
                .or_default()
                .push((item.args.clone(), ty));
            self.union_instance_type_lookups.borrow_mut().clear();
        }
        Ok(())
    }

    pub(super) fn define_struct_bodies(&mut self) -> Result<(), Diagnostic> {
        for item in self.program.structs.values() {
            let Some(owner) = self.program.module(item.def_id.module_id) else {
                return Err(self.error(item.span, "missing struct owner module"));
            };
            let Some(struct_ty) = self.structs.get(&item.def_id).copied() else {
                return Err(self.error(
                    item.span,
                    format!("missing LLVM struct for `{}`", item.name),
                ));
            };
            let mut fields = Vec::new();
            for field in self.physical_struct_fields(item.def_id, &[], item.span)? {
                fields.push(self.llvm_basic_type_in(
                    field.ty,
                    field.span,
                    &owner.interner,
                    &owner.layouts,
                )?);
            }
            struct_ty.set_body(&fields, false);
        }
        for item in self.program.struct_instances_by_def.values().flatten() {
            let Some(owner) = self.program.module(item.def_id.module_id) else {
                return Err(self.error(item.span, "missing struct owner module"));
            };
            let Some(struct_ty) = self
                .struct_instances
                .get(&item.def_id)
                .and_then(|instances| instances.get(item.args.as_slice()))
                .copied()
            else {
                return Err(self.error(item.span, "missing LLVM struct instance"));
            };
            let mut fields = Vec::new();
            for field in self.physical_struct_fields(item.def_id, &item.args, item.span)? {
                fields.push(self.llvm_basic_type_in(
                    field.ty,
                    field.span,
                    &owner.interner,
                    &owner.layouts,
                )?);
            }
            struct_ty.set_body(&fields, false);
        }
        for item in self.program.unions.values() {
            let Some(union_ty) = self.unions.get(&item.def_id).copied() else {
                return Err(
                    self.error(item.span, format!("missing LLVM union for `{}`", item.name))
                );
            };
            union_ty.set_body(
                &self.union_storage_fields(item.def_id, &[], item.span)?,
                false,
            );
        }
        for item in self.program.union_instances_by_def.values().flatten() {
            let Some(union_ty) = self
                .union_instances
                .get(&item.def_id)
                .and_then(|instances| instances.get(item.args.as_slice()))
                .copied()
            else {
                return Err(self.error(item.span, "missing LLVM union instance"));
            };
            union_ty.set_body(
                &self.union_storage_fields(item.def_id, &item.args, item.span)?,
                false,
            );
        }
        Ok(())
    }

    pub(super) fn declare_functions(&mut self) -> Result<(), Diagnostic> {
        for function in self.program.functions.values() {
            let Some(owner) = self.program.module(function.def_id.module_id) else {
                return Err(self.error(function.span, "missing function owner module"));
            };
            let ty = self.function_type_in(function, &owner.interner, &owner.layouts)?;
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
            self.functions.insert(function.def_id, value);
        }
        for instance in self.program.function_instances_by_def.values().flatten() {
            let Some(owner) = self.program.module(instance.def_id.module_id) else {
                return Err(self.error(instance.span, "missing function owner module"));
            };
            let ty = self.function_signature_type_in(FunctionSignature {
                param_tys: instance.params.iter().map(|param| (param.ty, param.span)),
                return_type: instance.return_type,
                is_extern: instance.is_extern,
                is_variadic: instance.is_variadic,
                span: instance.span,
                interner: &owner.interner,
                layouts: &owner.layouts,
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
            self.function_instances
                .entry((instance.def_id, instance.arg_module_id))
                .or_default()
                .insert(instance.args.clone(), value);
            self.function_instances_by_def
                .entry(instance.def_id)
                .or_default()
                .push((instance.args.clone(), value));
            self.function_instance_value_lookups.borrow_mut().clear();
        }
        Ok(())
    }

    pub(super) fn declare_globals(&mut self) -> Result<(), Diagnostic> {
        for global in self.program.globals.values() {
            let Some(owner) = self.program.module(global.def_id.module_id) else {
                return Err(self.error(global.span, "missing global owner module"));
            };
            let ty =
                self.llvm_basic_type_in(global.ty, global.span, &owner.interner, &owner.layouts)?;
            let value = self
                .module
                .add_global(ty, None, &self.global_symbol_name(global))
                .map_err(Self::diagnostic_from_llvm_error)?;
            let is_local = global.def_id.module_id == self.source.id;
            if global.is_extern || !is_local {
                value.set_linkage(Linkage::External);
            }
            if global.is_const {
                value.set_constant(true);
            }
            self.globals.insert(global.def_id, value);
        }
        for global in self.program.globals.values() {
            if global.def_id.module_id != self.source.id || global.is_extern {
                continue;
            }
            let Some(owner) = self.program.module(global.def_id.module_id) else {
                return Err(self.error(global.span, "missing global owner module"));
            };
            let ty =
                self.llvm_basic_type_in(global.ty, global.span, &owner.interner, &owner.layouts)?;
            let Some(value) = self.globals.get(&global.def_id).copied() else {
                return Err(self.error(global.span, "missing global declaration"));
            };
            let init = match &global.init {
                Some(init) => self.static_init_value_in(
                    global.ty,
                    init,
                    global.span,
                    &owner.interner,
                    &owner.layouts,
                )?,
                None => ty.const_zero().map_err(Self::diagnostic_from_llvm_error)?,
            };
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
                        let value = match &entry.function {
                            BackendTraitObjectVtableFunction::Function(def_id) => self
                                .function(*def_id)
                                .ok_or_else(|| {
                                    self.error(vtable.span, "missing vtable method function")
                                })?
                                .as_global_value()
                                .as_pointer_value(),
                            BackendTraitObjectVtableFunction::FunctionInstance { def_id, args } => {
                                self.function_instance_value(*def_id, args)
                                    .ok_or_else(|| {
                                        self.error(
                                            vtable.span,
                                            "missing vtable method function instance",
                                        )
                                    })?
                                    .as_global_value()
                                    .as_pointer_value()
                            }
                        };
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
}
