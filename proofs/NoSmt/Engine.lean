import NoSmt.Effects
import NoSmt.Lifecycle

set_option autoImplicit false

/-!
Forward simulation and a postfixed certificate obligation for an engine indexed
by structural keys. A certificate must cover its initial inputs and every
successor of each retained input summary; empty queues are insufficient.

The concrete instantiation below proves local instruction steps (Word256
assignment/add/load/store, zero branch, unknown envelope, callback, checkpoint,
touch and rollback) rather than assuming their soundness. It is a register and
selected-account projection, not an encoding of all Rust MachinePayload fields
or EVM opcodes. The generic theorem exposes the remaining transfer/cover and
scheduler-to-certificate obligations without declaring them to be axioms.
-/
namespace NoSmt.Engine

universe u v w
variable {K : Type u} {C : Type v} {A : Type w}

inductive Reaches (step : C → C → Prop) (initial : C) : C → Prop where
  | refl : Reaches step initial initial
  | next (previous target : C) : Reaches step initial previous → step previous target →
      Reaches step initial target

def Covered (gamma : A → C → Prop) (certificate : K → Option A) (state : K × C) : Prop :=
  ∃ summary, certificate state.1 = some summary ∧ gamma summary state.2

def Postfixed (transfer : K → A → List (K × A)) (covers : A → A → Prop)
    (certificate : K → Option A) : Prop :=
  ∀ key summary, certificate key = some summary →
    ∀ output ∈ transfer key summary,
      ∃ stored, certificate output.1 = some stored ∧ covers stored output.2

def LocallySound (gamma : A → C → Prop) (transfer : K → A → List (K × A))
    (step : (K × C) → (K × C) → Prop) : Prop :=
  ∀ source target summary, gamma summary source.2 → step source target →
    ∃ output, (target.1, output) ∈ transfer source.1 summary ∧ gamma output target.2

theorem postfixed_one_step (gamma : A → C → Prop) (transfer : K → A → List (K × A))
    (covers : A → A → Prop) (step : (K × C) → (K × C) → Prop)
    (simulation : LocallySound gamma transfer step)
    (cover_sound : ∀ a b c, covers a b → gamma b c → gamma a c)
    (certificate : K → Option A) (closed : Postfixed transfer covers certificate)
    (source target : K × C) (h : Covered gamma certificate source) (edge : step source target) :
    Covered gamma certificate target := by
  obtain ⟨summary, installed, input⟩ := h
  obtain ⟨output, successor, included⟩ := simulation source target summary input edge
  obtain ⟨stored, present, covered⟩ := closed source.1 summary installed _ successor
  exact ⟨stored, present, cover_sound stored output target.2 covered included⟩

theorem postfixed_finite_prefix (gamma : A → C → Prop) (transfer : K → A → List (K × A))
    (covers : A → A → Prop) (step : (K × C) → (K × C) → Prop)
    (simulation : LocallySound gamma transfer step)
    (cover_sound : ∀ a b c, covers a b → gamma b c → gamma a c)
    (certificate : K → Option A) (closed : Postfixed transfer covers certificate)
    (initial target : K × C) (seed : Covered gamma certificate initial)
    (executionPath : Reaches step initial target) : Covered gamma certificate target := by
  induction executionPath with
  | refl => exact seed
  | next previous target _ edge ih =>
    exact postfixed_one_step gamma transfer covers step simulation cover_sound certificate closed
      previous target ih edge

abbrev Word := Effects.Word

def optionGamma {D : Type u} (fact : Option D) (value : D) : Prop :=
  match fact with
  | none => True
  | some known => value = known

/-- Structural coverage, not a definition that simply restates concretization. -/
def optionCovers {D : Type u} (wide narrow : Option D) : Prop :=
  wide = none ∨ wide = narrow

