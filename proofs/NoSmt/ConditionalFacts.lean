import NoSmt.PartialFacts
import NoSmt.Facts

set_option autoImplicit false

/-!
Common-key must facts plus recursively selector-guarded residual scopes. This
reuses PartialFacts.lookup/Gamma/join, not a separate boolean fact model.
Residual equality compares the entire abstract component; production BTreeMap
unique keys justify the explicit UniqueKeys premise for first-match lists.

Generic joins expose only numeric component inclusion premises. Concrete finite
Nat-set and BitVec256-set instances discharge them. Guard environments may be
supplied separately, requiring path truth; fresh injections below instead derive
truth from the numeric key valuation and prove invariance on the old support.
The ExprId evaluator, source/root meaning, capture-avoiding namespace allocation
and production NumericValue/provenance refinement remain binding obligations.

Pruning is exact recursive semantic equivalence under compatible known truths.
The scoped-tree normalization corresponds to production fix 14e6a48 plus its
mechanical U256-import correction 7009a3d. This does not prove general ExprId
renaming exact, NumericValue refinement, or the whole Rust executor.
-/
namespace NoSmt.ConditionalFacts

universe u v
variable {D : Type u} {C : Type v}

def UniqueKeys (facts : PartialFacts.FactMap D) : Prop :=
  (facts.map Prod.fst).Nodup

def residual [DecidableEq D] :
    PartialFacts.FactMap D → PartialFacts.FactMap D → PartialFacts.FactMap D
  | [], _ => []
  | (key, value) :: rest, common =>
    if PartialFacts.lookup common key = some value then
      residual rest common
    else
      (key, value) :: residual rest common

theorem residual_eq_filter [DecidableEq D]
    (facts common : PartialFacts.FactMap D) :
    residual facts common = facts.filter (fun entry =>
      decide (PartialFacts.lookup common entry.1 ≠ some entry.2)) := by
  induction facts with
  | nil => rfl
  | cons entry rest ih =>
    obtain ⟨key, value⟩ := entry
    by_cases shared : PartialFacts.lookup common key = some value
    · simp [residual, shared, ih]
    · simp [residual, shared, ih]

theorem mem_residual [DecidableEq D] (facts common : PartialFacts.FactMap D)
    (entry : Nat × D) :
    entry ∈ residual facts common ↔
      entry ∈ facts ∧ PartialFacts.lookup common entry.1 ≠ some entry.2 := by
  rw [residual_eq_filter]
  simp

/-- Filtering can only remove keys, even when the input has duplicates. -/
theorem residual_keys_subset [DecidableEq D]
    (facts common : PartialFacts.FactMap D) (key : Nat)
    (member : key ∈ (residual facts common).map Prod.fst) :
    key ∈ facts.map Prod.fst := by
  rcases List.mem_map.mp member with ⟨entry, present, equal⟩
  exact List.mem_map.mpr ⟨entry, ((mem_residual facts common entry).mp present).1, equal⟩

/-- A structurally absent key cannot acquire a first-match lookup value. -/
theorem lookup_none_of_not_mem_keys (facts : PartialFacts.FactMap D) (key : Nat)
    (absent : key ∉ facts.map Prod.fst) : PartialFacts.lookup facts key = none := by
  revert absent
  induction facts with
  | nil => intro _; rfl
  | cons entry rest ih =>
    obtain ⟨head, value⟩ := entry
    intro absent
    have different : key ≠ head := by
      intro equal
      exact absent (by simp [equal])
    have tailAbsent : key ∉ rest.map Prod.fst := by
      intro member
      exact absent (by simpa only [List.map_cons] using List.mem_cons_of_mem head member)
    simpa [PartialFacts.lookup, different] using ih tailAbsent

theorem residual_uniqueKeys [DecidableEq D]
    (facts common : PartialFacts.FactMap D) (unique : UniqueKeys facts) :
    UniqueKeys (residual facts common) := by
  revert unique
  induction facts with
  | nil => intro _; simp [UniqueKeys, residual]
  | cons entry rest ih =>
    obtain ⟨key, value⟩ := entry
    intro unique
    have parts : key ∉ rest.map Prod.fst ∧ UniqueKeys rest := by
      simpa [UniqueKeys] using unique
    have tailUnique := ih parts.2
    by_cases shared : PartialFacts.lookup common key = some value
    · simpa [residual, shared] using tailUnique
    · have absent : key ∉ (residual rest common).map Prod.fst := by
        intro member
        exact parts.1 (residual_keys_subset rest common key member)
      simpa [residual, shared, UniqueKeys] using List.nodup_cons.mpr ⟨absent, tailUnique⟩

