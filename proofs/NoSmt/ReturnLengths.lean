import NoSmt.PartialFacts
import NoSmt.ConditionalFacts
import NoSmt.Snapshots

set_option autoImplicit false

/-!
Return-length words are BitVec 256; Nat labels their expression/source identities.
The snapshot supplement reuses Snapshots' existing Nat-valued cell projection.
Non-ITE terms are opaque inputs. Evaluation is transparent, and Coherent explicitly
binds every modeled expression identity to that evaluation. In particular the
ITE equation is derived, not supplied as a bound-soundness premise.

Production must supply a sound NumericValue concretization and a same-path
bound for the exact current-frame returndata length. ExprId/source identity,
guard truth, immutable payload/root meaning and allocator freshness must realize
the displayed coherence and support conditions. No Rust simulation is assumed
as an axiom. Metadata import does not model a content-root rewrite.
-/
namespace NoSmt.ReturnLengths

abbrev Word := BitVec 256
abbrev Numeric := List Word
abbrev Facts := PartialFacts.FactMap Numeric
abbrev Scope := ConditionalFacts.Scope Numeric
abbrev Guard := ConditionalFacts.Guard

def member (bound : Numeric) (value : Word) : Prop := value ∈ bound

def wordTruth (value : Word) : Bool := value != 0

structure World where
  sources : Nat → Word
  values : Nat → Word

def env (world : World) (key : Nat) : Bool := wordTruth (world.values key)

inductive Expr where
  | input (key source : Nat)
  | ite (key : Nat) (guard : Guard) (left right : Expr)

def Expr.key : Expr → Nat
  | .input key _ => key
  | .ite key _ _ _ => key

def evaluate (world : World) : Expr → Word
  | .input _ source => world.sources source
  | .ite _ guard left right =>
    if ConditionalFacts.guardValue (env world) guard then evaluate world left
    else evaluate world right

/-- An identity binding, with no numeric-bound or path-entailment premise. -/
def Coherent (world : World) : Expr → Prop
  | .input key source => world.values key = world.sources source
  | expression@(.ite _ _ left right) =>
    world.values expression.key = evaluate world expression ∧
      Coherent world left ∧ Coherent world right

theorem evaluate_ite (world : World) (key : Nat) (guard : Guard) (left right : Expr) :
    evaluate world (.ite key guard left right) =
      if ConditionalFacts.guardValue (env world) guard then evaluate world left
      else evaluate world right := rfl

theorem coherent_value (world : World) (expression : Expr)
    (coherent : Coherent world expression) :
    world.values expression.key = evaluate world expression := by
  cases expression with
  | input key source => exact coherent
  | ite key guard left right => exact coherent.1

/-- The key-level ITE equation follows from recursive evaluation and binding. -/
theorem coherent_ite_equation (world : World) (key : Nat) (guard : Guard)
    (left right : Expr) (coherent : Coherent world (.ite key guard left right)) :
    world.values key =
      if ConditionalFacts.guardValue (env world) guard then world.values left.key
      else world.values right.key := by
  change world.values (Expr.ite key guard left right).key = _
  rw [coherent_value world _ coherent, evaluate_ite]
  rw [coherent_value world left coherent.2.1, coherent_value world right coherent.2.2]

def EntailsBound (path : World → Prop) (expression : Expr) (bound : Numeric) : Prop :=
  ∀ world, path world → member bound (evaluate world expression)

/-- Both implications use exactly the same path predicate, without selecting g. -/
theorem same_path_guarded_arms (path : World → Prop) (key : Nat) (guard : Guard)
    (left right : Expr) (bound : Numeric)
    (proved : EntailsBound path (.ite key guard left right) bound) :
    (∀ world, path world → ConditionalFacts.guardValue (env world) guard = true →
      member bound (evaluate world left)) ∧
    (∀ world, path world → ConditionalFacts.guardValue (env world) guard = false →
      member bound (evaluate world right)) := by
  constructor
  · intro world onPath selected
    simpa [evaluate, selected] using proved world onPath
  · intro world onPath selected
    simpa [evaluate, selected] using proved world onPath