theorem option_covers_inclusion {D : Type u} (wide narrow : Option D) (value : D)
    (cover : optionCovers wide narrow) (h : optionGamma narrow value) : optionGamma wide value := by
  rcases cover with top | equal
  · simp [optionGamma, top]
  · simpa only [equal] using h

def storeCovers (wide narrow : Effects.Abstract) : Prop :=
  ∀ cell, optionCovers (wide cell) (narrow cell)

theorem store_covers_inclusion (wide narrow : Effects.Abstract) (concrete : Effects.Concrete)
    (cover : storeCovers wide narrow) (h : Effects.Gamma narrow concrete) : Effects.Gamma wide concrete := by
  intro cell value known
  rcases cover cell with top | equal
  · rw [top] at known
    contradiction
  · rw [equal] at known
    exact h cell value known

def accountCovers (wide narrow : Lifecycle.AbstractAccount) : Prop :=
  optionCovers wide.existence narrow.existence ∧
    optionCovers wide.touched narrow.touched ∧ optionCovers wide.empty narrow.empty

theorem flag_covers_inclusion (wide narrow : Option Bool) (value : Bool)
    (cover : optionCovers wide narrow) (h : Lifecycle.accepts narrow value) :
    Lifecycle.accepts wide value := by
  rcases cover with top | equal
  · simp [Lifecycle.accepts, top]
  · simpa only [equal] using h

theorem account_covers_inclusion (wide narrow : Lifecycle.AbstractAccount)
    (concrete : Lifecycle.ConcreteAccount) (cover : accountCovers wide narrow)
    (h : Lifecycle.Gamma narrow concrete) : Lifecycle.Gamma wide concrete :=
  ⟨flag_covers_inclusion _ _ _ cover.1 h.1,
    flag_covers_inclusion _ _ _ cover.2.1 h.2.1,
    flag_covers_inclusion _ _ _ cover.2.2 h.2.2⟩

structure Payload where
  reg : Word
  savedReg : Word
  store : Effects.Concrete
  savedStore : Effects.Concrete
  account : Lifecycle.ConcreteAccount
  savedAccount : Lifecycle.ConcreteAccount

structure AbstractPayload where
  reg : Option Word
  savedReg : Option Word
  store : Effects.Abstract
  savedStore : Effects.Abstract
  account : Lifecycle.AbstractAccount
  savedAccount : Lifecycle.AbstractAccount

def Gamma (abstract : AbstractPayload) (concrete : Payload) : Prop :=
  optionGamma abstract.reg concrete.reg ∧ optionGamma abstract.savedReg concrete.savedReg ∧
    Effects.Gamma abstract.store concrete.store ∧ Effects.Gamma abstract.savedStore concrete.savedStore ∧
    Lifecycle.Gamma abstract.account concrete.account ∧ Lifecycle.Gamma abstract.savedAccount concrete.savedAccount

def payloadCovers (wide narrow : AbstractPayload) : Prop :=
  optionCovers wide.reg narrow.reg ∧ optionCovers wide.savedReg narrow.savedReg ∧
    storeCovers wide.store narrow.store ∧ storeCovers wide.savedStore narrow.savedStore ∧
    accountCovers wide.account narrow.account ∧ accountCovers wide.savedAccount narrow.savedAccount

theorem payload_covers_inclusion (wide narrow : AbstractPayload) (concrete : Payload)
    (cover : payloadCovers wide narrow) (h : Gamma narrow concrete) : Gamma wide concrete := by
  rcases cover with ⟨cr, cs, ch, csh, ca, csa⟩
  rcases h with ⟨hr, hs, hh, hsh, ha, hsa⟩
  exact ⟨option_covers_inclusion _ _ _ cr hr, option_covers_inclusion _ _ _ cs hs,
    store_covers_inclusion _ _ _ ch hh, store_covers_inclusion _ _ _ csh hsh,
    account_covers_inclusion _ _ _ ca ha, account_covers_inclusion _ _ _ csa hsa⟩

