import Std

set_option autoImplicit false

/-!
Executable EIP-161 touched-empty decision and transaction rollback. This models
the proposed three-valued touch/emptiness policy, not all account lifecycle opcodes.
Rollback models the owner-3 sticky-touch exception as well as ordinary owners.
Logs.lean treats the separate source-identity and event-aggregation boundary.
-/
namespace NoSmt.Lifecycle

def accepts (flag : Option Bool) (value : Bool) : Prop :=
  match flag with
  | none => True
  | some known => value = known

/-- Unknown touches/emptiness can authorize only a may-delete result. -/
def deletion : Option Bool → Option Bool → Option Bool
  | some false, _ => some false
  | _, some false => some false
  | some true, some true => some true
  | _, _ => none

theorem deletion_inclusion (touch empty : Option Bool) (ct ce : Bool)
    (ht : accepts touch ct) (he : accepts empty ce) :
    accepts (deletion touch empty) (ct && ce) := by
  rcases touch with _ | (_ | _) <;> rcases empty with _ | (_ | _) <;>
    simp_all [accepts, deletion]

theorem strong_deletion_requires_both (touch empty : Option Bool) :
    deletion touch empty = some true ↔ touch = some true ∧ empty = some true := by
  rcases touch with _ | (_ | _) <;> rcases empty with _ | (_ | _) <;>
    simp [deletion]

def zeroFlag (value : Option Nat) : Option Bool := value.map (· == 0)

def numericAccepts (fact : Option Nat) (value : Nat) : Prop :=
  match fact with
  | none => True
  | some known => value = known

theorem zeroFlag_inclusion (fact : Option Nat) (value : Nat) (h : numericAccepts fact value) :
    accepts (zeroFlag fact) (value == 0) := by
  cases fact <;> simp_all [zeroFlag, accepts, numericAccepts]

def emptyFlag (nonce balance : Option Nat) (codeEmpty : Option Bool) : Option Bool :=
  deletion codeEmpty (deletion (zeroFlag nonce) (zeroFlag balance))

/-- Missing balance cannot be silently replaced with zero to prove emptiness. -/
theorem emptyFlag_inclusion (nonce balance : Option Nat) (codeEmpty : Option Bool)
    (cn cb : Nat) (cc : Bool) (hn : numericAccepts nonce cn)
    (hb : numericAccepts balance cb) (hc : accepts codeEmpty cc) :
    accepts (emptyFlag nonce balance codeEmpty) (cc && (cn == 0 && cb == 0)) :=
  deletion_inclusion _ _ _ _ hc
    (deletion_inclusion _ _ _ _ (zeroFlag_inclusion _ _ hn) (zeroFlag_inclusion _ _ hb))

theorem unknown_balance_is_not_known_empty : emptyFlag (some 0) none (some true) = none := rfl

theorem known_nonzero_balance_is_nonempty (value : Nat) (nonzero : value ≠ 0) :
    emptyFlag none (some value) none = some false := by
  cases value with
  | zero => exact False.elim (nonzero rfl)
  | succ value => rfl

def either : Option Bool → Option Bool → Option Bool
  | some true, _ => some true
  | _, some true => some true
  | some false, some false => some false
  | _, _ => none

theorem either_inclusion (left right : Option Bool) (cl cr : Bool)
    (hl : accepts left cl) (hr : accepts right cr) : accepts (either left right) (cl || cr) := by
  rcases left with _ | (_ | _) <;> rcases right with _ | (_ | _) <;>
    simp_all [accepts, either]

def combinedDeletion (pending touch empty : Option Bool) : Option Bool :=
  either pending (deletion touch empty)

/-- Combines the existing pending-destruction path with touched-empty deletion. -/
theorem combined_deletion_inclusion (pending touch empty : Option Bool) (cp ct ce : Bool)
    (hp : accepts pending cp) (ht : accepts touch ct) (he : accepts empty ce) :
    accepts (combinedDeletion pending touch empty) (cp || (ct && ce)) :=
  either_inclusion _ _ _ _ hp (deletion_inclusion _ _ _ _ ht he)

theorem combined_strong_deletion (pending touch empty : Option Bool) :
    combinedDeletion pending touch empty = some true ↔
      pending = some true ∨ touch = some true ∧ empty = some true := by
  rcases pending with _ | (_ | _) <;> rcases touch with _ | (_ | _) <;>
    rcases empty with _ | (_ | _) <;> simp [combinedDeletion, deletion, either]

def touchPossible (previous : Option Bool) : Option Bool :=
  match previous with
  | some true => some true
  | _ => none

theorem optional_touch_inclusion (previous : Option Bool) (touched didTouch : Bool)
    (h : accepts previous touched) : accepts (touchPossible previous) (touched || didTouch) := by
  rcases previous with _ | (_ | _) <;> simp_all [accepts, touchPossible]