theorem singleton_Gamma_iff (key : Nat) (bound : Numeric) (values : Nat → Word) :
    PartialFacts.Gamma member [(key, bound)] values ↔ member bound (values key) := by
  constructor
  · intro holds
    simpa [PartialFacts.accepts, PartialFacts.lookup] using holds key
  · intro holds query
    by_cases equal : query = key
    · simpa [PartialFacts.accepts, PartialFacts.lookup, equal] using holds
    · simp [PartialFacts.accepts, PartialFacts.lookup, equal]

/-- Matches the production recursive ITE walk; non-ITE operands stay opaque. -/
def distribute : Expr → Numeric → Scope
  | .input key _, bound => .facts [(key, bound)]
  | .ite _ guard left right, bound =>
    .choice guard (distribute left bound) (distribute right bound)

/-- This connects actual map/scoped Gamma to evaluated 256-bit membership. -/
theorem distribute_iff_evaluate (world : World) (expression : Expr) (bound : Numeric)
    (coherent : Coherent world expression) :
    ConditionalFacts.scopeGamma member (distribute expression bound) world.values (env world) ↔
      member bound (evaluate world expression) := by
  induction expression with
  | input key source =>
    change PartialFacts.Gamma member [(key, bound)] world.values ↔ _
    rw [singleton_Gamma_iff]
    change member bound (world.values key) ↔ member bound (world.sources source)
    rw [coherent]
  | ite key guard left right leftIH rightIH =>
    cases selected : ConditionalFacts.guardValue (env world) guard <;>
      simp [distribute, ConditionalFacts.scopeGamma, evaluate, selected,
        leftIH coherent.2.1, rightIH coherent.2.2]

theorem same_path_distribute (path : World → Prop) (expression : Expr) (bound : Numeric)
    (coherent : ∀ world, path world → Coherent world expression)
    (proved : EntailsBound path expression bound) :
    ∀ world, path world →
      ConditionalFacts.scopeGamma member (distribute expression bound) world.values (env world) := by
  intro world onPath
  exact (distribute_iff_evaluate world expression bound (coherent world onPath)).mpr
    (proved world onPath)

def whole (expression : Expr) (bound : Numeric) : Facts := [(expression.key, bound)]

def stampContent (expression : Expr) (bound : Numeric) : Scope :=
  .all (.facts (whole expression bound)) (distribute expression bound)

def trueStamp (expression : Expr) (bound : Numeric) : Scope :=
  .choice (.constant true) (stampContent expression bound) (.facts [])

def importBound (previous : Scope) (expression : Expr) (bound : Numeric) : Scope :=
  .all previous (trueStamp expression bound)

def StateGamma (common : Facts) (scopes : Scope) (world : World) : Prop :=
  PartialFacts.Gamma member common world.values ∧
    ConditionalFacts.scopeGamma member scopes world.values (env world)

theorem stamp_iff_evaluate (world : World) (expression : Expr) (bound : Numeric)
    (coherent : Coherent world expression) :
    ConditionalFacts.scopeGamma member (trueStamp expression bound) world.values (env world) ↔
      member bound (evaluate world expression) := by
  change (PartialFacts.Gamma member (whole expression bound) world.values ∧
    ConditionalFacts.scopeGamma member (distribute expression bound) world.values (env world)) ↔ _
  rw [whole, singleton_Gamma_iff, coherent_value world expression coherent,
    distribute_iff_evaluate world expression bound coherent]
  simp

theorem coherent_whole_iff_stamp (world : World) (expression : Expr) (bound : Numeric)
    (coherent : Coherent world expression) :
    PartialFacts.Gamma member (whole expression bound) world.values ↔
      ConditionalFacts.scopeGamma member (trueStamp expression bound) world.values (env world) := by
  rw [whole, singleton_Gamma_iff, coherent_value world expression coherent,
    stamp_iff_evaluate world expression bound coherent]

