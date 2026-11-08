import Lean

namespace NiaFormal.ScopedExports

/- The model starts after host-path resolution. Host access permits selecting
   a member; it does not become that member's exported access restriction.
   Provider publication is guarded by its directive, independently of namespace
   publication. The caller grant predicate is an input, not proved here. -/

inductive Visibility where
  | localOnly | parentTree | packageTree | publicAll
  deriving DecidableEq, Repr

inductive DeclarationKind where
  | item | moduleName
  deriving DecidableEq, Repr

structure Restriction where
  owner : Nat
  visibility : Visibility
  kind : DeclarationKind
  deriving DecidableEq, Repr

-- The graph supplies ancestry/package identity; this does not verify that graph.
structure AccessContext where
  packageOf : Nat -> Nat
  parentOf : Nat -> Option Nat
  inTree : Nat -> Nat -> Prop

def grant (context : AccessContext) (caller : Nat) (restriction : Restriction) : Prop :=
  match restriction.visibility with
  | .publicAll => True
  | .packageTree => context.packageOf caller = context.packageOf restriction.owner
  | .parentTree => Or (context.inTree caller restriction.owner)
      (Exists fun parent => And (context.parentOf restriction.owner = some parent)
        (context.inTree caller parent))
  | .localOnly =>
      match restriction.kind with
      | .item => caller = restriction.owner
      | .moduleName => context.inTree caller restriction.owner

theorem private_item_requires_its_owner (context : AccessContext) (caller owner : Nat) :
    Iff (grant context caller (Restriction.mk owner .localOnly .item)) (caller = owner) :=
  Iff.rfl

theorem private_module_permits_descendants (context : AccessContext) (caller owner : Nat) :
    Iff (grant context caller (Restriction.mk owner .localOnly .moduleName))
      (context.inTree caller owner) := Iff.rfl

abbrev Route := List Restriction
abbrev Scope := List Route

def routeAllows (grant : Restriction -> Prop) (route : Route) : Prop :=
  forall restriction, restriction ∈ route -> grant restriction

def allows (grant : Restriction -> Prop) (scope : Scope) : Prop :=
  Exists fun route => And (route ∈ scope) (routeAllows grant route)

def restrict (scope : Scope) (restriction : Restriction) : Scope :=
  scope.map (fun route => restriction :: route)

def intersect (left right : Scope) : Scope :=
  left.flatMap fun l => right.map (fun r => l ++ r)

theorem union_routes (grant : Restriction -> Prop) (left right : Scope) :
    Iff (allows grant (left ++ right)) (Or (allows grant left) (allows grant right)) := by
  simp only [allows, List.mem_append]
  constructor
  case mp =>
    rintro ⟨ route, (hleft | hright), hgrant⟩
    case inl => exact Or.inl ⟨ route, hleft, hgrant⟩
    case inr => exact Or.inr ⟨ route, hright, hgrant⟩
  case mpr =>
    rintro (⟨ route, hmem, hgrant⟩ | ⟨ route, hmem, hgrant⟩)
    case inl => exact ⟨ route, Or.inl hmem, hgrant⟩
    case inr => exact ⟨ route, Or.inr hmem, hgrant⟩

theorem restriction_preserved (grant : Restriction -> Prop) (scope : Scope)
    (restriction : Restriction) :
    Iff (allows grant (restrict scope restriction))
      (And (grant restriction) (allows grant scope)) := by
  constructor
  case mp =>
    rintro ⟨ _, hmem, hgrant⟩
    obtain ⟨ route, hroute, rfl⟩ := List.mem_map.mp hmem
    exact ⟨ hgrant restriction (by simp),
      route, hroute, fun r hr => hgrant r (by simp [hr])⟩
  case mpr =>
    rintro ⟨ hrestriction, route, hmem, hgrant⟩
    refine ⟨ restriction :: route, List.mem_map.mpr ⟨ route, hmem, rfl⟩, ?_⟩
    intro r hr
    rcases List.mem_cons.mp hr with heq | hrest
    case inl => cases heq; exact hrestriction
    case inr => exact hgrant r hrest