def putAbstract (store : Effects.Abstract) (cell : Effects.Cell) (value : Option Word) : Effects.Abstract :=
  fun query => if query = cell then value else store query

theorem put_inclusion (abstract : Effects.Abstract) (concrete : Effects.Concrete)
    (cell : Effects.Cell) (fact : Option Word) (value : Word)
    (known : optionGamma fact value) (h : Effects.Gamma abstract concrete) :
    Effects.Gamma (putAbstract abstract cell fact) (Effects.setConcrete concrete cell value) := by
  intro query result output
  by_cases equal : query = cell
  · subst query
    simp only [putAbstract] at output
    change fact = some result at output
    rw [output] at known
    have same : value = result := known
    simp [Effects.setConcrete, same]
  · simp only [putAbstract, if_neg equal] at output
    simpa [Effects.setConcrete, equal] using h query result output

theorem option_add_inclusion (fact : Option Word) (value delta : Word) (h : optionGamma fact value) :
    optionGamma (fact.map (· + delta)) (value + delta) := by
  cases fact <;> simp_all [optionGamma]

theorem load_inclusion (abstract : Effects.Abstract) (concrete : Effects.Concrete)
    (cell : Effects.Cell) (h : Effects.Gamma abstract concrete) :
    optionGamma (abstract cell) (concrete cell) := by
  cases known : abstract cell with
  | none => trivial
  | some value => exact h cell value known

def unknownAccount (policy : Effects.Policy) (account : Lifecycle.AbstractAccount) : Lifecycle.AbstractAccount :=
  if policy.isStatic then Lifecycle.applyStaticTouches account
  else ⟨none, Lifecycle.touchPossible account.touched, none⟩

theorem unknown_account_inclusion (policy : Effects.Policy) (abstract : Lifecycle.AbstractAccount)
    (old next : Lifecycle.ConcreteAccount) (didTouch : Bool) (h : Lifecycle.Gamma abstract old)
    (touch : next.touched = (old.touched || didTouch))
    (static : policy.isStatic = true → next.existence = old.existence ∧ next.empty = old.empty) :
    Lifecycle.Gamma (unknownAccount policy abstract) next := by
  cases hs : policy.isStatic with
  | false =>
    simpa [unknownAccount, hs, Lifecycle.Gamma, Lifecycle.accepts, touch]
      using Lifecycle.optional_touch_inclusion abstract.touched old.touched didTouch h.2.1
  | true =>
    obtain ⟨existence, empty⟩ := static hs
    simpa [unknownAccount, hs, Lifecycle.Gamma, Lifecycle.applyStaticTouches, existence, empty, touch]
      using (show Lifecycle.accepts abstract.existence old.existence ∧
        Lifecycle.accepts (Lifecycle.touchPossible abstract.touched) (old.touched || didTouch) ∧
        Lifecycle.accepts abstract.empty old.empty from
        ⟨h.1, Lifecycle.optional_touch_inclusion _ _ _ h.2.1, h.2.2⟩)

inductive Op where
  | assign (next : Nat) (value : Word)
  | add (next : Nat) (delta : Word)
  | load (next : Nat) (cell : Effects.Cell)
  | store (next : Nat) (cell : Effects.Cell)
  | unknown (next : Nat) (policy : Effects.Policy)
  | callback (next : Nat) (body : Effects.Callback)
  | checkpoint (next : Nat)
  | rollback (next : Nat)
  | touch (next : Nat)
  | branchZero (zeroPC nonzeroPC : Nat)
  | halt

