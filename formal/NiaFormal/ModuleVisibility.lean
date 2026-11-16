import Lean

namespace NiaFormal.ModuleVisibility

/- The model keeps source extension visibility and trait witnesses separate.
   A module sees the inherent extension methods of the modules its own
   imports reach: the modules it imports, what they re-export, the providers
   that the facade of an imported item publishes, and what the root facade of
   an imported package publishes. A provider is published only by `pub using`:
   private imports, provider demands (its own or an imported module's),
   nominal type owners, and trait witnesses add no source extension provider.
   The loader answers a method demand by searching exactly the providers this
   frame publishes, so the demand-selected providers are a subset of the
   visible ones and visibility never depends on which demands were made. -/

structure Input where
  root : Nat
  directImports : List Nat
  /-- Modules an item (a type, function, ...) is imported from. -/
  itemImportFacades : List Nat
  publicReexports : List (Prod Nat Nat)
  publicProviderReexports : List (Prod Nat Nat)
  packageRootImports : List (Prod Nat Nat)
  packageFacadeProviderReexports : List (Prod Nat Nat)
  privateUsings : List (Prod Nat Nat)
  typeImports : List Nat
  providerDemands : List Nat
  importedProviderDemands : List (Prod Nat Nat)
  traitImplProviders : List (Prod Nat Nat)
  deriving Repr

/-- The part of an input that extension visibility reads: the module, its
    imports, and the publications reachable from them. -/
structure ImportFrame where
  root : Nat
  directImports : List Nat
  itemImportFacades : List Nat
  publicReexports : List (Prod Nat Nat)
  publicProviderReexports : List (Prod Nat Nat)
  packageRootImports : List (Prod Nat Nat)
  packageFacadeProviderReexports : List (Prod Nat Nat)

def Input.frame (input : Input) : ImportFrame :=
  { root := input.root
    directImports := input.directImports
    itemImportFacades := input.itemImportFacades
    publicReexports := input.publicReexports
    publicProviderReexports := input.publicProviderReexports
    packageRootImports := input.packageRootImports
    packageFacadeProviderReexports := input.packageFacadeProviderReexports }

inductive ReexportReachable
    (directImports : List Nat) (publicReexports : List (Prod Nat Nat)) : Nat -> Prop where
  | direct {module : Nat} (h : List.Mem module directImports) :
      ReexportReachable directImports publicReexports module
  | reexport {parent module : Nat}
      (reachable : ReexportReachable directImports publicReexports parent)
      (edge : List.Mem (parent, module) publicReexports) :
      ReexportReachable directImports publicReexports module

/-- A provider published along the frame: a `pub using` chain that starts
    at an imported module or at the facade of an imported item. -/
inductive PublishedProvider (frame : ImportFrame) : Nat -> Prop where
  | fromModule {facade provider : Nat}
      (facadeReached : ReexportReachable frame.directImports frame.publicReexports facade)
      (edge : List.Mem (facade, provider) frame.publicProviderReexports) :
      PublishedProvider frame provider
  | fromItem {facade provider : Nat}
      (h : List.Mem facade frame.itemImportFacades)
      (edge : List.Mem (facade, provider) frame.publicProviderReexports) :
      PublishedProvider frame provider
  | chain {parent provider : Nat}
      (reached : PublishedProvider frame parent)
      (edge : List.Mem (parent, provider) frame.publicProviderReexports) :
      PublishedProvider frame provider

def frameVisible (frame : ImportFrame) (module : Nat) : Prop :=
  module = frame.root \/
    ReexportReachable frame.directImports frame.publicReexports module \/
    PublishedProvider frame module \/
    (∃ imported root,
      List.Mem imported frame.directImports ∧
      List.Mem (imported, root) frame.packageRootImports ∧
      List.Mem (root, module) frame.packageFacadeProviderReexports)

def extensionModuleVisible (input : Input) (module : Nat) : Prop :=
  frameVisible input.frame module

/-- The loader's answer to the module's method demands is sound when every
    provider it selects is published along the module's own frame. -/
def demandsSound (input : Input) : Prop :=
  ∀ provider, List.Mem provider input.providerDemands → PublishedProvider input.frame provider

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
  | itemImportFacadeProviders
  | packageFacadeProviderReexport
  | soundProviderDemandsAddNothing
  | traitProviderClosure
  | privateUsingIsLocal
  | providerDemandsAreModuleLocal
  | typeImportDoesNotImportExtensionProvider
  | separateExtensionAndTraitClosures
  | visibilityDependsOnImportsAlone
  | importsOnlyAddVisibility
  deriving DecidableEq, Repr

def VisibilityRule.name : VisibilityRule -> String
  | .rootModule => "root_module"
  | .directUsing => "direct_using"
  | .publicReexport => "public_reexport"
  | .publicProviderReexport => "public_provider_reexport"
  | .itemImportFacadeProviders => "item_import_facade_providers"
  | .packageFacadeProviderReexport => "package_facade_provider_reexport"
  | .soundProviderDemandsAddNothing => "sound_provider_demands_add_nothing"
  | .traitProviderClosure => "trait_provider_closure"
  | .privateUsingIsLocal => "private_using_is_local"
  | .providerDemandsAreModuleLocal => "provider_demands_are_module_local"
  | .typeImportDoesNotImportExtensionProvider => "type_import_does_not_import_extension_provider"
  | .separateExtensionAndTraitClosures => "separate_extension_and_trait_closures"
  | .visibilityDependsOnImportsAlone => "visibility_depends_on_imports_alone"
  | .importsOnlyAddVisibility => "imports_only_add_visibility"

def allVisibilityRules : List VisibilityRule := [
  .rootModule, .directUsing, .publicReexport, .publicProviderReexport,
  .itemImportFacadeProviders, .packageFacadeProviderReexport,
  .soundProviderDemandsAddNothing, .traitProviderClosure, .privateUsingIsLocal,
  .providerDemandsAreModuleLocal,
  .typeImportDoesNotImportExtensionProvider, .separateExtensionAndTraitClosures,
  .visibilityDependsOnImportsAlone, .importsOnlyAddVisibility
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

theorem direct_public_provider_reexport_is_visible (input : Input)
    (parent provider : Nat)
    (hparent : List.Mem parent input.directImports)
    (hedge : List.Mem (parent, provider) input.publicProviderReexports) :
    extensionModuleVisible input provider := by
  exact Or.inr (Or.inr (Or.inl (.fromModule (.direct hparent) hedge)))

/-- Importing a type from a facade sees the providers the facade publishes. -/
theorem item_import_facade_provider_is_visible (input : Input)
    (facade provider : Nat)
    (hfacade : List.Mem facade input.itemImportFacades)
    (hedge : List.Mem (facade, provider) input.publicProviderReexports) :
    extensionModuleVisible input provider := by
  exact Or.inr (Or.inr (Or.inl (.fromItem hfacade hedge)))

theorem package_facade_provider_reexport_is_visible (input : Input)
    (imported root provider : Nat)
    (himport : List.Mem imported input.directImports)
    (hroot : List.Mem (imported, root) input.packageRootImports)
    (hedge : List.Mem (root, provider) input.packageFacadeProviderReexports) :
    extensionModuleVisible input provider := by
  exact Or.inr (Or.inr (Or.inr ⟨imported, root, himport, hroot, hedge⟩))

/-- Extension visibility reads the import frame and nothing else: two inputs
    with the same frame see the same modules, whatever their private imports,
    provider demands, type owners, or trait witnesses. -/
theorem visibility_depends_on_imports_alone (input other : Input)
    (h : input.frame = other.frame) (module : Nat) :
    extensionModuleVisible input module ↔ extensionModuleVisible other module := by
  unfold extensionModuleVisible
  rw [h]

theorem private_using_edges_do_not_change_extension_visibility
    (input : Input) (privateUsings : List (Prod Nat Nat)) (module : Nat) :
    extensionModuleVisible input module ↔
      extensionModuleVisible { input with privateUsings := privateUsings } module :=
  visibility_depends_on_imports_alone _ _ rfl module

theorem imported_provider_demands_do_not_change_extension_visibility
    (input : Input) (providerDemands : List (Prod Nat Nat)) (module : Nat) :
    extensionModuleVisible input module ↔
      extensionModuleVisible { input with importedProviderDemands := providerDemands } module :=
  visibility_depends_on_imports_alone _ _ rfl module

theorem type_imports_do_not_change_extension_visibility
    (input : Input) (typeImports : List Nat) (module : Nat) :
    extensionModuleVisible input module ↔
      extensionModuleVisible { input with typeImports := typeImports } module :=
  visibility_depends_on_imports_alone _ _ rfl module

theorem trait_provider_edges_do_not_change_extension_visibility
    (input : Input) (traitImplProviders : List (Prod Nat Nat)) (module : Nat) :
    extensionModuleVisible input module ↔
      extensionModuleVisible { input with traitImplProviders := traitImplProviders } module :=
  visibility_depends_on_imports_alone _ _ rfl module

/-- The compiler makes the demand-selected providers visible as well. When
    the loader is sound, that adds nothing: every such provider is already
    visible through the module's own imports. -/
theorem sound_provider_demands_add_nothing (input : Input) (sound : demandsSound input)
    (module : Nat) :
    (extensionModuleVisible input module ∨ List.Mem module input.providerDemands) ↔
      extensionModuleVisible input module := by
  constructor
  · rintro (h | h)
    · exact h
    · exact Or.inr (Or.inr (Or.inl (sound module h)))
  · exact Or.inl

theorem nothing_is_reachable_without_imports
    (reexports : List (Prod Nat Nat)) (module : Nat) :
    ¬ ReexportReachable [] reexports module := by
  intro h
  induction h with
  | direct h => cases h
  | reexport _ _ ih => exact ih

theorem nothing_is_published_without_imports (frame : ImportFrame)
    (hmodules : frame.directImports = []) (hitems : frame.itemImportFacades = [])
    (module : Nat) :
    ¬ PublishedProvider frame module := by
  intro h
  induction h with
  | fromModule reached _ =>
    rw [hmodules] at reached
    exact nothing_is_reachable_without_imports _ _ reached
  | fromItem h _ =>
    rw [hitems] at h
    cases h
  | chain _ _ ih => exact ih

/-- A module that imports nothing sees no other module, so no sound demand
    can select a provider for it: a private `using` elsewhere publishes
    nothing it could reach. -/
theorem no_imports_no_visible_provider (input : Input)
    (hmodules : input.directImports = []) (hitems : input.itemImportFacades = [])
    (module : Nat) (hdistinct : module ≠ input.root) :
    ¬ extensionModuleVisible input module := by
  intro h
  rcases h with h | h | h | ⟨_, _, himport, _, _⟩
  · exact hdistinct h
  · rw [show input.frame.directImports = [] from hmodules] at h
    exact nothing_is_reachable_without_imports _ _ h
  · exact nothing_is_published_without_imports input.frame hmodules hitems module h
  · rw [show input.frame.directImports = [] from hmodules] at himport
    cases himport

theorem reexport_reachable_mono {imports imports' : List Nat}
    {edges : List (Prod Nat Nat)}
    (himports : ∀ module, List.Mem module imports → List.Mem module imports')
    {module : Nat} (h : ReexportReachable imports edges module) :
    ReexportReachable imports' edges module := by
  induction h with
  | direct h => exact .direct (himports _ h)
  | reexport _ edge ih => exact .reexport ih edge

theorem published_provider_mono (input : Input) (extra : List Nat) {module : Nat}
    (h : PublishedProvider input.frame module) :
    PublishedProvider { input with directImports := input.directImports ++ extra }.frame module := by
  have more : ∀ m, List.Mem m input.directImports → List.Mem m (input.directImports ++ extra) :=
    fun _ hm => List.mem_append_left _ hm
  induction h with
  | fromModule reached edge => exact .fromModule (reexport_reachable_mono more reached) edge
  | fromItem h edge => exact .fromItem h edge
  | chain _ edge ih => exact .chain ih edge

/-- Importing more never hides a module that was visible. -/
theorem imports_only_add_visibility (input : Input) (extra : List Nat) (module : Nat)
    (h : extensionModuleVisible input module) :
    extensionModuleVisible { input with directImports := input.directImports ++ extra } module := by
  have more : ∀ m, List.Mem m input.directImports → List.Mem m (input.directImports ++ extra) :=
    fun _ hm => List.mem_append_left _ hm
  rcases h with h | h | h | ⟨imported, root, himport, hroot, hfacade⟩
  · exact Or.inl h
  · exact Or.inr (Or.inl (reexport_reachable_mono more h))
  · exact Or.inr (Or.inr (Or.inl (published_provider_mono input extra h)))
  · exact Or.inr (Or.inr (Or.inr ⟨imported, root, more _ himport, hroot, hfacade⟩))

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
