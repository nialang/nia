import NiaFormal.SymbolicConst

/-!
Residual const computations preserve partial execution, not algebraic integer
equations. The primitive/call kernel below is a parameter: this proves the
substitution and lazy-execution boundary, not the Rust interpreter or call body.
-/
namespace NiaConstComputation

open NiaSymbolicConst.Scalar

inductive Expr where
  | literal (value : Value)
  | parameter (id : Nat)
  | unary (operation : Nat) (operand : Expr)
  | binary (operation : Nat) (left right : Expr)
  | branch (condition yes no : Expr)
  | call (function : Nat) (argument : Expr)

structure Kernel where
  unary : Nat → Value → Option Value
  binary : Nat → Value → Value → Option Value
  call : Nat → Value → Option Value

def eval (kernel : Kernel) (env : Nat → Option Value) : Expr → Option Value
  | .literal value => some value
  | .parameter id => env id
  | .unary op operand => (eval kernel env operand).bind (kernel.unary op)
  | .binary op left right => do
      let left ← eval kernel env left
      let right ← eval kernel env right
      kernel.binary op left right
  | .branch condition yes no => do
      match ← eval kernel env condition with
      | .bool true => eval kernel env yes
      | .bool false => eval kernel env no
      | _ => none
  | .call function argument => (eval kernel env argument).bind (kernel.call function)

def substitute (replacement : Nat → Expr) : Expr → Expr
  | .literal value => .literal value
  | .parameter id => replacement id
  | .unary op operand => .unary op (substitute replacement operand)
  | .binary op left right => .binary op (substitute replacement left) (substitute replacement right)
  | .branch condition yes no => .branch (substitute replacement condition)
      (substitute replacement yes) (substitute replacement no)
  | .call function argument => .call function (substitute replacement argument)

theorem substitution_preserves_execution (kernel : Kernel)
    (source destination : Nat → Option Value) (replacement : Nat → Expr)
    (compatible : ∀ id, eval kernel destination (replacement id) = source id)
    (expr : Expr) :
    eval kernel destination (substitute replacement expr) = eval kernel source expr := by
  induction expr with
  | literal => rfl
  | parameter id => exact compatible id
  | unary op operand ih => simp only [substitute, eval, ih]
  | binary op left right ihl ihr => simp only [substitute, eval, ihl, ihr]
  | branch condition yes no ihc ihy ihn => simp only [substitute, eval, ihc, ihy, ihn]
  | call function argument ih => simp only [substitute, eval, ih]

theorem substitution_preserves_failure (kernel : Kernel)
    (source destination : Nat → Option Value) (replacement : Nat → Expr)
    (compatible : ∀ id, eval kernel destination (replacement id) = source id)
    (expr : Expr) :
    eval kernel destination (substitute replacement expr) = none ↔
      eval kernel source expr = none := by
  rw [substitution_preserves_execution kernel source destination replacement compatible]

theorem unselected_branch_is_not_executed (kernel : Kernel) (env : Nat → Option Value)
    (yes no : Expr) :
    eval kernel env (.branch (.literal (.bool true)) yes no) = eval kernel env yes := by
  rfl

-- Boolean short circuiting is conditional execution of the right operand.
def lazyOr (left right : Expr) : Expr := .branch left (.literal (.bool true)) right
def lazyAnd (left right : Expr) : Expr := .branch left right (.literal (.bool false))

example (kernel : Kernel) (env : Nat → Option Value) (right : Expr) :
    eval kernel env (lazyOr (.literal (.bool true)) right) = some (.bool true) := rfl
example (kernel : Kernel) (env : Nat → Option Value) (right : Expr) :
    eval kernel env (lazyAnd (.literal (.bool false)) right) = some (.bool false) := rfl

-- Scalar publication has one target/type validation boundary. A result may
-- fail execution or fail validation; neither failure publishes a value.
def publish (target : Target) (ty : Ty) (result : Option Value) : Option Value := do
  let value ← result
  if validate target ty value then some value else none

theorem published_value_fits (target : Target) (ty : Ty) (result : Option Value)
    (published : publish target ty result = some value) : conforms target ty value := by
  cases result with
  | none => simp [publish] at published
  | some candidate =>
      by_cases valid : validate target ty candidate = true
      · have equal : candidate = value := by simpa [publish, valid] using published
        subst candidate
        exact (validation_is_exact target ty value).mp valid
      · simp [publish, valid] at published

theorem substitution_preserves_publication (target : Target) (ty : Ty) (kernel : Kernel)
    (source destination : Nat → Option Value) (replacement : Nat → Expr)
    (compatible : ∀ id, eval kernel destination (replacement id) = source id) (expr : Expr) :
    publish target ty (eval kernel destination (substitute replacement expr)) =
      publish target ty (eval kernel source expr) := by
  rw [substitution_preserves_execution kernel source destination replacement compatible]

-- Checked arithmetic is partial: reassociation can erase an overflow.
def addI8 (left right : Int) : Option Int :=
  if -128 ≤ left + right ∧ left + right < 128 then some (left + right) else none

-- Every initial operand fits i8; only the left-associated intermediate fails.
example : ((addI8 127 1).bind (fun value => addI8 value (-1))) = none := by decide
example : ((addI8 1 (-1)).bind (fun value => addI8 127 value)) = some 127 := by decide

example : NiaSymbolicConst.manifestNames "computation_cases" = [
    "local", "repeat", "annotation", "argument", "abstraction", "generic_call",
    "signature", "nominal", "nested_call", "lazy_or", "lazy_and", "lazy_if",
    "loop_abstraction", "layout_operand", "alias", "cast", "shift", "negative",
    "unused_generic"
  ] := by native_decide

example : NiaSymbolicConst.manifestNames "computation_laws" = [
    "substitution_preserves_execution", "substitution_preserves_failure",
    "unselected_branch_is_not_executed", "published_value_fits",
    "substitution_preserves_publication"
  ] := by native_decide

end NiaConstComputation