/-- The input can be the proved fact on the exact R key. Coherence discharges
    R's ITE equation; the result is membership on each selected arm key. -/
theorem same_path_key_guarded_arms (path : World → Prop) (key : Nat) (guard : Guard)
    (left right : Expr) (bound : Numeric)
    (coherent : ∀ world, path world → Coherent world (.ite key guard left right))
    (proved : ∀ world, path world → member bound (world.values key)) :
    (∀ world, path world → ConditionalFacts.guardValue (env world) guard = true →
      member bound (world.values left.key)) ∧
    (∀ world, path world → ConditionalFacts.guardValue (env world) guard = false →
      member bound (world.values right.key)) := by
  constructor
  · intro world onPath selected
    have boundHolds := proved world onPath
    rw [coherent_ite_equation world key guard left right (coherent world onPath)] at boundHolds
    simpa [selected] using boundHolds
  · intro world onPath selected
    have boundHolds := proved world onPath
    rw [coherent_ite_equation world key guard left right (coherent world onPath)] at boundHolds
    simpa [selected] using boundHolds

/-- Import always projects back to the old unconditional and scoped Gamma. -/
theorem import_preserves_old_Gamma (common : Facts) (previous : Scope)
    (expression : Expr) (bound : Numeric) (world : World)
    (holds : StateGamma common (importBound previous expression bound) world) :
    StateGamma common previous world := ⟨holds.1, holds.2.1⟩

/-- Same-F proved content makes the metadata stamp Gamma-neutral. -/
theorem same_path_import_Gamma_iff (path : World → Prop) (common : Facts) (previous : Scope)
    (expression : Expr) (bound : Numeric)
    (coherent : ∀ world, path world → Coherent world expression)
    (proved : EntailsBound path expression bound) (world : World) (onPath : path world) :
    StateGamma common (importBound previous expression bound) world ↔
      StateGamma common previous world := by
  constructor
  · exact import_preserves_old_Gamma common previous expression bound world
  · intro old
    exact ⟨old.1, old.2,
      (stamp_iff_evaluate world expression bound (coherent world onPath)).mpr
        (proved world onPath)⟩

/-- A whole-key query on this same F suffices; no separate leaf query is needed. -/
theorem same_path_whole_key_import_Gamma_iff (path : World → Prop) (common : Facts)
    (previous : Scope) (expression : Expr) (bound : Numeric)
    (coherent : ∀ world, path world → Coherent world expression)
    (proved : ∀ world, path world →
      PartialFacts.Gamma member (whole expression bound) world.values)
    (world : World) (onPath : path world) :
    StateGamma common (importBound previous expression bound) world ↔
      StateGamma common previous world := by
  constructor
  · exact import_preserves_old_Gamma common previous expression bound world
  · intro old
    exact ⟨old.1, old.2, (coherent_whole_iff_stamp world expression bound
      (coherent world onPath)).mp (proved world onPath)⟩

/-- Duplicate detection can skip work; even repeated insertion is idempotent in Gamma. -/
theorem repeated_stamp_idempotent (common : Facts) (previous : Scope)
    (expression : Expr) (bound : Numeric) (world : World) :
    StateGamma common (importBound (importBound previous expression bound) expression bound) world ↔
      StateGamma common (importBound previous expression bound) world := by
  change (_ ∧ ((_ ∧ _) ∧ _)) ↔ (_ ∧ (_ ∧ _))
  constructor
  · rintro ⟨commonHolds, ⟨old, stamp⟩, _⟩
    exact ⟨commonHolds, old, stamp⟩
  · rintro ⟨commonHolds, old, stamp⟩
    exact ⟨commonHolds, ⟨old, stamp⟩, stamp⟩