def Op.successors (owner : Nat) : Op → AbstractPayload → List (Nat × AbstractPayload)
  | .assign next value, input => [(next, { input with reg := some value })]
  | .add next delta, input => [(next, { input with reg := input.reg.map (· + delta) })]
  | .load next cell, input => [(next, { input with reg := input.store cell })]
  | .store next cell, input => [(next, { input with store := putAbstract input.store cell input.reg })]
  | .unknown next policy, input => [(next, { input with
      store := Effects.havoc policy input.store
      account := unknownAccount policy input.account })]
  | .callback next body, input => [(next, { input with
      store := body.abstractRun owner input.store
      account := Lifecycle.markTouched input.account })]
  | .checkpoint next, input => [(next, { input with
      savedReg := input.reg
      savedStore := input.store
      savedAccount := input.account })]
  | .rollback next, input => [(next, { input with
      reg := input.savedReg
      store := input.savedStore
      account := (Lifecycle.revertCall owner ⟨input.savedAccount, input.account⟩).current })]
  | .touch next, input => [(next, { input with account := Lifecycle.markTouched input.account })]
  | .branchZero zeroPC nonzeroPC, input => match input.reg with
    | none => [(zeroPC, input), (nonzeroPC, input)]
    | some value => if value = 0 then [(zeroPC, input)] else [(nonzeroPC, input)]
  | .halt, _ => []

inductive Executes (owner : Nat) : Op → Payload → (Nat × Payload) → Prop where
  | assign (next : Nat) (value : Word) (input : Payload) : Executes owner (.assign next value) input
      (next, { input with reg := value })
  | add (next : Nat) (delta : Word) (input : Payload) : Executes owner (.add next delta) input
      (next, { input with reg := input.reg + delta })
  | load (next : Nat) (cell : Effects.Cell) (input : Payload) : Executes owner (.load next cell) input
      (next, { input with reg := input.store cell })
  | store (next : Nat) (cell : Effects.Cell) (input : Payload) : Executes owner (.store next cell) input
      (next, { input with store := Effects.setConcrete input.store cell input.reg })
  | unknown (next : Nat) (policy : Effects.Policy) (input : Payload)
      (trace : List (Effects.Cell × Word)) (account : Lifecycle.ConcreteAccount) (didTouch : Bool)
      (allowed : ∀ cell value, (cell, value) ∈ trace → Effects.keeps policy cell = false)
      (touch : account.touched = (input.account.touched || didTouch))
      (static : policy.isStatic = true → account.existence = input.account.existence ∧ account.empty = input.account.empty) :
      Executes owner (.unknown next policy) input
        (next, { input with store := Effects.localRun trace input.store, account := account })
  | callback (next : Nat) (body : Effects.Callback) (input : Payload) :
      Executes owner (.callback next body) input (next, { input with
        store := body.run owner input.store
        account := Lifecycle.markConcrete input.account })
  | checkpoint (next : Nat) (input : Payload) : Executes owner (.checkpoint next) input
      (next, { input with savedReg := input.reg, savedStore := input.store, savedAccount := input.account })
  | rollback (next : Nat) (input : Payload) : Executes owner (.rollback next) input
      (next, { input with
        reg := input.savedReg
        store := input.savedStore
        account := Lifecycle.revertConcrete owner input.savedAccount input.account })
  | touch (next : Nat) (input : Payload) : Executes owner (.touch next) input
      (next, { input with account := Lifecycle.markConcrete input.account })
  | branchZero (zeroPC nonzeroPC : Nat) (input : Payload) : Executes owner (.branchZero zeroPC nonzeroPC) input
      (if input.reg = 0 then zeroPC else nonzeroPC, input)