theorem lookup_residual [DecidableEq D]
    (facts common : PartialFacts.FactMap D) (key : Nat) (unique : UniqueKeys facts) :
    PartialFacts.lookup (residual facts common) key =
      match PartialFacts.lookup facts key with
      | none => none
      | some value =>
        if PartialFacts.lookup common key = some value then none else some value := by
  revert unique
  induction facts with
  | nil => intro _; rfl
  | cons entry rest ih =>
    obtain ⟨head, value⟩ := entry
    intro unique
    have parts : head ∉ rest.map Prod.fst ∧ UniqueKeys rest := by
      simpa [UniqueKeys] using unique
    have tailFormula := ih parts.2
    by_cases same : key = head
    · subst key
      have tailAbsent : PartialFacts.lookup (residual rest common) head = none :=
        lookup_none_of_not_mem_keys _ _ (fun member =>
          parts.1 (residual_keys_subset rest common head member))
      by_cases shared : PartialFacts.lookup common head = some value
      · simp [residual, PartialFacts.lookup, shared, tailAbsent]
      · simp [residual, PartialFacts.lookup, shared]
    · by_cases shared : PartialFacts.lookup common head = some value
      · simpa [residual, PartialFacts.lookup, same, shared] using tailFormula
      · simpa [residual, PartialFacts.lookup, same, shared] using tailFormula

theorem Gamma_residual [DecidableEq D] (gamma : D → C → Prop)
    (facts common : PartialFacts.FactMap D) (valuation : Nat → C)
    (unique : UniqueKeys facts) (satisfies : PartialFacts.Gamma gamma facts valuation) :
    PartialFacts.Gamma gamma (residual facts common) valuation := by
  intro key
  rw [lookup_residual facts common key unique]
  have point := satisfies key
  cases original : PartialFacts.lookup facts key with
  | none => trivial
  | some value =>
    by_cases shared : PartialFacts.lookup common key = some value
    · simp [shared, PartialFacts.accepts]
    · simpa [shared, PartialFacts.accepts, original] using point

theorem Gamma_common_residual_iff [DecidableEq D] (gamma : D → C → Prop)
    (facts common : PartialFacts.FactMap D) (valuation : Nat → C)
    (unique : UniqueKeys facts) :
    (PartialFacts.Gamma gamma common valuation ∧
      PartialFacts.Gamma gamma (residual facts common) valuation) ↔
    (PartialFacts.Gamma gamma common valuation ∧ PartialFacts.Gamma gamma facts valuation) := by
  constructor
  · rintro ⟨commonHolds, residualHolds⟩
    refine ⟨commonHolds, ?_⟩
    intro key
    cases original : PartialFacts.lookup facts key with
    | none => trivial
    | some value =>
      by_cases shared : PartialFacts.lookup common key = some value
      · simpa [PartialFacts.accepts, shared] using commonHolds key
      · have point := residualHolds key
        rw [lookup_residual facts common key unique] at point
        simpa [original, shared, PartialFacts.accepts] using point
  · rintro ⟨commonHolds, factsHold⟩
    exact ⟨commonHolds, Gamma_residual gamma facts common valuation unique factsHold⟩

def duplicateShadowFacts : PartialFacts.FactMap Facts.Numeric := [(7, [1]), (7, [0])]
def duplicateShadowCommon : PartialFacts.FactMap Facts.Numeric := [(7, [1])]
def duplicateShadowValuation : Nat → Nat := fun _ => 1

/-- Removing the shared first entry exposes a conflicting entry at the same key. -/
theorem duplicate_shadow_lookups :
    PartialFacts.lookup duplicateShadowFacts 7 = some [1] ∧
    PartialFacts.lookup (residual duplicateShadowFacts duplicateShadowCommon) 7 = some [0] := by
  decide

theorem duplicate_shadow_counterexample :
    ¬ UniqueKeys duplicateShadowFacts ∧
    PartialFacts.Gamma (fun values value => value ∈ values)
      duplicateShadowFacts duplicateShadowValuation ∧
    PartialFacts.Gamma (fun values value => value ∈ values)
      duplicateShadowCommon duplicateShadowValuation ∧
    ¬ PartialFacts.Gamma (fun values value => value ∈ values)
      (residual duplicateShadowFacts duplicateShadowCommon) duplicateShadowValuation := by
  refine ⟨by simp [UniqueKeys, duplicateShadowFacts], ?_, ?_, ?_⟩
  · intro key
    by_cases same : key = 7 <;>
      simp [PartialFacts.lookup, PartialFacts.accepts, duplicateShadowFacts,
        duplicateShadowValuation, same]
  · intro key
    by_cases same : key = 7 <;>
      simp [PartialFacts.lookup, PartialFacts.accepts, duplicateShadowCommon,
        duplicateShadowValuation, same]
  · intro holds
    have bad := holds 7
    simp [residual, PartialFacts.lookup, PartialFacts.accepts, duplicateShadowFacts,
      duplicateShadowCommon, duplicateShadowValuation] at bad

inductive Guard where
  | constant (truth : Bool)
  | expr (key : Nat)
  deriving DecidableEq

def guardValue (env : Nat → Bool) : Guard → Bool
  | .constant truth => truth
  | .expr key => env key

inductive Scope (D : Type u) where
  | facts (values : PartialFacts.FactMap D)
  | all (left right : Scope D)
  | choice (guard : Guard) (left right : Scope D)

def scopeGamma (gamma : D → C → Prop) (tree : Scope D)
    (valuation : Nat → C) (env : Nat → Bool) : Prop :=
  match tree with
  | .facts values => PartialFacts.Gamma gamma values valuation
  | .all left right =>
    scopeGamma gamma left valuation env ∧ scopeGamma gamma right valuation env
  | .choice guard left right =>
    if guardValue env guard then scopeGamma gamma left valuation env
    else scopeGamma gamma right valuation env