/-- Both ancestor guards remain on each nested implication in the actual scope. -/
theorem nested_ancestor_guards (world : World) (outer inner : Guard)
    (root middle a b c sa sb sc : Nat) (bound : Numeric) :
    ConditionalFacts.scopeGamma member
      (distribute (.ite root outer (.ite middle inner (.input a sa) (.input b sb))
        (.input c sc)) bound) world.values (env world) ↔
    ((ConditionalFacts.guardValue (env world) outer = true →
        ConditionalFacts.guardValue (env world) inner = true →
        member bound (world.values a)) ∧
      (ConditionalFacts.guardValue (env world) outer = true →
        ConditionalFacts.guardValue (env world) inner = false →
        member bound (world.values b)) ∧
      (ConditionalFacts.guardValue (env world) outer = false →
        member bound (world.values c))) := by
  cases ho : ConditionalFacts.guardValue (env world) outer <;>
    cases hi : ConditionalFacts.guardValue (env world) inner <;>
    simp [distribute, ConditionalFacts.scopeGamma, ho, hi, singleton_Gamma_iff]

-- The two coherent worlds keep opposite guards feasible with the same whole bound.
def fixtureExpr : Expr := .ite 30 (.expr 9) (.input 10 100) (.input 11 101)

def armWorld (selected : Bool) : World where
  sources := fun source => if source = 100 then (if selected then 1 else 2) else
    if source = 101 then (if selected then 2 else 1) else 0
  values := fun key => if key = 30 then 1 else
    if key = 9 then (if selected then 1 else 0) else
    if key = 10 then (if selected then 1 else 2) else
    if key = 11 then (if selected then 2 else 1) else 0

theorem fixture_coherent_and_bound (selected : Bool) :
    Coherent (armWorld selected) fixtureExpr ∧
      evaluate (armWorld selected) fixtureExpr = 1 := by
  cases selected <;>
    simp [Coherent, evaluate, fixtureExpr, armWorld, Expr.key, env, wordTruth,
      ConditionalFacts.guardValue]

theorem unknown_guard_cannot_force_branch :
    StateGamma [] (importBound (.facts []) fixtureExpr [1]) (armWorld false) ∧
    env (armWorld false) 9 = false ∧
    ¬ PartialFacts.Gamma member [(10, [1])] (armWorld false).values ∧
    StateGamma [] (importBound (.facts []) fixtureExpr [1]) (armWorld true) ∧
    env (armWorld true) 9 = true ∧
    ¬ PartialFacts.Gamma member [(11, [1])] (armWorld true).values := by
  have admitted (selected : Bool) :
      StateGamma [] (importBound (.facts []) fixtureExpr [1]) (armWorld selected) := by
    refine ⟨by intro key; trivial, (by intro key; trivial), ?_⟩
    apply (stamp_iff_evaluate _ _ _ (fixture_coherent_and_bound selected).1).mpr
    simp [member, (fixture_coherent_and_bound selected).2]
  refine ⟨admitted false, by decide, ?_, admitted true, by decide, ?_⟩ <;>
    rw [singleton_Gamma_iff] <;> simp [member, armWorld]

/-- A metadata-only valuation shows why a whole-key cache hit is not a full
    implication-tree stamp. It is deliberately not a coherent concrete world. -/
def wholeOnlyWorld : World :=
  { armWorld false with values := ConditionalFacts.setValue (armWorld false).values 9 1 }

theorem whole_fact_is_not_import_stamp :
    PartialFacts.Gamma member (whole fixtureExpr [1]) wholeOnlyWorld.values ∧
    ¬ ConditionalFacts.scopeGamma member (distribute fixtureExpr [1])
      wholeOnlyWorld.values (env wholeOnlyWorld) ∧
    ¬ Coherent wholeOnlyWorld fixtureExpr := by
  refine ⟨?_, ?_, ?_⟩
  · rw [whole, singleton_Gamma_iff]
    simp [member, fixtureExpr, Expr.key, wholeOnlyWorld, ConditionalFacts.setValue, armWorld]
  · change ¬ PartialFacts.Gamma member [(10, [1])] wholeOnlyWorld.values
    rw [singleton_Gamma_iff]
    simp [member, wholeOnlyWorld, ConditionalFacts.setValue, armWorld]
  · simp [Coherent, evaluate, fixtureExpr, wholeOnlyWorld, ConditionalFacts.setValue,
      armWorld, Expr.key, env, wordTruth, ConditionalFacts.guardValue]

