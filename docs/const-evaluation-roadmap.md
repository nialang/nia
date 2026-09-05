# Const Evaluation Roadmap

Status: active roadmap (`F4` implemented)

Nia's const evaluator supports the builtin operations documented as
const-capable. Runtime-only builtins remain callable from ordinary expressions
but are rejected when reached from a constant expression. Any expansion must
be implemented through the complete owner chain:

```text
std declaration -> nia-ids capability -> nia-body-check -> nia-const-check evaluator -> tests/spec
```

## F4: Bit-counting Builtins

`std::builtin::ctz`, `clz`, and `popcount` are const-capable. The existing
typed builtin evaluator computes their result directly as an `IntConst`; no
separate const-value variant or lowering is required. Evaluation first masks
the input to the target primitive width, so signed inputs use the same
two's-complement bit pattern as their runtime representation. `ctz(0)` and
`clz(0)` return that width, while `popcount(0)` returns zero.

The implementation is owned by three pieces that must stay aligned:

1. `BuiltinFunction::is_const_capable()` admits the operations during const
   body checking.
2. `nia-const-check` evaluates the typed call using target width and signedness.
3. Standard-library declarations are marked `const fn`, with driver tests
   covering direct bindings, wrappers, zero values, narrow widths, signed
   bit patterns, and runtime behavior.

Future const builtin work should follow this capability-plus-evaluator pattern
instead of adding a new IR variant when an existing constant value is enough.
