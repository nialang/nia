// SPDX-License-Identifier: GPL-3.0-or-later
use nia_body_ir::AsmOption;
use nia_diagnostic::Diagnostic;
use nia_function_ir::FunctionInlineAsm;
use nia_llvm::{
    InlineAsmDialect, InlineAsmOptions,
    types::{BasicMetadataTypeEnum, BasicTypeEnum},
    values::PointerValue,
};

use super::FunctionCodegen;

impl<'m, 'ctx, 'a> FunctionCodegen<'m, 'ctx, 'a> {
    pub(super) fn emit_inline_asm(&mut self, asm: &FunctionInlineAsm) -> Result<(), Diagnostic> {
        let mut input_values = Vec::with_capacity(asm.inputs.len());
        for input in &asm.inputs {
            input_values.push(self.emit_expr(&input.value)?);
        }

        let mut output_ptrs = Vec::<PointerValue<'ctx>>::with_capacity(asm.outputs.len());
        let mut output_tys = Vec::<BasicTypeEnum<'ctx>>::with_capacity(asm.outputs.len());
        for output in &asm.outputs {
            output_ptrs.push(self.emit_typed_place_addr(&output.place)?);
            output_tys.push(self.module.llvm_basic_type(output.place.ty, output.span)?);
        }

        let mut constraints = Vec::new();
        constraints.extend(asm.outputs.iter().map(|output| output.constraint.clone()));
        constraints.extend(asm.inputs.iter().map(|input| input.constraint.clone()));
        constraints.extend(asm.clobbers.iter().map(|clobber| format!("~{{{clobber}}}")));
        let param_tys = input_values
            .iter()
            .map(|value| value.get_type())
            .collect::<Result<Vec<BasicMetadataTypeEnum<'ctx>>, _>>()?;
        let fn_ty = match output_tys.as_slice() {
            [] => self.module.context.void_type().fn_type(&param_tys, false),
            [ty] => ty.fn_type(&param_tys, false),
            tys => self
                .module
                .context
                .struct_type(tys, false)
                .fn_type(&param_tys, false),
        };
        let has_sideeffects = asm.options.contains(&AsmOption::Volatile) || output_tys.is_empty();
        let inline_asm = self.module.context.create_inline_asm(
            fn_ty,
            asm.code.clone(),
            constraints.join(","),
            InlineAsmOptions {
                sideeffects: has_sideeffects,
                alignstack: false,
                dialect: Some(InlineAsmDialect::Intel),
                can_throw: false,
            },
        );
        let call = self
            .builder
            .build_indirect_call(fn_ty, inline_asm, &input_values, "asm")
            .map_err(|_| self.error(self.function.span, "failed to emit inline assembly"))?;

        if output_tys.is_empty() {
            return Ok(());
        }
        let result = call
            .try_as_basic_value()
            .basic()
            .ok_or_else(|| self.error(self.function.span, "inline assembly output is missing"))??;
        for (index, ptr) in output_ptrs.iter().enumerate() {
            let value = if output_tys.len() == 1 {
                result
            } else {
                self.builder
                    .build_extract_value(
                        result.into_struct_value()?,
                        index as u32,
                        &format!("asm.out.{index}"),
                    )
                    .map_err(|_| {
                        self.error(
                            self.function.span,
                            "failed to extract inline assembly output",
                        )
                    })?
            };
            self.builder.build_store(*ptr, value).map_err(|_| {
                self.error(self.function.span, "failed to store inline assembly output")
            })?;
        }
        Ok(())
    }
}