theorem intersection_requires_both (grant : Restriction -> Prop) (left right : Scope) :
    Iff (allows grant (intersect left right))
      (And (allows grant left) (allows grant right)) := by
  simp only [allows, intersect, List.mem_flatMap, List.mem_map]
  constructor
  case mp =>
    rintro ⟨ _, ⟨ l, hl, r, hr, rfl⟩, hgrant⟩
    exact ⟨ ⟨ l, hl, fun x hx => hgrant x (List.mem_append_left _ hx)⟩,
      ⟨ r, hr, fun x hx => hgrant x (List.mem_append_right _ hx)⟩⟩
  case mpr =>
    rintro ⟨ ⟨ l, hl, hgl⟩, ⟨ r, hr, hgr⟩⟩
    exact ⟨ l ++ r, ⟨ l, hl, r, hr, rfl⟩,
      fun x hx => (List.mem_append.mp hx).elim (hgl x) (hgr x)⟩

theorem subsumed_route_is_redundant (grant : Restriction -> Prop)
    (broad narrow : Route)
    (contained : forall r, r ∈ broad -> r ∈ narrow) :
    routeAllows grant narrow -> routeAllows grant broad := by
  exact fun h r hr => h r (contained r hr)

theorem reexport_cannot_widen (grant : Restriction -> Prop) (scope : Scope)
    (restriction : Restriction) :
    allows grant (restrict scope restriction) -> allows grant scope := by
  exact fun h => ((restriction_preserved grant scope restriction).mp h).2

def enumVariantScope (scope : Scope) : Scope := scope

theorem enum_variant_preserves_scope (grant : Restriction -> Prop) (scope : Scope) :
    Iff (allows grant (enumVariantScope scope)) (allows grant scope) := Iff.rfl

def selectedMemberScope (_host : Scope) (member : Scope) : Scope := member
def providerPublicationScope (_implementationNamespace : Scope)
    (directive : Scope) : Scope := directive

theorem private_host_does_not_restrict_public_member
    (grant : Restriction -> Prop) (host member : Scope) :
    Iff (allows grant (selectedMemberScope host member)) (allows grant member) := Iff.rfl

theorem provider_publication_does_not_publish_namespace
    (grant : Restriction -> Prop) (implementation directive : Scope) :
    Iff (allows grant (providerPublicationScope implementation directive))
      (allows grant directive) := Iff.rfl

structure PublishedEdge where
  source : Nat
  target : Nat
  scope : Scope

inductive ProviderReachable (permission : Restriction -> Prop)
    (imports : List Nat) (edges : List PublishedEdge) : Nat -> Prop where
  | direct {target : Nat} (present : target ∈ imports) :
      ProviderReachable permission imports edges target
  | published {edge : PublishedEdge}
      (present : edge ∈ edges)
      (source : ProviderReachable permission imports edges edge.source)
      (permitted : allows permission edge.scope) :
      ProviderReachable permission imports edges edge.target

theorem no_provider_without_an_authorized_route (permission : Restriction -> Prop)
    (imports : List Nat) (edges : List PublishedEdge) (target : Nat)
    (notDirect : Not (target ∈ imports))
    (denied : forall edge, edge ∈ edges -> edge.target = target ->
      Not (allows permission edge.scope)) :
    Not (ProviderReachable permission imports edges target) := by
  intro reachable
  cases reachable with
  | direct present => exact notDirect present
  | published present _ permitted => exact denied _ present rfl permitted

def matrix : Lean.Json :=
  match Lean.Json.parse (include_str "export_scope_matrix.json") with
  | .ok value => value
  | .error _ => .null

def names (key : String) : List String :=
  ((matrix.getObjValAs? (List String) key).toOption.getD [])

example : names "rules" = [
  "alternate_routes", "restriction_preserved", "intersection_requires_both",
  "subsumed_route", "enum_variant_scope", "namespace_member_separation",
  "provider_publication_separation", "declaration_access_kind",
  "guarded_provider_reachability"] := by native_decide

example : names "compiler_cases" = [
  "restricted_reexport_facades_work_inside_the_package_without_leaking",
  "facade_public_items_and_providers_do_not_publish_private_namespaces",
  "restricted_provider_publication_stays_within_its_scope",
  "restricted_reexport_alias_chains_do_not_widen_access",
  "restricted_module_aliases_preserve_parent_and_package_boundaries"] := by native_decide

end NiaFormal.ScopedExports
