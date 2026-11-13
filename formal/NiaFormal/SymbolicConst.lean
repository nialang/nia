import Lean

namespace NiaSymbolicConst

/-
The compiler compares normalized scalar const identities before instantiation.
Type keys stand for canonical, alias-expanded operand types. The environment
supplies target-dependent layouts and parameter values; this model assumes
their validity and does not prove Rust name resolution, layout, or execution.
-/
inductive Query where
  | size | align
  deriving DecidableEq, Repr

inductive Length where
  | literal (value : Nat)
  | parameter (id : Nat)
  | layout (query : Query) (typeKey : Nat)
  deriving DecidableEq, Repr

structure Environment where
  parameter : Nat → Nat
  layout : Query → Nat → Nat

def evaluate (env : Environment) : Length → Nat
  | .literal n => n
  | .parameter id => env.parameter id
  | .layout query key => env.layout query key

def equivalent (a b : Length) : Bool := decide (a = b)

theorem symbolic_equality_is_sound (env : Environment) (a b : Length)
    (same : equivalent a b = true) : evaluate env a = evaluate env b := by
  have equality : a = b := of_decide_eq_true same
  rw [equality]

-- Coinciding values in one instance cannot establish a generic-body equality.
theorem literal_ne_parameter (n id : Nat) :
    equivalent (.literal n) (.parameter id) = false := by
  simp [equivalent]

structure Substitution where
  parameter : Nat → Length
  layout : Query → Nat → Length

def substitute (s : Substitution) : Length → Length
  | .literal n => .literal n
  | .parameter id => s.parameter id
  | .layout query key => s.layout query key

def substitutedEnvironment (env : Environment) (s : Substitution) : Environment where
  parameter id := evaluate env (s.parameter id)
  layout query key := evaluate env (s.layout query key)

theorem substitution_preserves_meaning (env : Environment) (s : Substitution) (n : Length) :
    evaluate env (substitute s n) = evaluate (substitutedEnvironment env s) n := by
  cases n <;> rfl

theorem equality_survives_instantiation (env : Environment) (s : Substitution)
    (a b : Length) (same : equivalent a b = true) :
    evaluate env (substitute s a) = evaluate env (substitute s b) := by
  rw [substitution_preserves_meaning, substitution_preserves_meaning]
  exact symbolic_equality_is_sound _ a b same

inductive ConstArgument where
  | integer (value : Nat)
  | parameter (id : Nat)
  | layout (query : Query) (typeKey : Nat)
  deriving DecidableEq, Repr

def toArgument : Length → ConstArgument
  | .literal n => .integer n
  | .parameter id => .parameter id
  | .layout query key => .layout query key

def toLength : ConstArgument → Length
  | .integer n => .literal n
  | .parameter id => .parameter id
  | .layout query key => .layout query key

theorem length_argument_roundtrip (n : Length) : toLength (toArgument n) = n := by
  cases n <;> rfl

theorem argument_length_roundtrip (a : ConstArgument) : toArgument (toLength a) = a := by
  cases a <;> rfl

theorem roundtrip_preserves_meaning (env : Environment) (n : Length) :
    evaluate env (toLength (toArgument n)) = evaluate env n := by
  rw [length_argument_roundtrip]

-- Resolution supplies semantic identities. Ordinary constants retain their
-- values, and runtime bindings cannot manufacture a symbolic parameter.
inductive ResolvedValue where
  | constant (value : Nat)
  | parameter (id : Nat)
  | layout (query : Query) (typeKey : Nat)
  | localAlias (id : Nat)
  | runtime (id : Nat)

def classify (aliases : Nat → Option Length) : ResolvedValue → Option Length
  | .constant value => some (.literal value)
  | .parameter id => some (.parameter id)
  | .layout query key => some (.layout query key)
  | .localAlias id => aliases id
  | .runtime _ => none

def resolvedMeaning (env : Environment) (locals : Nat → Nat) : ResolvedValue → Option Nat
  | .constant value => some value
  | .parameter id => some (env.parameter id)
  | .layout query key => some (env.layout query key)
  | .localAlias id => some (locals id)
  | .runtime _ => none