theorem execute_inclusion (owner : Nat) (op : Op) (abstract : AbstractPayload) (concrete : Payload)
    (target : Nat × Payload) (h : Gamma abstract concrete) (execution : Executes owner op concrete target) :
    ∃ output, (target.1, output) ∈ op.successors owner abstract ∧ Gamma output target.2 := by
  rcases h with ⟨hr, hs, hh, hsh, ha, hsa⟩
  cases execution with
  | assign next value input =>
    refine ⟨{ abstract with reg := some value }, by simp [Op.successors], ?_⟩
    exact ⟨rfl, hs, hh, hsh, ha, hsa⟩
  | add next delta input =>
    refine ⟨{ abstract with reg := abstract.reg.map (· + delta) }, by simp [Op.successors], ?_⟩
    exact ⟨option_add_inclusion _ _ _ hr, hs, hh, hsh, ha, hsa⟩
  | load next cell input =>
    refine ⟨{ abstract with reg := abstract.store cell }, by simp [Op.successors], ?_⟩
    exact ⟨load_inclusion _ _ _ hh, hs, hh, hsh, ha, hsa⟩
  | store next cell input =>
    refine ⟨{ abstract with store := putAbstract abstract.store cell abstract.reg }, by simp [Op.successors], ?_⟩
    exact ⟨hr, hs, put_inclusion _ _ _ _ _ hr hh, hsh, ha, hsa⟩
  | unknown next policy input trace account didTouch allowed touch static =>
    refine ⟨{ abstract with
        store := Effects.havoc policy abstract.store
        account := unknownAccount policy abstract.account }, by simp [Op.successors], ?_⟩
    exact ⟨hr, hs, Effects.arbitrary_local_trace_closed _ _ _ _ allowed (Effects.havoc_inclusion _ _ _ hh),
      hsh, unknown_account_inclusion _ _ _ _ _ ha touch static, hsa⟩
  | callback next body input =>
    refine ⟨{ abstract with
        store := body.abstractRun owner abstract.store
        account := Lifecycle.markTouched abstract.account }, by simp [Op.successors], ?_⟩
    exact ⟨hr, hs, Effects.callback_inclusion _ _ _ _ hh, hsh, Lifecycle.markTouched_inclusion _ _ ha, hsa⟩
  | checkpoint next input =>
    refine ⟨{ abstract with
        savedReg := abstract.reg
        savedStore := abstract.store
        savedAccount := abstract.account }, by simp [Op.successors], ?_⟩
    exact ⟨hr, hr, hh, hh, ha, ha⟩
  | rollback next input =>
    refine ⟨{ abstract with
        reg := abstract.savedReg
        store := abstract.savedStore
        account := (Lifecycle.revertCall owner ⟨abstract.savedAccount, abstract.account⟩).current },
      by simp [Op.successors], ?_⟩
    exact ⟨hs, hs, hsh, hsh, Lifecycle.rollback_inclusion _ _ _ _ hsa ha, hsa⟩
  | touch next input =>
    refine ⟨{ abstract with account := Lifecycle.markTouched abstract.account }, by simp [Op.successors], ?_⟩
    exact ⟨hr, hs, hh, hsh, Lifecycle.markTouched_inclusion _ _ ha, hsa⟩
  | branchZero zeroPC nonzeroPC input =>
    refine ⟨abstract, ?_, ⟨hr, hs, hh, hsh, ha, hsa⟩⟩
    cases known : abstract.reg with
    | none => by_cases eq : concrete.reg = 0#256 <;> simp [Op.successors, known, eq]
    | some value =>
      have equal : concrete.reg = value := by simpa [optionGamma, known] using hr
      by_cases eq : value = 0#256 <;> simp [Op.successors, known, equal, eq]

def modeledStep (owner : Nat) (code : Nat → Op) (source target : Nat × Payload) : Prop :=
  Executes owner (code source.1) source.2 target

def modeledTransfer (owner : Nat) (code : Nat → Op) (key : Nat) (input : AbstractPayload) :
    List (Nat × AbstractPayload) := (code key).successors owner input

theorem modeled_forward_simulation (owner : Nat) (code : Nat → Op) :
    LocallySound Gamma (modeledTransfer owner code) (modeledStep owner code) := by
  intro source target abstract h edge
  exact execute_inclusion owner (code source.1) abstract source.2 target h edge