theorem must_touch_survives_unknown_effects : touchPossible (some true) = some true := rfl

theorem optional_touch_idempotent (previous : Option Bool) :
    touchPossible (touchPossible previous) = touchPossible previous := by
  rcases previous with _ | (_ | _) <;> rfl

structure ConcreteAccount where
  existence : Bool
  touched : Bool
  empty : Bool
  deriving DecidableEq, Repr

structure AbstractAccount where
  existence : Option Bool
  touched : Option Bool
  empty : Option Bool
  deriving DecidableEq, Repr

def Gamma (abstract : AbstractAccount) (concrete : ConcreteAccount) : Prop :=
  accepts abstract.existence concrete.existence ∧
    accepts abstract.touched concrete.touched ∧ accepts abstract.empty concrete.empty

def finishConcrete (account : ConcreteAccount) : ConcreteAccount :=
  { account with existence := account.existence && !(account.touched && account.empty), touched := false }

def finish (account : AbstractAccount) : AbstractAccount :=
  { account with
    existence := match deletion account.touched account.empty with
      | some false => account.existence
      | some true => some false
      | none => none
    touched := some false }

theorem finish_inclusion (abstract : AbstractAccount) (concrete : ConcreteAccount)
    (h : Gamma abstract concrete) : Gamma (finish abstract) (finishConcrete concrete) := by
  obtain ⟨existence, touched, empty⟩ := abstract
  obtain ⟨ce, ct, cm⟩ := concrete
  rcases existence with _ | (_ | _) <;> rcases touched with _ | (_ | _) <;>
    rcases empty with _ | (_ | _) <;> cases ce <;> cases ct <;> cases cm <;>
    simp_all [Gamma, accepts, finish, finishConcrete, deletion]

theorem untouched_preserves_existence (account : AbstractAccount)
    (untouched : account.touched = some false) : (finish account).existence = account.existence := by
  simp [finish, deletion, untouched]

theorem nonempty_preserves_existence (account : AbstractAccount)
    (nonempty : account.empty = some false) : (finish account).existence = account.existence := by
  cases h : account.touched with
  | none => simp [finish, deletion, nonempty, h]
  | some touched => cases touched <;> simp [finish, deletion, nonempty, h]

theorem touched_empty_deletes (account : AbstractAccount)
    (touch : account.touched = some true) (empty : account.empty = some true) :
    (finish account).existence = some false := by
  simp [finish, deletion, touch, empty]

theorem maybe_touched_empty_is_unknown (account : AbstractAccount)
    (touch : account.touched = none) (empty : account.empty = some true) :
    (finish account).existence = none := by
  simp [finish, deletion, touch, empty]

def markTouched (account : AbstractAccount) : AbstractAccount :=
  { account with touched := some true }

def markConcrete (account : ConcreteAccount) : ConcreteAccount :=
  { account with touched := true }

theorem markTouched_inclusion (abstract : AbstractAccount) (concrete : ConcreteAccount)
    (h : Gamma abstract concrete) : Gamma (markTouched abstract) (markConcrete concrete) := by
  exact ⟨h.1, rfl, h.2.2⟩

def applyStaticTouches (account : AbstractAccount) : AbstractAccount :=
  { account with touched := touchPossible account.touched }

def concreteOptionalTouch (account : ConcreteAccount) (didTouch : Bool) : ConcreteAccount :=
  { account with touched := account.touched || didTouch }

theorem static_touch_inclusion (abstract : AbstractAccount) (concrete : ConcreteAccount)
    (didTouch : Bool) (h : Gamma abstract concrete) :
    Gamma (applyStaticTouches abstract) (concreteOptionalTouch concrete didTouch) :=
  ⟨h.1, optional_touch_inclusion _ _ _ h.2.1, h.2.2⟩

theorem static_touch_finalization_inclusion (abstract : AbstractAccount)
    (concrete : ConcreteAccount) (didTouch : Bool) (h : Gamma abstract concrete) :
    Gamma (finish (applyStaticTouches abstract))
      (finishConcrete (concreteOptionalTouch concrete didTouch)) :=
  finish_inclusion _ _ (static_touch_inclusion _ _ _ h)

theorem static_touch_can_change_completion :
    let concrete : ConcreteAccount := ⟨true, false, true⟩
    let abstract : AbstractAccount := ⟨some true, some false, some true⟩
    (finishConcrete concrete).existence = true ∧
      (finishConcrete (concreteOptionalTouch concrete true)).existence = false ∧
      (finish (applyStaticTouches abstract)).existence = none := by
  simp [finishConcrete, concreteOptionalTouch, finish, applyStaticTouches, touchPossible, deletion]