theorem classification_preserves_meaning (env : Environment) (locals : Nat → Nat)
    (aliases : Nat → Option Length)
    (valid : ∀ id n, aliases id = some n → evaluate env n = locals id)
    (value : ResolvedValue) (n : Length) (classified : classify aliases value = some n) :
    resolvedMeaning env locals value = some (evaluate env n) := by
  cases value with
  | constant v => simp [classify] at classified; subst n; rfl
  | parameter id => simp [classify] at classified; subst n; rfl
  | layout q k => simp [classify] at classified; subst n; rfl
  | localAlias id =>
      simp only [classify] at classified
      simp [resolvedMeaning, valid id n classified]
  | runtime id => simp [classify] at classified

theorem runtime_binding_is_rejected (aliases : Nat → Option Length) (id : Nat) :
    classify aliases (.runtime id) = none := rfl

/- Typed scalar boundary. Mathematical Int represents the value, not the
   interpreter's signed/raw-bit storage. Widths come from the artifact target,
   never from the machine running Lean or the compiler. Floats, char, pointers,
   and integer arithmetic are intentionally outside this fragment. -/
namespace Scalar

structure Target where
  pointerBits : Nat
  positive : 0 < pointerBits
  bounded : pointerBits ≤ 128

inductive Width where
  | fixed (bits : Nat)
  | pointer
  deriving DecidableEq

def Width.bits (target : Target) : Width → Nat
  | .fixed bits => bits
  | .pointer => target.pointerBits

inductive Ty where
  | bool
  | integer (signed : Bool) (width : Width)
  deriving DecidableEq

def usize : Ty := .integer false .pointer
def isize : Ty := .integer true .pointer

inductive Value where
  | bool (value : Bool)
  | integer (value : Int)
  deriving DecidableEq

def integerRange (signed : Bool) (bits : Nat) (value : Int) : Prop :=
  0 < bits ∧ bits ≤ 128 ∧
    if signed then -(2 ^ (bits - 1) : Int) ≤ value ∧ value < 2 ^ (bits - 1)
    else 0 ≤ value ∧ value < 2 ^ bits

def conforms (target : Target) : Ty → Value → Prop
  | .bool, .bool _ => True
  | .integer signed width, .integer value => integerRange signed (width.bits target) value
  | _, _ => False

instance (target : Target) (ty : Ty) (value : Value) :
    Decidable (conforms target ty value) := by
  cases ty <;> cases value <;> simp only [conforms] <;>
    first | infer_instance | unfold integerRange; infer_instance

def validate (target : Target) (ty : Ty) (value : Value) : Bool :=
  decide (conforms target ty value)

theorem validation_is_exact (target : Target) (ty : Ty) (value : Value) :
    validate target ty value = true ↔ conforms target ty value := by
  simp [validate]

theorem usize_uses_artifact_width (target : Target) (value : Int) :
    conforms target usize (.integer value) ↔
      0 ≤ value ∧ value < 2 ^ target.pointerBits := by
  simp [conforms, usize, Width.bits, integerRange, target.positive, target.bounded]

theorem isize_uses_artifact_width (target : Target) (value : Int) :
    conforms target isize (.integer value) ↔
      -(2 ^ (target.pointerBits - 1) : Int) ≤ value ∧
        value < 2 ^ (target.pointerBits - 1) := by
  simp [conforms, isize, Width.bits, integerRange, target.positive, target.bounded]

theorem integer_cannot_be_bool (target : Target) (value : Int) :
    validate target .bool (.integer value) = false := by
  simp [validate, conforms]

inductive Expr where
  | literal (ty : Ty) (value : Value)
  | parameter (id : Nat)
  | layout (query : Query) (typeKey : Nat)

structure Env where
  parameter : Nat → Value
  layout : Query → Nat → Nat

def eval (env : Env) : Expr → Value
  | .literal _ value => value
  | .parameter id => env.parameter id
  | .layout q key => .integer (env.layout q key)

-- Contexts retain the declaration's exact carrier type, including signedness.
inductive HasType (target : Target) (context : Nat → Ty) : Expr → Ty → Prop where
  | literal (valid : conforms target ty value) :
      HasType target context (.literal ty value) ty
  | parameter (id : Nat) : HasType target context (.parameter id) (context id)
  | layout (q : Query) (key : Nat) : HasType target context (.layout q key) usize

structure ValidEnv (target : Target) (context : Nat → Ty) (env : Env) : Prop where
  parameter : ∀ id, conforms target (context id) (env.parameter id)
  layout : ∀ q key, conforms target usize (.integer (env.layout q key))

