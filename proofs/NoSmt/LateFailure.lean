import NoSmt.Lifecycle
import NoSmt.Effects

set_option autoImplicit false

/-!
Checkpoint-relative late failure, followed separately by root account cleanup.
The field projection exists only BEFORE cleanup: deleting a touched empty
account may change storage/code/balance, and transaction cleanup resets transient
state. No relation on those fields AFTER cleanup is asserted here.

The finite suffix syntax below changes only a Word256 register. Mapping Rust
block boundaries, EVM touch sites, instruction-local touch/exception ordering
(including SELFDESTRUCT beneficiaries), frames, gas and refusals to these
operations remains a manual correspondence obligation.
-/
namespace NoSmt.LateFailure

open Lifecycle

def rollbackAccount (owner : Nat) (saved current : AbstractAccount) : AbstractAccount :=
  (revertCall owner ⟨saved, current⟩).current

def failureAccount (owner : Nat) (saved current : AbstractAccount) : AbstractAccount :=
  finish (rollbackAccount owner saved current)

def failureConcrete (owner : Nat) (saved current : ConcreteAccount) : ConcreteAccount :=
  finishConcrete (revertConcrete owner saved current)

/-- Saved/current Gamma suffices; rollback and cleanup are derived operations. -/
theorem rollback_finish_inclusion (owner : Nat) (saved current : AbstractAccount)
    (checkpoint running : ConcreteAccount) (hs : Gamma saved checkpoint)
    (hc : Gamma current running) :
    Gamma (failureAccount owner saved current) (failureConcrete owner checkpoint running) :=
  finish_inclusion _ _ (rollback_inclusion owner ⟨saved, current⟩ checkpoint running hs hc)

def savedAccount : AbstractAccount := ⟨some true, some false, some true⟩
def savedConcrete : ConcreteAccount := ⟨true, false, true⟩
def lateAccount : AbstractAccount := markTouched savedAccount
def lateConcrete : ConcreteAccount := markConcrete savedConcrete

theorem exact_accounts : Gamma savedAccount savedConcrete ∧ Gamma lateAccount lateConcrete := by
  simp [savedAccount, savedConcrete, lateAccount, lateConcrete, markTouched, markConcrete,
    Gamma, accepts]

/-- Entry-only failure admits presence but excludes the later concrete absence. -/
theorem entry_only_counterexample :
    Gamma (finish savedAccount) (finishConcrete savedConcrete) ∧
      (finish savedAccount).existence = some true ∧
      (failureConcrete 3 savedConcrete lateConcrete).existence = false ∧
      ¬ Gamma (finish savedAccount) (failureConcrete 3 savedConcrete lateConcrete) := by
  simp [savedAccount, savedConcrete, lateConcrete, markConcrete, failureConcrete,
    revertConcrete, finishConcrete, finish, deletion, Gamma, accepts]

theorem late_failure_repairs_counterexample :
    Gamma (failureAccount 3 savedAccount lateAccount)
      (failureConcrete 3 savedConcrete lateConcrete) ∧
      (failureAccount 3 savedAccount lateAccount).existence = some false := by
  refine ⟨rollback_finish_inclusion _ _ _ _ _ exact_accounts.1 exact_accounts.2, ?_⟩
  rfl

def mayTouchAccount : AbstractAccount := applyStaticTouches savedAccount

/-- Unknown touch includes both did-touch choices and both final existences. -/
theorem may_touch_failure_inclusion (didTouch : Bool) :
    Gamma (failureAccount 3 savedAccount mayTouchAccount)
      (failureConcrete 3 savedConcrete (concreteOptionalTouch savedConcrete didTouch)) ∧
      (failureAccount 3 savedAccount mayTouchAccount).existence = none ∧
      accepts (failureAccount 3 savedAccount mayTouchAccount).existence true ∧
      accepts (failureAccount 3 savedAccount mayTouchAccount).existence false := by
  refine ⟨rollback_finish_inclusion _ _ _ _ _ exact_accounts.1
    (static_touch_inclusion _ _ didTouch exact_accounts.1), ?_⟩
  simp [failureAccount, rollbackAccount, mayTouchAccount, savedAccount, applyStaticTouches,
    touchPossible, revertCall, either, finish, deletion, accepts]

theorem ordinary_owner_uses_checkpoint (owner : Nat) (saved current : AbstractAccount)
    (ordinary : owner ≠ 3) :
    rollbackAccount owner saved current = saved ∧
      failureAccount owner saved current = finish saved := by
  simp [rollbackAccount, failureAccount, revertCall, ordinary]

theorem ordinary_owner_keeps_untouched_empty (owner : Nat) (ordinary : owner ≠ 3) :
    (failureConcrete owner savedConcrete lateConcrete).existence = true := by
  simp [failureConcrete, revertConcrete, ordinary, savedConcrete, finishConcrete]

theorem rollback_non_touch_fields (owner : Nat) (saved current : AbstractAccount) :
    (rollbackAccount owner saved current).existence = saved.existence ∧
      (rollbackAccount owner saved current).empty = saved.empty :=
  rollback_restores_account_fields owner ⟨saved, current⟩

