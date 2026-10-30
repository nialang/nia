namespace NiaTraitProvider

abbrev Goal := Nat
abbrev Edge := Goal → Goal → Prop
abbrev Facts := Goal → Prop

def union (left right : Facts) : Facts := fun g => left g ∨ right g

/- A provider fact `q` may require another fact `g`. The outer query is kept
   separate from this graph: only selected impl prerequisites are edges. -/
def expand (edge : Edge) (facts : Facts) : Facts :=
  union facts (fun g => ∃ q, facts q ∧ q ≠ g ∧ edge q g)

theorem expand_subset (edge : Edge) (facts : Facts) : ∀ g, facts g → expand edge facts g := by
  intro g hg
  exact Or.inl hg

theorem expand_mono (edge : Edge) {left right : Facts}
    (h : ∀ g, left g → right g) : ∀ g, expand edge left g → expand edge right g := by
  intro g hg
  rcases hg with hg | ⟨q, hq, hneq, hedge⟩
  · exact Or.inl (h g hg)
  · exact Or.inr ⟨q, h q hq, hneq, hedge⟩

/- A strict prerequisite edge cannot manufacture the same goal through a
   self-edge. The explicit inequality is the cycle guard used by the solver. -/
theorem strict_edge_requires_distinct_prerequisite (edge : Edge) (facts : Facts) (q : Goal)
    (hq : expand edge facts q) : facts q ∨ ∃ r, facts r ∧ r ≠ q ∧ edge r q := by
  rcases hq with hq | ⟨r, hr, hneq, hedge⟩
  · exact Or.inl hq
  · exact Or.inr ⟨r, hr, hneq, hedge⟩

end NiaTraitProvider
