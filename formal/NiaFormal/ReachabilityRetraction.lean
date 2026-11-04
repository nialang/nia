import Lean

namespace NiaFormal.ReachabilityRetraction

abbrev Edge := Prod Nat Nat

inductive Path (edges : List Edge) : Nat → Nat → Prop where
  | refl (node : Nat) : Path edges node node
  | step {source middle target : Nat}
      (prior : Path edges source middle)
      (edge : (middle, target) ∈ edges) :
      Path edges source target

def Reachable (edges : List Edge) (roots : List Nat) (node : Nat) : Prop :=
  ∃ root, root ∈ roots ∧ Path edges root node

def Affected
    (oldEdges newEdges : List Edge) (changedSources : List Nat) (node : Nat) : Prop :=
  ∃ source, source ∈ changedSources ∧ Path (oldEdges ++ newEdges) source node

def Retained
    (oldEdges newEdges : List Edge) (roots changedSources : List Nat) (node : Nat) : Prop :=
  Reachable oldEdges roots node ∧ ¬ Affected oldEdges newEdges changedSources node

def RederivationSeed
    (oldEdges newEdges : List Edge) (roots changedSources : List Nat) (node : Nat) : Prop :=
  Affected oldEdges newEdges changedSources node ∧
    (node ∈ roots ∨ ∃ parent,
      Retained oldEdges newEdges roots changedSources parent ∧ (parent, node) ∈ newEdges)

def Rederived
    (oldEdges newEdges : List Edge) (roots changedSources : List Nat) (node : Nat) : Prop :=
  ∃ seed, RederivationSeed oldEdges newEdges roots changedSources seed ∧ Path newEdges seed node

def DRedResult
    (oldEdges newEdges : List Edge) (roots changedSources : List Nat) (node : Nat) : Prop :=
  Retained oldEdges newEdges roots changedSources node ∨
    Rederived oldEdges newEdges roots changedSources node

theorem path_trans {edges : List Edge} {source middle target : Nat}
    (first : Path edges source middle) (second : Path edges middle target) :
    Path edges source target := by
    induction second with
  | refl => exact first
  | step prior edge ih => exact .step ih edge

theorem affected_extends {oldEdges newEdges : List Edge} {changedSources : List Nat}
    {source target : Nat}
    (affected : Affected oldEdges newEdges changedSources source)
    (path : Path (oldEdges ++ newEdges) source target) :
    Affected oldEdges newEdges changedSources target := by
  rcases affected with ⟨origin, originChanged, prior⟩
  exact ⟨origin, originChanged, path_trans prior path⟩

theorem affected_contains_changed_source
    (oldEdges newEdges : List Edge) (changedSources : List Nat) (source : Nat)
    (changed : source ∈ changedSources) :
    Affected oldEdges newEdges changedSources source := by
  exact ⟨source, changed, Path.refl source⟩

theorem path_transfer_outside_affected
    {oldEdges newEdges : List Edge} {changedSources : List Nat}
    {source target : Nat}
    (sameOutgoing : ∀ sourceNode targetNode, sourceNode ∉ changedSources →
      ((sourceNode, targetNode) ∈ oldEdges ↔ (sourceNode, targetNode) ∈ newEdges))
    (path : Path oldEdges source target)
    (outside : ¬ Affected oldEdges newEdges changedSources target) :
    Path newEdges source target := by
  induction path with
  | refl => exact .refl source
  | @step middle target prior edge ih =>
      have middleOutside : ¬ Affected oldEdges newEdges changedSources middle := by
        intro middleAffected
        apply outside (affected_extends middleAffected ?_)
        exact .step (.refl middle) (List.mem_append_left _ edge)
      have middleUnchanged : middle ∉ changedSources := by
        intro changed
        exact middleOutside
          (affected_contains_changed_source oldEdges newEdges changedSources middle changed)
      exact .step (ih middleOutside) ((sameOutgoing middle target middleUnchanged).mp edge)

