# Const Evaluation Roadmap

Status: open feature roadmap

Nia's const evaluator currently supports the builtin operations documented as
const-capable. Runtime-only builtins remain callable from ordinary expressions
but are rejected when reached from a constant expression. Any expansion must
be implemented through the full owner chain:

```text
std declaration -> nia-const-check capability -> nia-const-ir/eval -> tests/spec
```

## Bit-counting Builtins

`std::builtin::ctz`, `clz`, and `popcount` are currently runtime-only. Their
runtime lowering exists, but `nia-const-eval` has no const representations for
them and `nia-const-check` does not admit them in const expressions.

This is a feature request, not a correctness bug. Implementing it requires:

1. Define target-width and zero-input semantics in the const evaluator. `ctz(0)`
   and `clz(0)` must return the primitive bit width, matching the language
   specification and runtime behavior.
2. Add the corresponding const IR/evaluator operations and capability admission.
3. Mark the standard-library declarations `const fn` only after the evaluator
   and checker are complete.
4. Add direct const bindings, `const fn` wrappers, target-width cases, zero-input
   cases, and runtime/const equivalence tests.

Until that work lands, the declarations remain ordinary `pub fn` and the
language specification describes them as runtime operations.