theorem modeled_postfixed_finite_prefix (owner : Nat) (code : Nat → Op)
    (certificate : Nat → Option AbstractPayload)
    (closed : Postfixed (modeledTransfer owner code) payloadCovers certificate)
    (initial target : Nat × Payload) (seed : Covered Gamma certificate initial)
    (executionPath : Reaches (modeledStep owner code) initial target) : Covered Gamma certificate target :=
  postfixed_finite_prefix Gamma (modeledTransfer owner code) payloadCovers (modeledStep owner code)
    (modeled_forward_simulation owner code) payload_covers_inclusion certificate closed initial target seed executionPath

theorem unknown_register_keeps_both_branch_targets (zeroPC nonzeroPC : Nat) (input : AbstractPayload)
    (unknown : input.reg = none) :
    (Op.branchZero zeroPC nonzeroPC).successors 7 input = [(zeroPC, input), (nonzeroPC, input)] := by
  simp [Op.successors, unknown]

def Op.targets : Op → List Nat
  | .assign next _ | .add next _ | .load next _ | .store next _ | .unknown next _ |
      .callback next _ | .checkpoint next | .rollback next | .touch next => [next]
  | .branchZero zeroPC nonzeroPC => [zeroPC, nonzeroPC]
  | .halt => []

theorem successor_has_declared_target (owner : Nat) (op : Op) (input : AbstractPayload)
    (output : Nat × AbstractPayload) (member : output ∈ op.successors owner input) :
    output.1 ∈ op.targets := by
  cases op with
  | assign next value | add next delta | load next cell | store next cell | unknown next policy | callback next body | checkpoint next | rollback next | touch next =>
    simp [Op.successors] at member
    subst output
    simp [Op.targets]
  | branchZero zeroPC nonzeroPC =>
    cases known : input.reg with
    | none =>
      simp [Op.successors, known] at member
      rcases member with rfl | rfl <;> simp [Op.targets]
    | some value =>
      by_cases eq : value = 0#256 <;> simp [Op.successors, known, eq] at member <;>
        subst output <;> simp [Op.targets]
  | halt => simp [Op.successors] at member

def Op.protectsCode (codeOwner : Nat) : Op → Prop
  | .store _ cell => cell ≠ .code codeOwner
  | .unknown _ policy => Effects.keeps policy (.code codeOwner) = true
  | _ => True

theorem transfer_preserves_protected_code (owner codeOwner : Nat) (op : Op)
    (input : AbstractPayload) (value : Word) (safe : op.protectsCode codeOwner)
    (current : input.store (.code codeOwner) = some value)
    (saved : input.savedStore (.code codeOwner) = some value)
    (output : Nat × AbstractPayload) (member : output ∈ op.successors owner input) :
    output.2.store (.code codeOwner) = some value ∧
      output.2.savedStore (.code codeOwner) = some value := by
  cases op with
  | assign next word | add next word | load next cell | touch next =>
    simp [Op.successors] at member
    subst output
    exact ⟨current, saved⟩
  | store next cell =>
    simp [Op.successors] at member
    subst output
    have different : Effects.Cell.code codeOwner ≠ cell := Ne.symm safe
    exact ⟨by simpa [putAbstract, different] using current, saved⟩
  | unknown next policy =>
    simp [Op.successors] at member
    subst output
    change Effects.keeps policy (.code codeOwner) = true at safe
    exact ⟨by simpa [Effects.havoc, safe] using current, saved⟩
  | callback next body =>
    simp [Op.successors] at member
    subst output
    exact ⟨by simpa only [Effects.callback_preserves_code] using current, saved⟩
  | checkpoint next =>
    simp [Op.successors] at member
    subst output
    exact ⟨current, current⟩
  | rollback next =>
    simp [Op.successors] at member
    subst output
    exact ⟨saved, saved⟩
  | branchZero zeroPC nonzeroPC =>
    cases known : input.reg with
    | none =>
      simp [Op.successors, known] at member
      rcases member with rfl | rfl <;> exact ⟨current, saved⟩
    | some word =>
      by_cases eq : word = 0#256 <;> simp [Op.successors, known, eq] at member <;>
        subst output <;> exact ⟨current, saved⟩
  | halt => simp [Op.successors] at member