def emptyArm : Scope := .facts [(10, [])]

theorem inactive_empty_arm_keeps_other_feasible :
    ¬ ConditionalFacts.scopeGamma member emptyArm (armWorld false).values (env (armWorld false)) ∧
    StateGamma []
      (importBound (.choice (.expr 9) emptyArm (.facts [])) fixtureExpr [1]) (armWorld false) := by
  constructor
  · change ¬ PartialFacts.Gamma member [(10, [])] (armWorld false).values
    rw [singleton_Gamma_iff]
    simp [member]
  · refine ⟨by intro key; trivial, (by intro key; trivial), ?_⟩
    apply (stamp_iff_evaluate _ _ _ (fixture_coherent_and_bound false).1).mpr
    simp [member, (fixture_coherent_and_bound false).2]

/-- The empty child under g AND !g is unreachable for either truth of g. -/
theorem contradictory_nested_arm_is_inactive (world : World) (guard : Guard)
    (left right : Scope) :
    ConditionalFacts.scopeGamma member
      (.choice guard (.choice guard left emptyArm) right) world.values (env world) ↔
    ConditionalFacts.scopeGamma member (.choice guard left right) world.values (env world) := by
  cases selected : ConditionalFacts.guardValue (env world) guard <;>
    simp [ConditionalFacts.scopeGamma, selected]

def guardKeys : Guard → List Nat
  | .constant _ => []
  | .expr key => [key]

/-- Includes ITE result identities and every guard, including inactive nodes. -/
def Expr.keys : Expr → List Nat
  | .input key _ => [key]
  | .ite key guard left right => key :: (guardKeys guard ++ (left.keys ++ right.keys))

def Expr.sources : Expr → List Nat
  | .input _ source => [source]
  | .ite _ _ left right => left.sources ++ right.sources

/-- The two namespaces are explicit; neither freshness condition implies the other. -/
def installSource (world : World) (key source : Nat) (value : Word) : World where
  sources := ConditionalFacts.setValue world.sources source value
  values := ConditionalFacts.setValue world.values key value

private theorem guard_installSource (world : World) (key source : Nat) (value : Word)
    (guard : Guard) (fresh : key ∉ guardKeys guard) :
    ConditionalFacts.guardValue (env (installSource world key source value)) guard =
      ConditionalFacts.guardValue (env world) guard := by
  cases guard with
  | constant truth => rfl
  | expr guard =>
    have different : guard ≠ key := by simpa [guardKeys, eq_comm] using fresh
    simp [ConditionalFacts.guardValue, env, installSource, ConditionalFacts.setValue, different]

/-- Fresh source and expression keys preserve the evaluation of old expressions. -/
theorem evaluate_installSource (world : World) (expression : Expr) (key source : Nat)
    (value : Word) (freshKey : key ∉ expression.keys) (freshSource : source ∉ expression.sources) :
    evaluate (installSource world key source value) expression = evaluate world expression := by
  induction expression with
  | input oldKey oldSource =>
    have different : oldSource ≠ source := by simpa [Expr.sources, eq_comm] using freshSource
    simp [evaluate, installSource, ConditionalFacts.setValue, different]
  | ite root guard left right leftIH rightIH =>
    change key ∉ root :: (guardKeys guard ++ (left.keys ++ right.keys)) at freshKey
    change source ∉ left.sources ++ right.sources at freshSource
    simp only [List.mem_cons, List.mem_append, not_or] at freshKey freshSource
    change (if ConditionalFacts.guardValue (env (installSource world key source value)) guard then
      evaluate (installSource world key source value) left
      else evaluate (installSource world key source value) right) = _
    rw [guard_installSource world key source value guard freshKey.2.1,
      leftIH freshKey.2.2.1 freshSource.1, rightIH freshKey.2.2.2 freshSource.2]
    rfl