theorem evaluation_preserves_type_and_range (target : Target) (context : Nat → Ty)
    (env : Env) (valid : ValidEnv target context env)
    (typed : HasType target context expr ty) : conforms target ty (eval env expr) := by
  cases typed with
  | literal fits => exact fits
  | parameter id => exact valid.parameter id
  | layout q key => exact valid.layout q key

structure Replacement where
  parameter : Nat → Expr
  layout : Query → Nat → Expr

def replace (s : Replacement) : Expr → Expr
  | .literal ty value => .literal ty value
  | .parameter id => s.parameter id
  | .layout q key => s.layout q key

structure TypedReplacement (target : Target) (source destination : Nat → Ty)
    (s : Replacement) : Prop where
  parameter : ∀ id, HasType target destination (s.parameter id) (source id)
  layout : ∀ q key, HasType target destination (s.layout q key) usize

theorem substitution_preserves_type (target : Target) (source destination : Nat → Ty)
    (s : Replacement) (valid : TypedReplacement target source destination s)
    (typed : HasType target source expr ty) :
    HasType target destination (replace s expr) ty := by
  cases typed with
  | literal fits => exact .literal fits
  | parameter id => exact valid.parameter id
  | layout q key => exact valid.layout q key

theorem substitution_preserves_type_and_range (target : Target)
    (source destination : Nat → Ty) (s : Replacement)
    (replacement : TypedReplacement target source destination s)
    (env : Env) (valid : ValidEnv target destination env)
    (typed : HasType target source expr ty) :
    conforms target ty (eval env (replace s expr)) := by
  exact evaluation_preserves_type_and_range target destination env valid
    (substitution_preserves_type target source destination s replacement typed)

def target32 : Target := ⟨32, by decide, by decide⟩
def target64 : Target := ⟨64, by decide, by decide⟩

-- A substitution proved valid for one target cannot be reused at another.
example : validate target32 usize (.integer 4294967295) = true := by decide
example : validate target32 usize (.integer 4294967296) = false := by decide
example : validate target64 usize (.integer 4294967296) = true := by decide
example : validate target32 isize (.integer (-2147483648)) = true := by decide
example : validate target32 isize (.integer 2147483648) = false := by decide
example : validate target64 isize (.integer 2147483648) = true := by decide
example : validate target64 (.integer false (.fixed 128))
    (.integer 340282366920938463463374607431768211455) = true := by decide
example : validate target64 (.integer true (.fixed 128))
    (.integer (-170141183460469231731687303715884105728)) = true := by decide

end Scalar

def manifest : Lean.Json :=
  match Lean.Json.parse (include_str "symbolic_const_matrix.json") with
  | .ok json => json
  | .error _ => .null

def manifestNames (key : String) : List String :=
  (manifest.getObjValAs? (List String) key).toOption.getD []

example : manifestNames "compiler_cases" = [
    "global_const_argument", "local_const_argument", "parameter_alias_chain",
    "layout_alias_chain", "layout_argument", "const_function_instances",
    "shadowed_parameter", "wrong_parameter_type", "runtime_argument",
    "symbol_literal_mismatch", "generic_arithmetic"
  ] := by native_decide

example : manifestNames "semantic_laws" = [
    "symbolic_equality_is_sound", "literal_ne_parameter",
    "substitution_preserves_meaning", "equality_survives_instantiation",
    "length_argument_roundtrip", "argument_length_roundtrip",
    "classification_preserves_meaning", "runtime_binding_is_rejected",
    "Scalar.validation_is_exact", "Scalar.usize_uses_artifact_width",
    "Scalar.isize_uses_artifact_width", "Scalar.integer_cannot_be_bool",
    "Scalar.evaluation_preserves_type_and_range", "Scalar.substitution_preserves_type",
    "Scalar.substitution_preserves_type_and_range"
  ] := by native_decide

example : manifestNames "typed_compiler_cases" = [
    "bool_value", "integer_is_not_bool", "unsigned_negative", "u8_max", "u8_overflow",
    "i8_min", "i8_underflow", "i8_overflow", "u128_max", "u128_overflow",
    "i128_min", "i128_underflow", "i128_overflow", "usize32_max", "usize32_overflow",
    "usize64_wider", "usize64_max", "usize64_overflow", "isize32_min", "isize32_max",
    "isize32_underflow", "isize32_overflow", "isize64_wider", "isize64_min",
    "isize64_max", "isize64_underflow", "isize64_overflow"
  ] := by native_decide

end NiaSymbolicConst