def protectedStore : Effects.Abstract :=
  fun cell => if cell = .code 7 then some 123 else none

def topAccount : Lifecycle.AbstractAccount := ⟨none, none, none⟩

def loopSummary : AbstractPayload :=
  ⟨none, none, protectedStore, protectedStore, topAccount, topAccount⟩

def loopPolicy : Effects.Policy := ⟨[7], none, false⟩

/-- A genuine loop with unknown writes, known modular ADD callbacks and rollback. -/
def loopCode : Nat → Op
  | 0 => .checkpoint 1
  | 1 => .assign 2 Effects.maxWord
  | 2 => .add 3 1
  | 3 => .branchZero 4 8
  | 4 => .store 5 (.persistent 7 0)
  | 5 => .unknown 6 loopPolicy
  | 6 => .callback 7 (.seq (.add 0 1) (.call 7 (.add 0 1)))
  | 7 => .rollback 0
  | _ => .halt

def loopCertificate (pc : Nat) : Option AbstractPayload :=
  if pc ≤ 8 then some loopSummary else none

theorem loop_targets_bounded (pc target : Nat) (member : target ∈ (loopCode pc).targets) :
    target ≤ 8 := by
  unfold loopCode at member
  split at member <;> simp [Op.targets] at member <;> omega

theorem loop_protects_code (pc : Nat) : (loopCode pc).protectsCode 7 := by
  unfold loopCode
  split <;> simp [Op.protectsCode, Effects.keeps, loopPolicy]

theorem protected_store_covers (narrow : Effects.Abstract)
    (known : narrow (.code 7) = some 123) : storeCovers protectedStore narrow := by
  intro cell
  by_cases equal : cell = .code 7
  · right
    subst cell
    simpa [protectedStore] using known.symm
  · left
    simp [protectedStore, equal]

theorem loop_summary_covers (narrow : AbstractPayload)
    (current : narrow.store (.code 7) = some 123)
    (saved : narrow.savedStore (.code 7) = some 123) : payloadCovers loopSummary narrow := by
  refine ⟨Or.inl rfl, Or.inl rfl, protected_store_covers _ current,
    protected_store_covers _ saved, ?_, ?_⟩ <;> exact ⟨Or.inl rfl, Or.inl rfl, Or.inl rfl⟩

theorem loop_postfixed : Postfixed (modeledTransfer 7 loopCode) payloadCovers loopCertificate := by
  intro pc summary present output member
  unfold loopCertificate at present
  split at present
  · cases present
    have declared := successor_has_declared_target 7 (loopCode pc) loopSummary output member
    have bounded := loop_targets_bounded pc output.1 declared
    refine ⟨loopSummary, by simp [loopCertificate, bounded], ?_⟩
    have preserved := transfer_preserves_protected_code 7 7 (loopCode pc) loopSummary 123
      (loop_protects_code pc) (by simp [loopSummary, protectedStore])
      (by simp [loopSummary, protectedStore]) output member
    exact loop_summary_covers output.2 preserved.1 preserved.2
  · contradiction

def concreteInitialStore : Effects.Concrete :=
  fun cell => (protectedStore cell).getD 0

def loopInitial : Payload :=
  ⟨0, 0, concreteInitialStore, concreteInitialStore,
    ⟨true, false, false⟩, ⟨true, false, false⟩⟩

theorem initial_store_inclusion : Effects.Gamma protectedStore concreteInitialStore := by
  intro cell value known
  simp [concreteInitialStore, known]