theorem installSource_preserves_coherence (world : World) (expression : Expr)
    (key source : Nat) (value : Word) (freshKey : key ∉ expression.keys)
    (freshSource : source ∉ expression.sources) (coherent : Coherent world expression) :
    Coherent (installSource world key source value) expression := by
  induction expression with
  | input oldKey oldSource =>
    have keyDifferent : oldKey ≠ key := by simpa [Expr.keys, eq_comm] using freshKey
    have sourceDifferent : oldSource ≠ source := by simpa [Expr.sources, eq_comm] using freshSource
    simpa [Coherent, installSource, ConditionalFacts.setValue, keyDifferent, sourceDifferent]
      using coherent
  | ite root guard left right leftIH rightIH =>
    have sameEvaluation := evaluate_installSource world (.ite root guard left right)
      key source value freshKey freshSource
    change key ∉ root :: (guardKeys guard ++ (left.keys ++ right.keys)) at freshKey
    change source ∉ left.sources ++ right.sources at freshSource
    simp only [List.mem_cons, List.mem_append, not_or] at freshKey freshSource
    refine ⟨?_, leftIH freshKey.2.2.1 freshSource.1 coherent.2.1,
      rightIH freshKey.2.2.2 freshSource.2 coherent.2.2⟩
    rw [sameEvaluation]
    simpa [installSource, ConditionalFacts.setValue, Expr.key, Ne.symm freshKey.1] using coherent.1

/-- Reuses the existing map and full scoped-expression support invariance lemmas. -/
theorem installSource_old_support_Gamma (common : Facts) (scopes : Scope) (world : World)
    (key source : Nat) (value : Word) (freshMap : key ∉ common.map Prod.fst)
    (freshScopes : key ∉ ConditionalFacts.expressionSupport scopes) :
    StateGamma common scopes (installSource world key source value) ↔ StateGamma common scopes world := by
  exact and_congr
    (ConditionalFacts.fresh_map_invariance member common world.values key value freshMap)
    (ConditionalFacts.fresh_scope_invariance member wordTruth scopes world.values key value freshScopes)

def oldSourceWorld : World where
  sources := fun source => if source = 70 then 1 else 0
  values := fun key => if key = 7 then 1 else 0

def newSourceWorld : World := installSource oldSourceWorld 8 80 2

/-- An old source of length 1 coexists with a fresh source of length 2. The
    imported old stamp holds, but its bound is false of the new source. -/
theorem distinct_source_does_not_inherit_bound :
    Coherent newSourceWorld (.input 7 70) ∧ Coherent newSourceWorld (.input 8 80) ∧
    evaluate newSourceWorld (.input 7 70) = 1 ∧ evaluate newSourceWorld (.input 8 80) = 2 ∧
    StateGamma (whole (.input 7 70) [1]) (trueStamp (.input 7 70) [1]) newSourceWorld ∧
    ¬ PartialFacts.Gamma member (whole (.input 8 80) [1]) newSourceWorld.values := by
  have oldCoherent : Coherent newSourceWorld (.input 7 70) := by
    simp [Coherent, newSourceWorld, installSource, ConditionalFacts.setValue, oldSourceWorld]
  refine ⟨oldCoherent, ?_, ?_, ?_, ?_, ?_⟩
  · simp [Coherent, newSourceWorld, installSource, ConditionalFacts.setValue]
  · simp [evaluate, newSourceWorld, installSource, ConditionalFacts.setValue, oldSourceWorld]
  · simp [evaluate, newSourceWorld, installSource, ConditionalFacts.setValue]
  · constructor
    · rw [whole, singleton_Gamma_iff]
      simp [member, Expr.key, newSourceWorld, installSource, ConditionalFacts.setValue, oldSourceWorld]
    · apply (stamp_iff_evaluate _ _ _ oldCoherent).mpr
      simp [member, evaluate, newSourceWorld, installSource, ConditionalFacts.setValue, oldSourceWorld]
  · rw [whole, singleton_Gamma_iff]
    simp [member, Expr.key, newSourceWorld, installSource, ConditionalFacts.setValue]