def prune (known : Nat → Option Bool) : Scope D → Scope D
  | .facts values => .facts values
  | .all left right => .all (prune known left) (prune known right)
  | .choice (.constant true) left _ => prune known left
  | .choice (.constant false) _ right => prune known right
  | .choice (.expr key) left right =>
    match known key with
    | some true => prune known left
    | some false => prune known right
    | none => .choice (.expr key) (prune known left) (prune known right)

theorem prune_facts (known : Nat → Option Bool) (values : PartialFacts.FactMap D) :
    prune known (.facts values) = Scope.facts values := rfl

/-- Both conjuncts remain, including when one has an empty numeric fact. -/
theorem prune_all (known : Nat → Option Bool) (left right : Scope D) :
    prune known (.all left right) = .all (prune known left) (prune known right) := rfl

theorem prune_constant_true (known : Nat → Option Bool) (left right : Scope D) :
    prune known (.choice (.constant true) left right) = prune known left := rfl

theorem prune_constant_false (known : Nat → Option Bool) (left right : Scope D) :
    prune known (.choice (.constant false) left right) = prune known right := rfl

theorem prune_known_true (known : Nat → Option Bool) (key : Nat)
    (left right : Scope D) (determined : known key = some true) :
    prune known (.choice (.expr key) left right) = prune known left := by
  simp [prune, determined]

theorem prune_known_false (known : Nat → Option Bool) (key : Nat)
    (left right : Scope D) (determined : known key = some false) :
    prune known (.choice (.expr key) left right) = prune known right := by
  simp [prune, determined]

/-- Unknown expression guards retain both normalized alternatives as a choice. -/
theorem prune_unknown (known : Nat → Option Bool) (key : Nat)
    (left right : Scope D) (unknown : known key = none) :
    prune known (.choice (.expr key) left right) =
      .choice (.expr key) (prune known left) (prune known right) := by
  simp [prune, unknown]

theorem scopeGamma_prune (gamma : D → C → Prop) (known : Nat → Option Bool)
    (tree : Scope D) (valuation : Nat → C) (env : Nat → Bool)
    (compatible : ∀ key truth, known key = some truth → env key = truth) :
    scopeGamma gamma (prune known tree) valuation env ↔
      scopeGamma gamma tree valuation env := by
  induction tree with
  | facts values => rfl
  | all left right leftIH rightIH =>
    exact and_congr leftIH rightIH
  | choice guard left right leftIH rightIH =>
    cases guard with
    | constant truth =>
      cases truth <;> simp [prune, scopeGamma, guardValue, leftIH, rightIH]
    | expr key =>
      cases determined : known key with
      | none =>
        simp [prune, determined, scopeGamma, guardValue, leftIH, rightIH]
      | some truth =>
        have envValue := compatible key truth determined
        cases truth <;>
          simp [prune, determined, scopeGamma, guardValue, envValue, leftIH, rightIH]

/-!
These membership statements expose the selected leaf's actual map, rather than
only a weakened semantic consequence. The structural equations above also retain
its entry order, shadowed entries, and lookup behavior exactly.
-/
theorem prune_constant_true_membership (known : Nat → Option Bool)
    (values : PartialFacts.FactMap D) (inactive : Scope D) (entry : Nat × D) :
    (match prune known (.choice (.constant true) (.facts values) inactive) with
      | .facts kept => entry ∈ kept
      | _ => False) ↔ entry ∈ values := Iff.rfl

theorem prune_constant_false_membership (known : Nat → Option Bool)
    (inactive : Scope D) (values : PartialFacts.FactMap D) (entry : Nat × D) :
    (match prune known (.choice (.constant false) inactive (.facts values)) with
      | .facts kept => entry ∈ kept
      | _ => False) ↔ entry ∈ values := Iff.rfl

theorem prune_known_true_membership (known : Nat → Option Bool) (key : Nat)
    (values : PartialFacts.FactMap D) (inactive : Scope D) (entry : Nat × D)
    (determined : known key = some true) :
    (match prune known (.choice (.expr key) (.facts values) inactive) with
      | .facts kept => entry ∈ kept
      | _ => False) ↔ entry ∈ values := by
  rw [prune_known_true known key (.facts values) inactive determined]
  rfl

theorem prune_known_false_membership (known : Nat → Option Bool) (key : Nat)
    (inactive : Scope D) (values : PartialFacts.FactMap D) (entry : Nat × D)
    (determined : known key = some false) :
    (match prune known (.choice (.expr key) inactive (.facts values)) with
      | .facts kept => entry ∈ kept
      | _ => False) ↔ entry ∈ values := by
  rw [prune_known_false known key inactive (.facts values) determined]
  rfl

