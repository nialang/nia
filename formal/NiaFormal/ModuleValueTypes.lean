import Lean

namespace NiaModuleValueTypes

/-
The body checker infers module-level global and const types by repeatedly
checking initializers against the facts discovered so far. This finite model
proves the bound used by that worklist: accepted facts only accumulate, and a
strict round must add a declaration that was previously unknown.

The model covers the per-module fact loop. Cross-module query-cycle handling is
covered separately by compiler integration tests.
-/

inductive DeclarationKind where
  | global
  | const
  deriving DecidableEq, Repr

def DeclarationKind.name : DeclarationKind → String
  | .global => "global"
  | .const => "const"

def declarationKinds : List DeclarationKind := [.global, .const]

abbrev Facts (Ty : Type) := List (Option Ty)

def knownCount {Ty : Type} : Facts Ty → Nat
  | [] => 0
  | none :: rest => knownCount rest
  | some _ :: rest => knownCount rest + 1

def Extends {Ty : Type} : Facts Ty → Facts Ty → Prop
  | [], [] => True
  | none :: before, _ :: after => Extends before after
  | some ty :: before, some next :: after => ty = next ∧ Extends before after
  | _, _ => False

def StrictGrowth {Ty : Type} (before after : Facts Ty) : Prop :=
  Extends before after ∧ knownCount before < knownCount after

def merge {Ty : Type} : Facts Ty → Facts Ty → Facts Ty
  | [], _ => []
  | established :: before, [] => established :: merge before []
  | some ty :: before, _ :: discovered => some ty :: merge before discovered
  | none :: before, next :: discovered => next :: merge before discovered

theorem merge_extends_established {Ty : Type}
    (established discovered : Facts Ty) :
    Extends established (merge established discovered) := by
  induction established generalizing discovered with
  | nil => simp [Extends, merge]
  | cons entry rest ih =>
      cases discovered with
      | nil =>
          cases entry <;> simp [Extends, merge] <;> exact ih []
      | cons next tail =>
          cases entry with
          | none => simp [Extends, merge, ih]
          | some ty =>
              cases next with
              | none => simp [Extends, merge, ih]
              | some other => simp [Extends, merge, ih]

theorem merge_preserves_fact_slots {Ty : Type}
    (established discovered : Facts Ty) :
    (merge established discovered).length = established.length := by
  induction established generalizing discovered with
  | nil => simp [merge]
  | cons entry rest ih =>
      cases discovered with
      | nil => simp [merge, ih]
      | cons next tail => cases entry <;> simp [merge, ih]

theorem known_count_le_length {Ty : Type} (facts : Facts Ty) :
    knownCount facts ≤ facts.length := by
  induction facts with
  | nil => simp [knownCount]
  | cons fact rest ih =>
      cases fact <;> simp [knownCount] <;> omega

theorem strict_rounds_are_bounded {Ty : Type}
    (states : Nat → Facts Ty) (rounds : Nat)
    (grows : ∀ round, round < rounds →
      StrictGrowth (states round) (states (round + 1))) :
    rounds + knownCount (states 0) ≤ knownCount (states rounds) := by
  induction rounds with
  | zero => simp
  | succ rounds ih =>
      have previous := ih (fun round beforeEnd => grows round (by omega))
      have step := (grows rounds (by omega)).2
      omega

theorem no_more_than_declarations_of_strict_rounds {Ty : Type}
    (count : Nat) (states : Nat → Facts Ty)
    (slots : ∀ round, (states round).length = count)
    (grows : ∀ round, round < count + 1 →
      StrictGrowth (states round) (states (round + 1))) : False := by
  have growth := strict_rounds_are_bounded states (count + 1) grows
  have bound := known_count_le_length (states (count + 1))
  rw [slots (count + 1)] at bound
  omega

def manifest : Lean.Json :=
  match Lean.Json.parse (include_str "module_value_types_matrix.json") with
  | .ok json => json
  | .error _ => .null

def manifestNames (key : String) : List String :=
  ((manifest.getObjValAs? (List String) key).toOption.getD [])

example : manifestNames "declaration_kinds" = declarationKinds.map DeclarationKind.name := by
  native_decide

example : manifestNames "dependency_cases" = [
    "forward_const_used_by_static",
    "transitive_const_chain",
    "same_type_fact_for_const_and_global",
    "cross_module_forward_const",
    "cross_module_const_dependency_change",
    "cross_module_const_cycle",
    "incremental_const_dependency_change",
    "const_cycle_source_diagnostic",
    "forward_global_dependency"
  ] := by native_decide

end NiaModuleValueTypes