theorem path_transfer_back_outside_affected
    {oldEdges newEdges : List Edge} {changedSources : List Nat}
    {source target : Nat}
    (sameOutgoing : ∀ sourceNode targetNode, sourceNode ∉ changedSources →
      ((sourceNode, targetNode) ∈ oldEdges ↔ (sourceNode, targetNode) ∈ newEdges))
    (path : Path newEdges source target)
    (outside : ¬ Affected oldEdges newEdges changedSources target) :
    Path oldEdges source target := by
  induction path with
  | refl => exact .refl source
  | @step middle target prior edge ih =>
      have middleOutside : ¬ Affected oldEdges newEdges changedSources middle := by
        intro middleAffected
        apply outside (affected_extends middleAffected ?_)
        exact .step (.refl middle) (List.mem_append_right _ edge)
      have middleUnchanged : middle ∉ changedSources := by
        intro changed
        exact middleOutside
          (affected_contains_changed_source oldEdges newEdges changedSources middle changed)
      exact .step (ih middleOutside) ((sameOutgoing middle target middleUnchanged).mpr edge)

theorem reachable_path_has_dred
    {oldEdges newEdges : List Edge} {roots changedSources : List Nat}
    (sameOutgoing : ∀ sourceNode targetNode, sourceNode ∉ changedSources →
      ((sourceNode, targetNode) ∈ oldEdges ↔ (sourceNode, targetNode) ∈ newEdges))
    {root node : Nat}
    (rootIn : root ∈ roots)
    (path : Path newEdges root node) :
    DRedResult oldEdges newEdges roots changedSources node := by
  induction path with
  | refl =>
      by_cases affected : Affected oldEdges newEdges changedSources root
      · exact Or.inr ⟨root, ⟨affected, Or.inl rootIn⟩, .refl root⟩
      · exact Or.inl ⟨⟨root, rootIn, .refl root⟩, affected⟩
  | @step middle target prior edge ih =>
      by_cases targetAffected : Affected oldEdges newEdges changedSources target
      · rcases ih with retained | rederived
        · rcases retained with ⟨middleReachable, middleOutside⟩
          exact Or.inr ⟨target,
            ⟨targetAffected, Or.inr ⟨middle, ⟨middleReachable, middleOutside⟩, edge⟩⟩,
            .refl target⟩
        · rcases rederived with ⟨seed, seedFacts, seedPath⟩
          exact Or.inr ⟨seed, seedFacts, .step seedPath edge⟩
      · have oldPath :=
            path_transfer_back_outside_affected sameOutgoing (.step prior edge) targetAffected
        exact Or.inl ⟨⟨root, rootIn, oldPath⟩, targetAffected⟩

theorem dred_exact
    {oldEdges newEdges : List Edge} {roots changedSources : List Nat}
    (sameOutgoing : ∀ sourceNode targetNode, sourceNode ∉ changedSources →
      ((sourceNode, targetNode) ∈ oldEdges ↔ (sourceNode, targetNode) ∈ newEdges))
    (node : Nat) :
    DRedResult oldEdges newEdges roots changedSources node ↔
      Reachable newEdges roots node := by
  constructor
  · intro result
    rcases result with ⟨oldReachable, outside⟩ | ⟨seed, seedFacts, path⟩
    · rcases oldReachable with ⟨root, rootIn, oldPath⟩
      exact ⟨root, rootIn,
        path_transfer_outside_affected sameOutgoing oldPath outside⟩
    · rcases seedFacts with ⟨seedAffected, rootIn | ⟨parent, parentRetained, edge⟩⟩
      · exact ⟨seed, rootIn, path⟩
      · rcases parentRetained with ⟨parentReachable, parentOutside⟩
        rcases parentReachable with ⟨root, rootIn, oldPath⟩
        have rootToParent := path_transfer_outside_affected sameOutgoing oldPath parentOutside
        have rootToSeed := path_trans rootToParent (.step (.refl parent) edge)
        exact ⟨root, rootIn, path_trans rootToSeed path⟩
  · intro reachable
    rcases reachable with ⟨root, rootIn, path⟩
    exact reachable_path_has_dred sameOutgoing rootIn path

end NiaFormal.ReachabilityRetraction