/-- An actually infeasible inactive leaf cannot invalidate a feasible selection. -/
theorem inactive_empty_numeric_cannot_poison (known : Nat → Option Bool)
    (selected : PartialFacts.FactMap (List Nat)) (key : Nat)
    (valuation : Nat → Nat) (env : Nat → Bool)
    (feasible : PartialFacts.Gamma (fun values value => value ∈ values) selected valuation) :
    ¬ scopeGamma (fun values value => value ∈ values)
        (.facts [(key, ([] : List Nat))]) valuation env ∧
      scopeGamma (fun values value => value ∈ values)
        (prune known (.choice (.constant true) (.facts selected)
          (.facts [(key, ([] : List Nat))]))) valuation env := by
  constructor
  · intro impossible
    have atKey := impossible key
    simp [PartialFacts.accepts, PartialFacts.lookup] at atKey
  · exact feasible

/-- Syntactic support includes guards in both children, even inactive children. -/
def support : Scope D → List Nat
  | .facts _ => []
  | .all left right => support left ++ support right
  | .choice (.constant _) left right => support left ++ support right
  | .choice (.expr key) left right => key :: (support left ++ support right)

/-- Only the Bool environment varies here; the numeric valuation stays fixed. -/
theorem scopeGamma_support_congr (gamma : D → C → Prop) (tree : Scope D)
    (valuation : Nat → C) (env otherEnv : Nat → Bool)
    (agree : ∀ key, key ∈ support tree → env key = otherEnv key) :
    scopeGamma gamma tree valuation env ↔ scopeGamma gamma tree valuation otherEnv := by
  induction tree generalizing env otherEnv with
  | facts values => rfl
  | all left right leftIH rightIH =>
    apply and_congr
    · apply leftIH env otherEnv
      intro key member
      exact agree key (by simp [support, member])
    · apply rightIH env otherEnv
      intro key member
      exact agree key (by simp [support, member])
  | choice guard left right leftIH rightIH =>
    have agreeLeft : ∀ key, key ∈ support left → env key = otherEnv key := by
      intro key member
      exact agree key (by cases guard <;> simp [support, member])
    have agreeRight : ∀ key, key ∈ support right → env key = otherEnv key := by
      intro key member
      exact agree key (by cases guard <;> simp [support, member])
    have sameGuard : guardValue env guard = guardValue otherEnv guard := by
      cases guard with
      | constant truth => rfl
      | expr key => exact agree key (by simp [support])
    have leftSame := leftIH env otherEnv agreeLeft
    have rightSame := rightIH env otherEnv agreeRight
    simp [scopeGamma, sameGuard, leftSame, rightSame]

def setChoice (env : Nat → Bool) (fresh : Nat) (truth : Bool) : Nat → Bool :=
  fun key => if key = fresh then truth else env key

theorem setChoice_at (env : Nat → Bool) (fresh : Nat) (truth : Bool) :
    setChoice env fresh truth fresh = truth := by
  simp [setChoice]

theorem setChoice_of_ne (env : Nat → Bool) (fresh key : Nat) (truth : Bool)
    (different : key ≠ fresh) :
    setChoice env fresh truth key = env key := by
  simp [setChoice, different]

theorem scopeGamma_setChoice_fresh (gamma : D → C → Prop) (tree : Scope D)
    (valuation : Nat → C) (env : Nat → Bool) (fresh : Nat) (truth : Bool)
    (absent : fresh ∉ support tree) :
    scopeGamma gamma tree valuation (setChoice env fresh truth) ↔
      scopeGamma gamma tree valuation env := by
  apply scopeGamma_support_congr
  intro key member
  apply setChoice_of_ne
  intro equal
  subst key
  exact absent member

theorem scopeGamma_fresh_true_false (gamma : D → C → Prop) (tree : Scope D)
    (valuation : Nat → C) (env : Nat → Bool) (fresh : Nat)
    (absent : fresh ∉ support tree) :
    scopeGamma gamma tree valuation (setChoice env fresh true) ↔
      scopeGamma gamma tree valuation (setChoice env fresh false) := by
  exact (scopeGamma_setChoice_fresh gamma tree valuation env fresh true absent).trans
    (scopeGamma_setChoice_fresh gamma tree valuation env fresh false absent).symm

/- The unconditional common map and branch residuals are separate conjuncts.
   Equality in residual compares the whole component D, including any modeled
   provenance. Neither shared roots nor equal numeric projections equate D. -/
def scopedJoin [DecidableEq D] (merge : D → D → D) (guard : Guard)
    (left right : PartialFacts.FactMap D) (previousLeft previousRight : Scope D) : Scope D :=
  let common := PartialFacts.join merge left right
  .all (.facts common)
    (.choice guard
      (.all (.facts (residual left common)) previousLeft)
      (.all (.facts (residual right common)) previousRight))

theorem scoped_join_left [DecidableEq D] (gamma : D → C → Prop)
    (merge : D → D → D)
    (component : ∀ a b value, gamma a value → gamma (merge a b) value)
    (guard : Guard) (left right : PartialFacts.FactMap D)
    (previousLeft previousRight : Scope D) (values : Nat → C) (env : Nat → Bool)
    (unique : UniqueKeys left) (truth : guardValue env guard = true)
    (numeric : PartialFacts.Gamma gamma left values)
    (scopes : scopeGamma gamma previousLeft values env) :
    scopeGamma gamma (scopedJoin merge guard left right previousLeft previousRight)
      values env := by
  change PartialFacts.Gamma gamma (PartialFacts.join merge left right) values ∧ _
  constructor
  · exact PartialFacts.join_left gamma merge component left right values numeric
  · simp only [scopeGamma, truth, ↓reduceIte]
    exact ⟨Gamma_residual gamma left _ values unique numeric, scopes⟩