/-!
Saved/current product supplement. Both ReadSources use the same initial world
and typed SnapshotId, but retain independent versions and write histories.
Conservative forgetting drops selected component constraints, not their product
structure. This is neither an initial observation nor a proof of EVM rollback.
-/
structure SourceProduct (snapshot : Snapshots.SnapshotId) where
  saved : Snapshots.ReadSource snapshot
  current : Snapshots.ReadSource snapshot

def ProductGamma {snapshot : Snapshots.SnapshotId} (product : SourceProduct snapshot)
    (keepSaved keepCurrent : Bool) (initial : Snapshots.Concrete)
    (saved current : Snapshots.InitialValue) : Prop :=
  (if keepSaved then product.saved.Models initial saved else True) ∧
  (if keepCurrent then product.current.Models initial current else True)

/-- A concrete implementation of conservative forgetting in each component. -/
theorem product_forgetting_inclusion {snapshot : Snapshots.SnapshotId}
    (product : SourceProduct snapshot) (keepSaved keepCurrent : Bool)
    (initial : Snapshots.Concrete) (saved current : Snapshots.InitialValue)
    (models : ProductGamma product true true initial saved current) :
    ProductGamma product keepSaved keepCurrent initial saved current := by
  cases keepSaved <;> cases keepCurrent <;> simp_all [ProductGamma]

def frameSources {snapshot : Snapshots.SnapshotId} (frame : Snapshots.Frame snapshot)
    (key : Snapshots.CellKey) (savedVersion currentVersion : Nat) : SourceProduct snapshot :=
  ⟨⟨savedVersion, frame.checkpoint, key⟩, ⟨currentVersion, frame.current, key⟩⟩

theorem rollback_selects_saved_source {snapshot : Snapshots.SnapshotId}
    (frame : Snapshots.Frame snapshot) (key : Snapshots.CellKey) (savedVersion currentVersion : Nat) :
    (frameSources (Snapshots.rollback frame) key savedVersion savedVersion).current =
      (frameSources frame key savedVersion currentVersion).saved := rfl

def productFrame : Snapshots.Frame Snapshots.fixtureSnapshot :=
  let base := Snapshots.Overlay.initial Snapshots.emptyRoot
  ⟨base, .write base Snapshots.fixtureKey 1, []⟩

def productSources : SourceProduct Snapshots.fixtureSnapshot :=
  frameSources productFrame Snapshots.fixtureKey 0 1

def productInitial : Snapshots.Concrete := fun _ => .word 0

/-- The runtime write produces current=1 while saved=0 in the same initial world. -/
theorem joint_saved_zero_current_one :
    productSources.saved.version ≠ productSources.current.version ∧
    ProductGamma productSources true true productInitial (.word 0) (.word 1) := by
  refine ⟨by decide, ?_⟩
  constructor <;>
    refine ⟨?_, ?_⟩
  · intro key value known
    simp [productSources, frameSources, productFrame, Snapshots.initialRoot,
      Snapshots.emptyRoot, Snapshots.lookup, Snapshots.lookupEntries] at known
  · rfl
  · intro key value known
    simp [productSources, frameSources, productFrame, Snapshots.initialRoot,
      Snapshots.emptyRoot, Snapshots.lookup, Snapshots.lookupEntries] at known
  · simp [Snapshots.ReadSource.denote, productSources, frameSources, productFrame, Snapshots.execute]

theorem joint_witness_survives_forgetting (keepSaved keepCurrent : Bool) :
    ProductGamma productSources keepSaved keepCurrent productInitial (.word 0) (.word 1) :=
  product_forgetting_inclusion _ _ _ _ _ _ joint_saved_zero_current_one.2