theorem loop_seed_covered : Covered Gamma loopCertificate (0, loopInitial) := by
  refine ⟨loopSummary, by simp [loopCertificate], ?_⟩
  exact ⟨True.intro, True.intro, initial_store_inclusion, initial_store_inclusion,
    ⟨True.intro, True.intro, True.intro⟩, ⟨True.intro, True.intro, True.intro⟩⟩

theorem loop_all_finite_prefixes (target : Nat × Payload)
    (executionPath : Reaches (modeledStep 7 loopCode) (0, loopInitial) target) :
    Covered Gamma loopCertificate target :=
  modeled_postfixed_finite_prefix 7 loopCode loopCertificate loop_postfixed (0, loopInitial) target
    loop_seed_covered executionPath

theorem loop_from_any_admitted_seed (initial : Payload) (target : Nat × Payload)
    (input : Gamma loopSummary initial)
    (executionPath : Reaches (modeledStep 7 loopCode) (0, initial) target) :
    Covered Gamma loopCertificate target :=
  modeled_postfixed_finite_prefix 7 loopCode loopCertificate loop_postfixed (0, initial) target
    ⟨loopSummary, by simp [loopCertificate], input⟩ executionPath

theorem loop_preserves_protected_code (target : Nat × Payload)
    (executionPath : Reaches (modeledStep 7 loopCode) (0, loopInitial) target) :
    target.2.store (.code 7) = 123 := by
  obtain ⟨summary, present, included⟩ := loop_all_finite_prefixes target executionPath
  unfold loopCertificate at present
  split at present
  · cases present
    exact included.2.2.1 (.code 7) 123 (by simp [loopSummary, protectedStore])
  · contradiction

theorem loop_add_wraps_at_actual_transition :
    modeledStep 7 loopCode (2, { loopInitial with reg := Effects.maxWord })
      (3, { loopInitial with reg := 0 }) := by
  have step := Executes.add (owner := 7) 3 1 { loopInitial with reg := Effects.maxWord }
  simpa [modeledStep, loopCode, Effects.maxword_plus_one_is_zero] using step

def missingSuccessorCertificate (pc : Nat) : Option AbstractPayload :=
  if pc = 2 then some loopSummary else none

theorem empty_queue_does_not_prove_postfixed :
    ([] : List Nat) = [] ∧ ([] : List Nat) = [] ∧
      ¬ Postfixed (modeledTransfer 7 loopCode) payloadCovers missingSuccessorCertificate := by
  refine ⟨rfl, rfl, ?_⟩
  intro closed
  obtain ⟨stored, impossible, _⟩ := closed 2 loopSummary
    (by simp [missingSuccessorCertificate]) (3, loopSummary)
    (by simp [modeledTransfer, loopCode, Op.successors, loopSummary])
  simp [missingSuccessorCertificate] at impossible

#print axioms postfixed_one_step
#print axioms postfixed_finite_prefix
#print axioms option_covers_inclusion
#print axioms store_covers_inclusion
#print axioms flag_covers_inclusion
#print axioms account_covers_inclusion
#print axioms payload_covers_inclusion
#print axioms put_inclusion
#print axioms option_add_inclusion
#print axioms load_inclusion
#print axioms unknown_account_inclusion
#print axioms execute_inclusion
#print axioms modeled_forward_simulation
#print axioms modeled_postfixed_finite_prefix
#print axioms unknown_register_keeps_both_branch_targets
#print axioms successor_has_declared_target
#print axioms transfer_preserves_protected_code
#print axioms loop_targets_bounded
#print axioms loop_protects_code
#print axioms protected_store_covers
#print axioms loop_summary_covers
#print axioms loop_postfixed
#print axioms initial_store_inclusion
#print axioms loop_seed_covered
#print axioms loop_all_finite_prefixes
#print axioms loop_from_any_admitted_seed
#print axioms loop_preserves_protected_code
#print axioms loop_add_wraps_at_actual_transition
#print axioms empty_queue_does_not_prove_postfixed

end NoSmt.Engine