theorem scoped_join_right [DecidableEq D] (gamma : D → C → Prop)
    (merge : D → D → D)
    (component : ∀ a b value, gamma b value → gamma (merge a b) value)
    (guard : Guard) (left right : PartialFacts.FactMap D)
    (previousLeft previousRight : Scope D) (values : Nat → C) (env : Nat → Bool)
    (unique : UniqueKeys right) (truth : guardValue env guard = false)
    (numeric : PartialFacts.Gamma gamma right values)
    (scopes : scopeGamma gamma previousRight values env) :
    scopeGamma gamma (scopedJoin merge guard left right previousLeft previousRight)
      values env := by
  change PartialFacts.Gamma gamma (PartialFacts.join merge left right) values ∧ _
  constructor
  · exact PartialFacts.join_right gamma merge component left right values numeric
  · simp only [scopeGamma, truth, Bool.false_eq_true, ↓reduceIte]
    exact ⟨Gamma_residual gamma right _ values unique numeric, scopes⟩

/-- Residual filtering loses no selected guarantee when conjoined with common.
    Previous scopes stay conjuncts; no equality of immutable roots is used. -/
theorem scoped_join_selected_iff [DecidableEq D] (gamma : D → C → Prop)
    (merge : D → D → D) (guard : Guard) (left right : PartialFacts.FactMap D)
    (previousLeft previousRight : Scope D) (values : Nat → C) (env : Nat → Bool)
    (uniqueLeft : UniqueKeys left) (uniqueRight : UniqueKeys right) :
    scopeGamma gamma (scopedJoin merge guard left right previousLeft previousRight)
        values env ↔
      PartialFacts.Gamma gamma (PartialFacts.join merge left right) values ∧
        (if guardValue env guard then
          PartialFacts.Gamma gamma left values ∧ scopeGamma gamma previousLeft values env
        else PartialFacts.Gamma gamma right values ∧ scopeGamma gamma previousRight values env) := by
  cases truth : guardValue env guard with
  | false =>
    simp only [scopedJoin, scopeGamma, truth, Bool.false_eq_true, ↓reduceIte]
    simpa only [and_assoc] using
      and_congr (Gamma_common_residual_iff gamma right _ values uniqueRight)
        (Iff.rfl : scopeGamma gamma previousRight values env ↔ _)
  | true =>
    simp only [scopedJoin, scopeGamma, truth, ↓reduceIte]
    simpa only [and_assoc] using
      and_congr (Gamma_common_residual_iff gamma left _ values uniqueLeft)
        (Iff.rfl : scopeGamma gamma previousLeft values env ↔ _)

/-- Concrete finite sets discharge both numeric merge obligations. This is a
    map instance; it does not encode production intervals/KnownBits/congruence. -/
theorem finite_set_scoped_join (left right : PartialFacts.FactMap (List Nat))
    (previousLeft previousRight : Scope (List Nat))
    (values : Nat → Nat) (env : Nat → Bool) (guard : Guard)
    (uniqueLeft : UniqueKeys left) (uniqueRight : UniqueKeys right) :
    ((guardValue env guard = true ∧ PartialFacts.Gamma (fun xs x => x ∈ xs) left values ∧
        scopeGamma (fun xs x => x ∈ xs) previousLeft values env) →
      scopeGamma (fun xs x => x ∈ xs)
        (scopedJoin List.append guard left right previousLeft previousRight) values env) ∧
    ((guardValue env guard = false ∧ PartialFacts.Gamma (fun xs x => x ∈ xs) right values ∧
        scopeGamma (fun xs x => x ∈ xs) previousRight values env) →
      scopeGamma (fun xs x => x ∈ xs)
        (scopedJoin List.append guard left right previousLeft previousRight) values env) := by
  constructor
  · rintro ⟨truth, numeric, scopes⟩
    exact scoped_join_left _ _ (by intro a b v h; exact List.mem_append_left b h)
      _ _ _ _ _ _ _ uniqueLeft truth numeric scopes
  · rintro ⟨truth, numeric, scopes⟩
    exact scoped_join_right _ _ (by intro a b v h; exact List.mem_append_right a h)
      _ _ _ _ _ _ _ uniqueRight truth numeric scopes

/-- The same map/scoping inclusion over actual 256-bit words; arithmetic and
    the Rust NumericValue implementation are outside this finite-set instance. -/
