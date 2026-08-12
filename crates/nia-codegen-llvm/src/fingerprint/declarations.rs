// SPDX-License-Identifier: GPL-3.0-or-later
//! Declaration and ABI closure encoding for source-unit fingerprints.

use super::*;

impl Encoder<'_> {
    pub(super) fn declaration_membership(
        &mut self,
        declarations: &CodegenDeclarationMembership,
        target: nia_layout::TargetDataLayout,
    ) {
        self.builder.write_u64(target.pointer_size);
        self.builder.write_u64(target.pointer_align);

        // Membership is already canonical and dependency-complete. Encoding
        // only these entries keeps unrelated foreign ABI edits from evicting
        // the unit while still covering every declaration codegen may inspect.
        self.len(declarations.structs.len());
        for &def_id in &declarations.structs {
            let item = self.index.struct_item(def_id).unwrap_or_else(|| {
                panic!("Nia ICE: declaration membership references missing struct {def_id:?}")
            });
            self.aggregate(
                item.def_id,
                item.name,
                &item.generics,
                &item.fields,
                item.is_extern,
            );
            self.optional_struct_layout(self.index.struct_layout(item.def_id));
        }
        self.len(declarations.struct_instances.len());
        for key in &declarations.struct_instances {
            let item = self
                .index
                .struct_instance(key.def_id, &key.args, &key.const_args)
                .unwrap_or_else(|| {
                    panic!("Nia ICE: declaration membership references missing struct instance")
                });
            self.aggregate_instance(
                item.def_id,
                item.name,
                &item.args,
                &item.const_args,
                &item.symbol,
                &item.fields,
                item.is_extern,
            );
            self.optional_struct_layout(self.index.struct_instance_layout(
                item.def_id,
                &item.args,
                &item.const_args,
            ));
        }
        self.len(declarations.unions.len());
        for &def_id in &declarations.unions {
            let item = self.index.union_item(def_id).unwrap_or_else(|| {
                panic!("Nia ICE: declaration membership references missing union {def_id:?}")
            });
            self.aggregate(
                item.def_id,
                item.name,
                &item.generics,
                &item.fields,
                item.is_extern,
            );
            self.optional_struct_layout(self.index.union_layout(item.def_id));
        }
        self.len(declarations.union_instances.len());
        for key in &declarations.union_instances {
            let item = self
                .index
                .union_instance(key.def_id, &key.args, &key.const_args)
                .unwrap_or_else(|| {
                    panic!("Nia ICE: declaration membership references missing union instance")
                });
            self.aggregate_instance(
                item.def_id,
                item.name,
                &item.args,
                &item.const_args,
                &item.symbol,
                &item.fields,
                item.is_extern,
            );
            self.optional_struct_layout(self.index.union_instance_layout(
                item.def_id,
                &item.args,
                &item.const_args,
            ));
        }
        self.len(declarations.globals.len());
        for &def_id in &declarations.globals {
            let item = self.index.global(def_id).unwrap_or_else(|| {
                panic!("Nia ICE: declaration membership references missing global {def_id:?}")
            });
            self.global_declaration(item);
        }
        self.len(declarations.global_instances.len());
        for key in &declarations.global_instances {
            let item = self
                .index
                .global_instance(key.def_id, key.arg_module_id, &key.args, &key.const_args)
                .unwrap_or_else(|| {
                    panic!("Nia ICE: declaration membership references missing global instance")
                });
            self.global_instance_declaration(item);
        }
        self.len(declarations.functions.len());
        for &def_id in &declarations.functions {
            let item = self.index.function(def_id).unwrap_or_else(|| {
                panic!("Nia ICE: declaration membership references missing function {def_id:?}")
            });
            self.function_declaration(item);
        }
        self.len(declarations.function_instances.len());
        for key in &declarations.function_instances {
            let item = self
                .index
                .function_instance(
                    key.def_id,
                    key.arg_module_id,
                    key.self_arg,
                    &key.args,
                    &key.const_args,
                )
                .unwrap_or_else(|| {
                    panic!("Nia ICE: declaration membership references missing function instance")
                });
            self.function_instance_declaration(item);
        }
        self.len(declarations.vtables.len());
        for key in &declarations.vtables {
            let item = self.index.trait_object_vtable(key).unwrap_or_else(|| {
                panic!("Nia ICE: declaration membership references missing vtable {key:?}")
            });
            self.trait_object_vtable_declaration(item);
        }
    }

    fn global_declaration(&mut self, item: &BackendGlobal) {
        self.global_def(item.def_id);
        self.symbol(item.name);
        self.optional_str(item.link_name.as_deref());
        self.ty(item.ty);
        self.bool(item.is_let);
        self.bool(item.is_extern);
    }

    fn global_instance_declaration(&mut self, item: &BackendGlobalInstance) {
        self.global_def(item.def_id);
        self.builder.write_str(&item.symbol);
        self.ty(item.ty);
        self.bool(item.is_let);
    }

    fn function_declaration(&mut self, item: &BackendFunction) {
        self.global_def(item.def_id);
        self.symbol(item.name);
        self.optional_str(item.link_name.as_deref());
        self.declaration_params(&item.params);
        self.ty(item.return_type);
        self.bool(item.is_extern);
        self.bool(item.is_variadic);
        self.function_attributes(&item.attributes);
    }

    fn function_instance_declaration(&mut self, item: &BackendFunctionInstance) {
        self.global_def(item.def_id);
        self.builder.write_str(&item.symbol);
        self.declaration_params(&item.params);
        self.ty(item.return_type);
        self.bool(item.is_extern);
        self.bool(item.is_variadic);
        self.function_attributes(&item.attributes);
    }

    fn declaration_params(&mut self, params: &[BackendParam]) {
        self.len(params.len());
        for param in params {
            self.ty(param.passing_ty);
        }
    }

    fn trait_object_vtable_declaration(&mut self, item: &BackendTraitObjectVtable) {
        self.ty(item.key.self_ty);
        self.ty(item.key.object_ty);
        self.len(item.entries.len());
    }
}