theorem cleanup_resets_touch (owner : Nat) (saved current : AbstractAccount)
    (checkpoint running : ConcreteAccount) :
    (failureAccount owner saved current).touched = some false ∧
      (failureConcrete owner checkpoint running).touched = false :=
  ⟨rfl, rfl⟩

/-- Relative/child output, before any transaction cleanup. -/
structure AbstractState where
  fields : Effects.Abstract
  account : AbstractAccount

structure ConcreteState where
  fields : Effects.Concrete
  account : ConcreteAccount

def StateGamma (abstract : AbstractState) (concrete : ConcreteState) : Prop :=
  Effects.Gamma abstract.fields concrete.fields ∧ Gamma abstract.account concrete.account

def restoreAbstract (owner : Nat) (saved current : AbstractState) : AbstractState :=
  ⟨saved.fields, rollbackAccount owner saved.account current.account⟩

def restoreConcrete (owner : Nat) (saved current : ConcreteState) : ConcreteState :=
  ⟨saved.fields, revertConcrete owner saved.account current.account⟩

theorem restore_inclusion (owner : Nat) (saved current : AbstractState)
    (checkpoint running : ConcreteState) (hs : StateGamma saved checkpoint)
    (hc : StateGamma current running) :
    StateGamma (restoreAbstract owner saved current) (restoreConcrete owner checkpoint running) :=
  ⟨hs.1, rollback_inclusion owner ⟨saved.account, current.account⟩
    checkpoint.account running.account hs.2 hc.2⟩

/-- The second conjunct projects the account only, after root cleanup. -/
theorem restore_then_cleanup_inclusion (owner : Nat) (saved current : AbstractState)
    (checkpoint running : ConcreteState) (hs : StateGamma saved checkpoint)
    (hc : StateGamma current running) :
    StateGamma (restoreAbstract owner saved current) (restoreConcrete owner checkpoint running) ∧
      Gamma (finish (restoreAbstract owner saved current).account)
        (finishConcrete (restoreConcrete owner checkpoint running).account) := by
  have restored := restore_inclusion owner saved current checkpoint running hs hc
  exact ⟨restored, finish_inclusion _ _ restored.2⟩

def writeAbstract (state : AbstractState) (cell : Effects.Cell) (value : Effects.Word) :
    AbstractState :=
  { state with fields := Effects.setAbstract state.fields cell value }

def writeConcrete (state : ConcreteState) (cell : Effects.Cell) (value : Effects.Word) :
    ConcreteState :=
  { state with fields := Effects.setConcrete state.fields cell value }

theorem write_inclusion (abstract : AbstractState) (concrete : ConcreteState)
    (cell : Effects.Cell) (value : Effects.Word) (h : StateGamma abstract concrete) :
    StateGamma (writeAbstract abstract cell value) (writeConcrete concrete cell value) :=
  ⟨Effects.set_inclusion _ _ cell value h.1, h.2⟩

/-- All displayed word fields are restored before cleanup, including overwritten ones. -/
theorem word_write_restore_inclusion (owner : Nat) (saved current : AbstractState)
    (checkpoint running : ConcreteState) (cell : Effects.Cell) (value : Effects.Word)
    (hs : StateGamma saved checkpoint) (hc : StateGamma current running) :
    StateGamma (restoreAbstract owner saved (writeAbstract current cell value))
      (restoreConcrete owner checkpoint (writeConcrete running cell value)) ∧
      (restoreAbstract owner saved (writeAbstract current cell value)).fields cell =
        saved.fields cell ∧
      (restoreConcrete owner checkpoint (writeConcrete running cell value)).fields cell =
        checkpoint.fields cell :=
  ⟨restore_inclusion _ _ _ _ _ hs (write_inclusion _ _ _ _ hc), rfl, rfl⟩

theorem persistent_write_rollback (state : Effects.Concrete) (owner slot : Nat)
    (value : Effects.Word) :
    let saved : AbstractState := ⟨Effects.exactState state, savedAccount⟩
    let checkpoint : ConcreteState := ⟨state, savedConcrete⟩
    let cell : Effects.Cell := .persistent owner slot
    StateGamma (restoreAbstract 3 saved (writeAbstract saved cell value))
      (restoreConcrete 3 checkpoint (writeConcrete checkpoint cell value)) ∧
      (writeConcrete checkpoint cell value).fields cell = value ∧
      (restoreConcrete 3 checkpoint (writeConcrete checkpoint cell value)).fields cell =
        state cell := by
  have h : StateGamma ⟨Effects.exactState state, savedAccount⟩ ⟨state, savedConcrete⟩ :=
    ⟨Effects.exact_inclusion state, exact_accounts.1⟩
  refine ⟨restore_inclusion _ _ _ _ _ h (write_inclusion _ _ _ _ h), ?_, rfl⟩
  simp [writeConcrete, Effects.setConcrete]