theorem word256_scoped_join (left right : PartialFacts.FactMap (List (BitVec 256)))
    (previousLeft previousRight : Scope (List (BitVec 256)))
    (values : Nat → BitVec 256) (env : Nat → Bool) (guard : Guard)
    (uniqueLeft : UniqueKeys left) (uniqueRight : UniqueKeys right) :
    ((guardValue env guard = true ∧ PartialFacts.Gamma (fun xs x => x ∈ xs) left values ∧
        scopeGamma (fun xs x => x ∈ xs) previousLeft values env) →
      scopeGamma (fun xs x => x ∈ xs)
        (scopedJoin List.append guard left right previousLeft previousRight) values env) ∧
    ((guardValue env guard = false ∧ PartialFacts.Gamma (fun xs x => x ∈ xs) right values ∧
        scopeGamma (fun xs x => x ∈ xs) previousRight values env) →
      scopeGamma (fun xs x => x ∈ xs)
        (scopedJoin List.append guard left right previousLeft previousRight) values env) := by
  constructor
  · rintro ⟨truth, numeric, scopes⟩
    exact scoped_join_left _ _ (by intro a b v h; exact List.mem_append_left b h)
      _ _ _ _ _ _ _ uniqueLeft truth numeric scopes
  · rintro ⟨truth, numeric, scopes⟩
    exact scoped_join_right _ _ (by intro a b v h; exact List.mem_append_right a h)
      _ _ _ _ _ _ _ uniqueRight truth numeric scopes

def leftNumbers : PartialFacts.FactMap (List Nat) := [(7, [1, 2, 3, 4, 5, 6, 7, 8, 9])]
def rightNumbers : PartialFacts.FactMap (List Nat) := [(7, [20])]
def emptyNumbers : PartialFacts.FactMap (List Nat) := [(7, [])]
def numericMember (xs : List Nat) (value : Nat) : Prop := value ∈ xs
def value20 : Nat → Nat := fun _ => 20
def value1 : Nat → Nat := fun _ => 1
def falseEnv : Nat → Bool := fun _ => false
def trueEnv : Nat → Bool := fun _ => true

private theorem right_numbers_admitted : PartialFacts.Gamma numericMember rightNumbers value20 := by
  intro key
  by_cases h : key = 7 <;>
    simp [PartialFacts.accepts, PartialFacts.lookup, numericMember, rightNumbers, value20, h]

private theorem left_numbers_admitted : PartialFacts.Gamma numericMember leftNumbers value1 := by
  intro key
  by_cases h : key = 7 <;>
    simp [PartialFacts.accepts, PartialFacts.lookup, numericMember, leftNumbers, value1, h]

/-- No known truth selects the left scope globally. The right valuation is
    admitted by the real scoped join but refutes the original one-sided Gamma. -/
theorem unknown_selector_cannot_promote_left :
    scopeGamma numericMember
      (scopedJoin List.append (.expr 101) leftNumbers rightNumbers (.facts []) (.facts []))
      value20 falseEnv ∧ ¬ PartialFacts.Gamma numericMember leftNumbers value20 := by
  constructor
  · exact scoped_join_right _ _ (by intro a b v h; exact List.mem_append_right a h)
      _ _ _ _ _ _ _ (by simp [UniqueKeys, rightNumbers]) rfl
      right_numbers_admitted (by intro key; trivial)
  · intro h
    have bad := h 7
    simp [PartialFacts.accepts, PartialFacts.lookup, numericMember, leftNumbers, value20] at bad

theorem unknown_selector_keeps_left :
    scopeGamma numericMember
      (scopedJoin List.append (.expr 101) leftNumbers rightNumbers (.facts []) (.facts []))
      value1 trueEnv := by
  exact scoped_join_left _ _ (by intro a b v h; exact List.mem_append_left b h)
    _ _ _ _ _ _ _ (by simp [UniqueKeys, leftNumbers]) rfl
    left_numbers_admitted (by intro key; trivial)

/-- An existing, false guard cannot be repurposed as an independent fresh
    selector to inject this left path without its truth obligation. -/
theorem nonfresh_left_requires_truth :
    PartialFacts.Gamma numericMember leftNumbers value1 ∧
    ¬ scopeGamma numericMember
      (scopedJoin List.append (.expr 101) leftNumbers rightNumbers (.facts []) (.facts []))
      value1 falseEnv := by
  refine ⟨left_numbers_admitted, ?_⟩
  intro h
  have bad := h.2.1 7
  simp [residual, PartialFacts.join, PartialFacts.lookup, PartialFacts.accepts,
    numericMember, leftNumbers, rightNumbers, value1] at bad

/-- Even a nested inactive scope containing an EMPTY numeric entry is not a
    contradiction of the feasible selected-right value. -/
theorem inactive_empty_scope_is_feasible :
    ¬ PartialFacts.Gamma numericMember emptyNumbers value20 ∧
    scopeGamma numericMember
      (.choice (.expr 101) (.facts emptyNumbers)
        (.all (.facts rightNumbers)
          (.choice (.expr 102) (.facts emptyNumbers) (.facts rightNumbers))))
      value20 falseEnv := by
  constructor
  · intro h
    have bad := h 7
    simp [PartialFacts.accepts, PartialFacts.lookup, numericMember, emptyNumbers] at bad
  · change PartialFacts.Gamma numericMember rightNumbers value20 ∧
      PartialFacts.Gamma numericMember rightNumbers value20
    exact ⟨right_numbers_admitted, right_numbers_admitted⟩

/-- P2 shape: choosing the outer false branch erases an obsolete inner selector
    while retaining both active conjunctions and their exact numeric maps. -/
