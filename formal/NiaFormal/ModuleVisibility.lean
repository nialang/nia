import Lean

namespace NiaFormal.ModuleVisibility

/- The model keeps source extension visibility and trait witnesses separate.
   Imported modules' private imports/provider demands, and nominal type imports,
   do not add source extension providers. -/

structure Input where
  root : Nat
  directImports : List Nat
  publicReexports : List (Prod Nat Nat)
  publicProviderReexports : List (Prod Nat Nat)
  privateUsings : List (Prod Nat Nat)
  typeImports : List Nat
  directExtensionProviders : List Nat
  importedProviderDemands : List (Prod Nat Nat)
  traitImplProviders : List (Prod Nat Nat)
  deriving Repr

inductive ReexportReachable
    (directImports : List Nat) (publicReexports : List (Prod Nat Nat)) : Nat -> Prop where
  | direct {module : Nat} (h : List.Mem module directImports) :
      ReexportReachable directImports publicReexports module
  | reexport {parent module : Nat}
      (reachable : ReexportReachable directImports publicReexports parent)
      (edge : List.Mem (parent, module) publicReexports) :
      ReexportReachable directImports publicReexports module

def extensionModuleVisible (input : Input) (module : Nat) : Prop :=
  module = input.root \/
    ReexportReachable input.directImports input.publicReexports module \/
    ReexportReachable input.directImports input.publicProviderReexports module \/
    List.Mem module input.directExtensionProviders

inductive TraitWitnessReachable (edges : List (Prod Nat Nat)) : Nat -> Nat -> Prop where
  | direct {source target : Nat} (edge : List.Mem (source, target) edges) :
      TraitWitnessReachable edges source target
  | transitive {source intermediate target : Nat}
      (first : TraitWitnessReachable edges source intermediate)
      (second : TraitWitnessReachable edges intermediate target) :
      TraitWitnessReachable edges source target

def traitWitnessVisible (input : Input) (module : Nat) : Prop :=
  module = input.root \/ TraitWitnessReachable input.traitImplProviders input.root module

inductive VisibilityRule where
  | rootModule
  | directUsing
  | publicReexport
  | publicProviderReexport
  | directProviderDemand
  | traitProviderClosure
  | privateUsingIsLocal
  | providerDemandsAreModuleLocal
  | typeImportDoesNotImportExtensionProvider
  | separateExtensionAndTraitClosures
  deriving DecidableEq, Repr

def VisibilityRule.name : VisibilityRule -> String
  | .rootModule => "root_module"
  | .directUsing => "direct_using"
  | .publicReexport => "public_reexport"
  | .publicProviderReexport => "public_provider_reexport"
  | .directProviderDemand => "direct_provider_demand"
  | .traitProviderClosure => "trait_provider_closure"
  | .privateUsingIsLocal => "private_using_is_local"
  | .providerDemandsAreModuleLocal => "provider_demands_are_module_local"
  | .typeImportDoesNotImportExtensionProvider => "type_import_does_not_import_extension_provider"
  | .separateExtensionAndTraitClosures => "separate_extension_and_trait_closures"

def allVisibilityRules : List VisibilityRule := [
  .rootModule, .directUsing, .publicReexport, .publicProviderReexport,
  .directProviderDemand, .traitProviderClosure, .privateUsingIsLocal,
  .providerDemandsAreModuleLocal,
  .typeImportDoesNotImportExtensionProvider, .separateExtensionAndTraitClosures
]

def visibilityRuleNames : List String := allVisibilityRules.map VisibilityRule.name

def visibilityMatrix : Lean.Json :=
  match Lean.Json.parse (include_str "visibility_matrix.json") with
  | .ok json => json
  | .error _ => .null

def manifestNames (key : String) : List String :=
  ((visibilityMatrix.getObjValAs? (List String) key).toOption.getD [])

example : manifestNames "rules" = visibilityRuleNames := by native_decide

theorem root_module_is_visible (input : Input) :
    extensionModuleVisible input input.root := by exact Or.inl rfl

theorem direct_import_is_visible (input : Input) (module : Nat)
    (h : List.Mem module input.directImports) :
    extensionModuleVisible input module := by exact Or.inr (Or.inl (.direct h))

theorem public_reexport_chain_is_visible (input : Input)
    (first second third : Nat)
    (hfirst : List.Mem first input.directImports)
    (hsecond : List.Mem (first, second) input.publicReexports)
    (hthird : List.Mem (second, third) input.publicReexports) :
    extensionModuleVisible input third := by
  exact Or.inr (Or.inl (.reexport (.reexport (.direct hfirst) hsecond) hthird))

theorem direct_provider_demand_is_visible (input : Input) (module : Nat)
    (h : List.Mem module input.directExtensionProviders) :
    extensionModuleVisible input module := by exact Or.inr (Or.inr (Or.inr h))

theorem direct_public_provider_reexport_is_visible (input : Input)
    (parent provider : Nat)
    (hparent : List.Mem parent input.directImports)
    (hedge : List.Mem (parent, provider) input.publicProviderReexports) :
    extensionModuleVisible input provider := by
  exact Or.inr (Or.inr (Or.inl (.reexport (.direct hparent) hedge)))

theorem private_using_edges_do_not_change_extension_visibility
    (input : Input) (privateUsings : List (Prod Nat Nat)) (module : Nat) :
    extensionModuleVisible input module =
      extensionModuleVisible { input with privateUsings := privateUsings } module := by rfl

theorem imported_provider_demands_do_not_change_extension_visibility
    (input : Input) (providerDemands : List (Prod Nat Nat)) (module : Nat) :
    extensionModuleVisible input module =
      extensionModuleVisible { input with importedProviderDemands := providerDemands } module := by rfl

theorem type_imports_do_not_change_extension_visibility
    (input : Input) (typeImports : List Nat) (module : Nat) :
    extensionModuleVisible input module =
      extensionModuleVisible { input with typeImports := typeImports } module := by rfl

theorem trait_provider_edges_do_not_change_extension_visibility
    (input : Input) (traitImplProviders : List (Prod Nat Nat)) (module : Nat) :
    extensionModuleVisible input module =
      extensionModuleVisible { input with traitImplProviders := traitImplProviders } module := by rfl

theorem direct_trait_provider_is_visible (input : Input) (module : Nat)
    (h : List.Mem (input.root, module) input.traitImplProviders) :
    traitWitnessVisible input module := by exact Or.inr (.direct h)

theorem transitive_trait_provider_is_visible (input : Input)
    (first second : Nat)
    (hfirst : List.Mem (input.root, first) input.traitImplProviders)
    (hsecond : List.Mem (first, second) input.traitImplProviders) :
    traitWitnessVisible input second := by
  exact Or.inr (.transitive (.direct hfirst) (.direct hsecond))

end NiaFormal.ModuleVisibility