/-- A write at one owner leaves a different owner's persistent field unchanged. -/
theorem different_owner_write_unaffected (abstract : AbstractState) (concrete : ConcreteState)
    (owner other slot otherSlot : Nat) (value : Effects.Word) (different : other ≠ owner) :
    (writeAbstract abstract (.persistent owner slot) value).fields (.persistent other otherSlot) =
      abstract.fields (.persistent other otherSlot) ∧
      (writeConcrete concrete (.persistent owner slot) value).fields (.persistent other otherSlot) =
        concrete.fields (.persistent other otherSlot) := by
  simp [writeAbstract, writeConcrete, Effects.setAbstract, Effects.setConcrete, different]

/-- A child/relative failure retains presence and sticky touch until root cleanup. -/
theorem relative_failure_before_cleanup (fields : Effects.Concrete) :
    let saved : AbstractState := ⟨Effects.exactState fields, savedAccount⟩
    let current : AbstractState := ⟨Effects.exactState fields, lateAccount⟩
    (restoreAbstract 3 saved current).account.existence = some true ∧
      (restoreAbstract 3 saved current).account.touched = some true ∧
      (finish (restoreAbstract 3 saved current).account).existence = some false := by
  exact ⟨rfl, rfl, rfl⟩

/-- A deliberately small suffix syntax, with no gas or EVM decoder. -/
inductive PureOp where
  | add (delta : Effects.Word)
  | jump

def runPure : List PureOp → Effects.Word → ConcreteState → Effects.Word × ConcreteState
  | [], register, state => (register, state)
  | .add delta :: rest, register, state => runPure rest (register + delta) state
  | .jump :: rest, register, state => runPure rest register state

theorem pure_add_continues (register delta : Effects.Word) (state : ConcreteState) :
    runPure [.add delta] register state = (register + delta, state) := rfl

theorem pure_suffix_preserves_projection (ops : List PureOp) (register : Effects.Word)
    (state : ConcreteState) : (runPure ops register state).2 = state := by
  induction ops generalizing register with
  | nil => rfl
  | cons op rest ih =>
    cases op with
    | add delta => exact ih (register + delta)
    | jump => exact ih register

/-- Any finite suffix of this syntax has the same projected failure output. -/
theorem pure_suffix_failure_inclusion (owner : Nat) (ops : List PureOp)
    (register : Effects.Word) (saved current : AbstractState)
    (checkpoint running : ConcreteState) (hs : StateGamma saved checkpoint)
    (hc : StateGamma current running) :
    StateGamma (restoreAbstract owner saved current)
      (restoreConcrete owner checkpoint (runPure ops register running).2) ∧
      Gamma (finish (restoreAbstract owner saved current).account)
        (finishConcrete (restoreConcrete owner checkpoint (runPure ops register running).2).account) := by
  rw [pure_suffix_preserves_projection]
  exact restore_then_cleanup_inclusion owner saved current checkpoint running hs hc

/-- Normal execution is retained; may/must touch also admits relative failure. -/
def alternatives (owner : Nat) (saved current : AbstractState) : List AbstractState :=
  if current.account.touched = some false then [current]
  else [current, restoreAbstract owner saved current]

def Covered (outputs : List AbstractState) (concrete : ConcreteState) : Prop :=
  ∃ abstract, abstract ∈ outputs ∧ StateGamma abstract concrete

theorem normal_execution_included (owner : Nat) (ops : List PureOp)
    (register : Effects.Word) (saved current : AbstractState) (running : ConcreteState)
    (hc : StateGamma current running) :
    Covered (alternatives owner saved current) (runPure ops register running).2 := by
  refine ⟨current, ?_, ?_⟩
  · by_cases untouched : current.account.touched = some false <;>
      simp [alternatives, untouched]
  · rw [pure_suffix_preserves_projection]
    exact hc

theorem failure_alternative_included (owner : Nat) (ops : List PureOp)
    (register : Effects.Word) (saved current : AbstractState)
    (checkpoint running : ConcreteState) (hs : StateGamma saved checkpoint)
    (hc : StateGamma current running) (eligible : current.account.touched ≠ some false) :
    Covered (alternatives owner saved current)
      (restoreConcrete owner checkpoint (runPure ops register running).2) := by
  refine ⟨restoreAbstract owner saved current, ?_, ?_⟩
  · simp [alternatives, eligible]
  · rw [pure_suffix_preserves_projection]
    exact restore_inclusion owner saved current checkpoint running hs hc

#print axioms rollback_finish_inclusion
#print axioms exact_accounts
#print axioms entry_only_counterexample
#print axioms late_failure_repairs_counterexample
#print axioms may_touch_failure_inclusion
#print axioms ordinary_owner_uses_checkpoint
#print axioms ordinary_owner_keeps_untouched_empty
#print axioms rollback_non_touch_fields
#print axioms cleanup_resets_touch
#print axioms restore_inclusion
#print axioms restore_then_cleanup_inclusion
#print axioms write_inclusion
#print axioms word_write_restore_inclusion
#print axioms persistent_write_rollback
#print axioms different_owner_write_unaffected
#print axioms relative_failure_before_cleanup
#print axioms pure_add_continues
#print axioms pure_suffix_preserves_projection
#print axioms pure_suffix_failure_inclusion
#print axioms normal_execution_included
#print axioms failure_alternative_included

end NoSmt.LateFailure