theorem constant_pruning_drops_inactive_selector :
    prune (fun _ => none)
      (.choice (.constant false)
        (.choice (.expr 101) (.facts leftNumbers) (.facts emptyNumbers))
        (.all (.facts rightNumbers) (.facts rightNumbers))) =
      .all (.facts rightNumbers) (.facts rightNumbers) := rfl

theorem constant_pruning_preserves_active_numeric :
    scopeGamma numericMember
      (prune (fun _ => none)
        (.choice (.constant false)
          (.choice (.expr 101) (.facts leftNumbers) (.facts emptyNumbers))
          (.all (.facts rightNumbers) (.facts rightNumbers)))) value20 falseEnv ∧
    ¬ scopeGamma numericMember
      (prune (fun _ => none)
        (.choice (.constant false)
          (.choice (.expr 101) (.facts leftNumbers) (.facts emptyNumbers))
          (.all (.facts rightNumbers) (.facts rightNumbers)))) value1 falseEnv := by
  rw [constant_pruning_drops_inactive_selector]
  constructor
  · exact ⟨right_numbers_admitted, right_numbers_admitted⟩
  · intro impossible
    have bad := impossible.1 7
    simp [PartialFacts.lookup, PartialFacts.accepts, numericMember, rightNumbers, value1] at bad

theorem prune_known_true_numeric (gamma : D → C → Prop) (known : Nat → Option Bool)
    (selector query : Nat) (values : Nat → C) (env : Nat → Bool)
    (facts : PartialFacts.FactMap D) (inactive : Scope D) (domain : D)
    (determined : known selector = some true)
    (found : PartialFacts.lookup facts query = some domain)
    (active : scopeGamma gamma
      (prune known (.choice (.expr selector) (.facts facts) inactive)) values env) :
    gamma domain (values query) := by
  rw [prune_known_true known selector (.facts facts) inactive determined] at active
  have point := active query
  simpa only [PartialFacts.accepts, found] using point

theorem prune_known_false_numeric (gamma : D → C → Prop) (known : Nat → Option Bool)
    (selector query : Nat) (values : Nat → C) (env : Nat → Bool)
    (inactive : Scope D) (facts : PartialFacts.FactMap D) (domain : D)
    (determined : known selector = some false)
    (found : PartialFacts.lookup facts query = some domain)
    (active : scopeGamma gamma
      (prune known (.choice (.expr selector) inactive (.facts facts))) values env) :
    gamma domain (values query) := by
  rw [prune_known_false known selector inactive (.facts facts) determined] at active
  have point := active query
  simpa only [PartialFacts.accepts, found] using point

/-- All expression labels that affect this model's fact membership or guards.
    Rust must also account for leaves of composite ExprId terms and the source,
    root and payload namespaces; this list is not a Rust dependency traversal. -/
def expressionSupport : Scope D → List Nat
  | .facts facts => facts.map Prod.fst
  | .all left right => expressionSupport left ++ expressionSupport right
  | .choice (.constant _) left right => expressionSupport left ++ expressionSupport right
  | .choice (.expr key) left right => key :: (expressionSupport left ++ expressionSupport right)

def inputSupport (left right : PartialFacts.FactMap D)
    (previousLeft previousRight : Scope D) : List Nat :=
  left.map Prod.fst ++ (right.map Prod.fst ++
    (expressionSupport previousLeft ++ expressionSupport previousRight))

def setValue (values : Nat → C) (selector : Nat) (selected : C) : Nat → C :=
  fun key => if key = selector then selected else values key

theorem fresh_map_invariance (gamma : D → C → Prop) (facts : PartialFacts.FactMap D)
    (values : Nat → C) (selector : Nat) (selected : C)
    (fresh : selector ∉ facts.map Prod.fst) :
    PartialFacts.Gamma gamma facts (setValue values selector selected) ↔
      PartialFacts.Gamma gamma facts values := by
  have same : ∀ key, PartialFacts.accepts gamma (PartialFacts.lookup facts key)
      (setValue values selector selected key) ↔
      PartialFacts.accepts gamma (PartialFacts.lookup facts key) (values key) := by
    intro key
    by_cases equal : key = selector
    · subst key
      simp only [lookup_none_of_not_mem_keys facts selector fresh, PartialFacts.accepts]
    · simp only [setValue, equal, ↓reduceIte]
  exact forall_congr' same

/-- Numeric values and guard truths share the same key interpretation here.
    Freshness proves the required old-meaning invariance rather than assuming
    that selector truth is independent of every old numeric expression. -/
theorem fresh_scope_invariance (gamma : D → C → Prop) (decode : C → Bool)
    (tree : Scope D) (values : Nat → C) (selector : Nat) (selected : C)
    (fresh : selector ∉ expressionSupport tree) :
    scopeGamma gamma tree (setValue values selector selected)
        (fun key => decode (setValue values selector selected key)) ↔
      scopeGamma gamma tree values (fun key => decode (values key)) := by
  induction tree with
  | facts facts => exact fresh_map_invariance gamma facts values selector selected fresh
  | all left right ihLeft ihRight =>
    simp only [expressionSupport, List.mem_append, not_or] at fresh
    exact and_congr (ihLeft fresh.1) (ihRight fresh.2)
  | choice guard left right ihLeft ihRight =>
    cases guard with
    | constant truth =>
      simp only [expressionSupport, List.mem_append, not_or] at fresh
      cases truth
      · exact ihRight fresh.2
      · exact ihLeft fresh.1
    | expr key =>
      simp only [expressionSupport, List.mem_cons, List.mem_append, not_or] at fresh
      have different : key ≠ selector := Ne.symm fresh.1
      simp only [scopeGamma, guardValue, setValue, different, ↓reduceIte]
      split
      · exact ihLeft fresh.2.1
      · exact ihRight fresh.2.2