def joinFlag (left right : Option Bool) : Option Bool :=
  if left = right then left else none

theorem joinFlag_left (left right : Option Bool) (value : Bool) (h : accepts left value) :
    accepts (joinFlag left right) value := by
  unfold joinFlag
  split
  · exact h
  · trivial

theorem joinFlag_right (left right : Option Bool) (value : Bool) (h : accepts right value) :
    accepts (joinFlag left right) value := by
  unfold joinFlag
  split
  · rename_i equal
    simpa [equal] using h
  · trivial

structure CallFrame where
  checkpoint : AbstractAccount
  current : AbstractAccount
  deriving DecidableEq, Repr

def callTouch (frame : CallFrame) : CallFrame :=
  { frame with current := markTouched frame.current }

def revertCall (owner : Nat) (frame : CallFrame) : CallFrame :=
  { frame with current := if owner = 3 then
      { frame.checkpoint with touched := either frame.checkpoint.touched frame.current.touched }
    else frame.checkpoint }

theorem call_touch_rollback (owner : Nat) (frame : CallFrame) (ordinary : owner ≠ 3) :
    (revertCall owner (callTouch frame)).current = frame.checkpoint := by
  simp [revertCall, callTouch, ordinary]

theorem rollback_then_finish_uses_checkpoint (owner : Nat) (frame : CallFrame)
    (ordinary : owner ≠ 3) :
    finish (revertCall owner (callTouch frame)).current = finish frame.checkpoint := by
  rw [call_touch_rollback owner frame ordinary]

theorem rollback_restores_account_fields (owner : Nat) (frame : CallFrame) :
    (revertCall owner frame).current.existence = frame.checkpoint.existence ∧
      (revertCall owner frame).current.empty = frame.checkpoint.empty := by
  by_cases sticky : owner = 3 <;> simp [revertCall, sticky]

theorem ripemd_touch_survives_revert (frame : CallFrame) :
    (revertCall 3 (callTouch frame)).current.touched = some true := by
  cases h : frame.checkpoint.touched with
  | none => simp [revertCall, callTouch, markTouched, either, h]
  | some touched => cases touched <;> simp [revertCall, callTouch, markTouched, either, h]

def revertConcrete (owner : Nat) (checkpoint current : ConcreteAccount) : ConcreteAccount :=
  if owner = 3 then { checkpoint with touched := checkpoint.touched || current.touched } else checkpoint

theorem rollback_inclusion (owner : Nat) (frame : CallFrame)
    (checkpoint current : ConcreteAccount) (saved : Gamma frame.checkpoint checkpoint)
    (running : Gamma frame.current current) :
    Gamma (revertCall owner frame).current (revertConcrete owner checkpoint current) := by
  by_cases sticky : owner = 3
  · simp only [revertCall, revertConcrete, if_pos sticky]
    exact ⟨saved.1, either_inclusion _ _ _ _ saved.2.1 running.2.1, saved.2.2⟩
  · simpa only [revertCall, revertConcrete, if_neg sticky] using saved

/-- Old no-touch finalization leaves a definitely touched empty account alive. -/
theorem old_no_touch_counterexample :
    let concrete : ConcreteAccount := ⟨true, true, true⟩
    let old : AbstractAccount := ⟨some true, some true, some true⟩
    Gamma old concrete ∧ ¬ Gamma old (finishConcrete concrete) ∧
      Gamma (finish old) (finishConcrete concrete) := by
  simp [Gamma, accepts, finishConcrete, finish, deletion]

#print axioms deletion_inclusion
#print axioms strong_deletion_requires_both
#print axioms zeroFlag_inclusion
#print axioms emptyFlag_inclusion
#print axioms unknown_balance_is_not_known_empty
#print axioms known_nonzero_balance_is_nonempty
#print axioms either_inclusion
#print axioms combined_deletion_inclusion
#print axioms combined_strong_deletion
#print axioms optional_touch_inclusion
#print axioms must_touch_survives_unknown_effects
#print axioms optional_touch_idempotent
#print axioms finish_inclusion
#print axioms untouched_preserves_existence
#print axioms nonempty_preserves_existence
#print axioms touched_empty_deletes
#print axioms maybe_touched_empty_is_unknown
#print axioms markTouched_inclusion
#print axioms static_touch_inclusion
#print axioms static_touch_finalization_inclusion
#print axioms static_touch_can_change_completion
#print axioms joinFlag_left
#print axioms joinFlag_right
#print axioms call_touch_rollback
#print axioms rollback_then_finish_uses_checkpoint
#print axioms rollback_restores_account_fields
#print axioms ripemd_touch_survives_revert
#print axioms rollback_inclusion
#print axioms old_no_touch_counterexample

end NoSmt.Lifecycle