theorem joint_witness_is_not_diagonal :
    ProductGamma productSources true true productInitial (.word 0) (.word 1) ∧
    ¬ ∃ shared : Snapshots.InitialValue, shared = .word 0 ∧ shared = .word 1 := by
  exact ⟨joint_saved_zero_current_one.2, by simp⟩

theorem shared_fresh_key_loses_joint_witness (key : Nat) :
    ¬ ∃ values : Nat → Snapshots.InitialValue,
      values key = .word 0 ∧ values key = .word 1 := by
  rintro ⟨values, saved, current⟩
  exact joint_witness_is_not_diagonal.2 ⟨values key, saved, current⟩

/-- Two distinct fresh identities can represent any pair, including (0,1).
    This constructs the assignment; it does not assume an allocator is correct. -/
theorem distinct_keys_realize_pair (savedKey currentKey : Nat) (different : savedKey ≠ currentKey)
    (saved current : Snapshots.InitialValue) :
    ∃ values : Nat → Snapshots.InitialValue, values savedKey = saved ∧ values currentKey = current := by
  refine ⟨ConditionalFacts.setValue (fun _ => saved) currentKey current, ?_, ?_⟩ <;>
    simp [ConditionalFacts.setValue, different]

theorem fresh_pair_preserves_joint_witness :
    ∃ values : Nat → Snapshots.InitialValue,
      values productSources.saved.version = .word 0 ∧
      values productSources.current.version = .word 1 ∧
      ProductGamma productSources false false productInitial
        (values productSources.saved.version) (values productSources.current.version) ∧
      ProductGamma productSources true true productInitial
        (values productSources.saved.version) (values productSources.current.version) := by
  obtain ⟨values, saved, current⟩ := distinct_keys_realize_pair _ _
    joint_saved_zero_current_one.1 (.word 0) (.word 1)
  refine ⟨values, saved, current, ?_, ?_⟩ <;> rw [saved, current]
  · exact joint_witness_survives_forgetting false false
  · exact joint_saved_zero_current_one.2

theorem rollback_witness_uses_zero_not_one :
    productSources.current.denote productInitial = .word 1 ∧
    (frameSources (Snapshots.rollback productFrame) Snapshots.fixtureKey 0 0).current.denote
      productInitial = .word 0 := by
  constructor
  · simp [productSources, frameSources, productFrame, Snapshots.ReadSource.denote, Snapshots.execute]
  · rw [rollback_selects_saved_source productFrame Snapshots.fixtureKey 0 1]
    rfl

#print axioms evaluate_ite
#print axioms coherent_value
#print axioms coherent_ite_equation
#print axioms same_path_guarded_arms
#print axioms singleton_Gamma_iff
#print axioms distribute_iff_evaluate
#print axioms same_path_distribute
#print axioms stamp_iff_evaluate
#print axioms coherent_whole_iff_stamp
#print axioms same_path_key_guarded_arms
#print axioms import_preserves_old_Gamma
#print axioms same_path_import_Gamma_iff
#print axioms same_path_whole_key_import_Gamma_iff
#print axioms repeated_stamp_idempotent
#print axioms nested_ancestor_guards
#print axioms fixture_coherent_and_bound
#print axioms unknown_guard_cannot_force_branch
#print axioms whole_fact_is_not_import_stamp
#print axioms inactive_empty_arm_keeps_other_feasible
#print axioms contradictory_nested_arm_is_inactive
#print axioms evaluate_installSource
#print axioms installSource_preserves_coherence
#print axioms installSource_old_support_Gamma
#print axioms distinct_source_does_not_inherit_bound
#print axioms product_forgetting_inclusion
#print axioms rollback_selects_saved_source
#print axioms joint_saved_zero_current_one
#print axioms joint_witness_survives_forgetting
#print axioms joint_witness_is_not_diagonal
#print axioms shared_fresh_key_loses_joint_witness
#print axioms distinct_keys_realize_pair
#print axioms fresh_pair_preserves_joint_witness
#print axioms rollback_witness_uses_zero_not_one

end NoSmt.ReturnLengths