/-- The fresh update constructs a left injection. Unique maps and component
    inclusion are the only numeric premises; old interpretation invariance is
    discharged structurally over inputSupport. Capture-avoiding ExprId/root
    interpretation must implement this update to use it for Rust. -/
theorem fresh_left_injection [DecidableEq D] (gamma : D → C → Prop)
    (merge : D → D → D)
    (component : ∀ a b value, gamma a value → gamma (merge a b) value)
    (decode : C → Bool) (selected : C) (selectedTruth : decode selected = true)
    (left right : PartialFacts.FactMap D) (previousLeft previousRight : Scope D)
    (values : Nat → C) (selector : Nat) (unique : UniqueKeys left)
    (fresh : selector ∉ inputSupport left right previousLeft previousRight)
    (numeric : PartialFacts.Gamma gamma left values)
    (scopes : scopeGamma gamma previousLeft values (fun key => decode (values key))) :
    scopeGamma gamma
      (scopedJoin merge (.expr selector) left right previousLeft previousRight)
      (setValue values selector selected)
      (fun key => decode (setValue values selector selected key)) := by
  simp only [inputSupport, List.mem_append, not_or] at fresh
  apply scoped_join_left gamma merge component _ _ _ _ _ _ _ unique
  · simpa only [guardValue, setValue, ↓reduceIte] using selectedTruth
  · exact (fresh_map_invariance gamma left values selector selected fresh.1).mpr numeric
  · exact (fresh_scope_invariance gamma decode previousLeft values selector selected
      fresh.2.2.1).mpr scopes

theorem fresh_right_injection [DecidableEq D] (gamma : D → C → Prop)
    (merge : D → D → D)
    (component : ∀ a b value, gamma b value → gamma (merge a b) value)
    (decode : C → Bool) (selected : C) (selectedTruth : decode selected = false)
    (left right : PartialFacts.FactMap D) (previousLeft previousRight : Scope D)
    (values : Nat → C) (selector : Nat) (unique : UniqueKeys right)
    (fresh : selector ∉ inputSupport left right previousLeft previousRight)
    (numeric : PartialFacts.Gamma gamma right values)
    (scopes : scopeGamma gamma previousRight values (fun key => decode (values key))) :
    scopeGamma gamma
      (scopedJoin merge (.expr selector) left right previousLeft previousRight)
      (setValue values selector selected)
      (fun key => decode (setValue values selector selected key)) := by
  simp only [inputSupport, List.mem_append, not_or] at fresh
  apply scoped_join_right gamma merge component _ _ _ _ _ _ _ unique
  · simpa only [guardValue, setValue, ↓reduceIte] using selectedTruth
  · exact (fresh_map_invariance gamma right values selector selected fresh.2.1).mpr numeric
  · exact (fresh_scope_invariance gamma decode previousRight values selector selected
      fresh.2.2.2).mpr scopes

#print axioms residual_eq_filter
#print axioms mem_residual
#print axioms residual_keys_subset
#print axioms lookup_none_of_not_mem_keys
#print axioms residual_uniqueKeys
#print axioms lookup_residual
#print axioms Gamma_residual
#print axioms Gamma_common_residual_iff
#print axioms duplicate_shadow_lookups
#print axioms duplicate_shadow_counterexample
#print axioms prune_facts
#print axioms prune_all
#print axioms prune_constant_true
#print axioms prune_constant_false
#print axioms prune_known_true
#print axioms prune_known_false
#print axioms prune_unknown
#print axioms scopeGamma_prune
#print axioms prune_constant_true_membership
#print axioms prune_constant_false_membership
#print axioms prune_known_true_membership
#print axioms prune_known_false_membership
#print axioms inactive_empty_numeric_cannot_poison
#print axioms scopeGamma_support_congr
#print axioms setChoice_at
#print axioms setChoice_of_ne
#print axioms scopeGamma_setChoice_fresh
#print axioms scopeGamma_fresh_true_false
#print axioms scoped_join_left
#print axioms scoped_join_right
#print axioms scoped_join_selected_iff
#print axioms finite_set_scoped_join
#print axioms word256_scoped_join
#print axioms unknown_selector_cannot_promote_left
#print axioms unknown_selector_keeps_left
#print axioms nonfresh_left_requires_truth
#print axioms inactive_empty_scope_is_feasible
#print axioms constant_pruning_drops_inactive_selector
#print axioms constant_pruning_preserves_active_numeric
#print axioms prune_known_true_numeric
#print axioms prune_known_false_numeric
#print axioms fresh_map_invariance
#print axioms fresh_scope_invariance
#print axioms fresh_left_injection
#print axioms fresh_right_injection

end NoSmt.ConditionalFacts
