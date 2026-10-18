import NoSmt.RootEnvelope

set_option autoImplicit false

/-!
Public checked transition MODEL corresponding to
analysis::engine::terminal::prepare, ReadyRoot::publish,
WorkBudget::try_completion_escrow and CompletionPermit::publish_allowed.
Engine/boundary/session/RPC publication gates retain the abstract transition
model below. All-input scope and complete revision/context bindings are abstract;
caller Bool guards and their source observations remain manual correspondences.
Work costs are Nat logical analysis work, not EVM gas or Word256 values.
None quotes represent overflow sentinels. Prefix includes actual construction
and finalizer receipts; read-only finalizer guards are NOT charged as receipts.
This source has no escrow settlement/refund, before or after detail.

Root-indexed Ready models owned non-Clone authority from paid initial(), not a
late equality/hash check. Lean values are not linear: single use is a theorem
about this state machine. Rust ownership, admission checks, exact input/Gamma
correspondence, bound adequacy (including drops), source/revision invalidation,
actual trigger/cancellation observations and finite Store representation remain
manual obligations. Return uses FinalizedGamma for ALL roots, including static
input; it is not the old Return=True constructor primitive. RootSaved equality,
original input source identity and Gamma ownership are unchanged and read-only.
The arbitrary finite byte-list model does not prove Rust ByteArray source or
Word256-length representation, or realize lists longer than Word256 permits.
EVM semantic contracts/outer transaction rules and actual owner scoping/log
wildcard interpretation remain manual. No Rust/EVM/callback/CFG closure theorem
is asserted.
Caller guards are supplied observations; session charge(0) polling is outside
the attachment transition whose ledger is framed below.
-/
namespace NoSmt.RootCoverageCapability
open RootEnvelope

structure Binding where
  snapshot : Nat
  revision : Nat
  storageSource : Nat
  runtimeRoot : Nat
  deriving DecidableEq

/-- Identity tokens name the original capture; Gamma couples WHOLE worlds. -/
structure Original where
  binding : Binding
  gamma : JointGamma

structure Ledger where
  maximum : Nat
  used : Nat
  sticky : Bool
  payer : Nat
  deriving DecidableEq

/-- Two observations distinguish optional decline from cancellation at charge. -/
def optionalPay (b : Ledger) (quote : Option Nat) (before atCharge : Bool) : Ledger × Bool :=
  match quote with
  | none => (b, false)
  | some fee =>
    if before = true ∨ b.sticky = true ∨ b.maximum < b.used + fee then (b, false)
    else if atCharge then ({ b with sticky := true }, false)
    else ({ b with used := b.used + fee }, true)

theorem optional_decline_exact (b : Ledger) (quote : Option Nat) (before : Bool)
    (decline : (optionalPay b quote before false).2 = false) :
    (optionalPay b quote before false).1 = b := by
  cases quote with
  | none => rfl
  | some fee =>
    simp only [optionalPay] at decline ⊢
    split <;> simp_all

theorem paid_prefix_kept (b : Ledger) (spent : Nat) (quote : Option Nat) (before : Bool)
    (decline : (optionalPay { b with used := b.used + spent } quote before false).2 = false) :
    let out := (optionalPay { b with used := b.used + spent } quote before false).1
    out.used = b.used + spent ∧ out.sticky = b.sticky := by
  rw [optional_decline_exact _ _ _ decline]
  exact ⟨rfl, rfl⟩

theorem cancellation_race_keeps_paid_work (b : Ledger) (fee : Nat)
    (available : b.sticky = false ∧ b.used + fee ≤ b.maximum) :
    optionalPay b (some fee) false true = ({ b with sticky := true }, false) := by
  simp [optionalPay, available.1, Nat.not_lt.mpr available.2]

theorem successful_payment_exact (b : Ledger) (quote : Option Nat) (before atCharge : Bool)
    (success : (optionalPay b quote before atCharge).2 = true) :
    ∃ fee, quote = some fee ∧ (optionalPay b quote before atCharge).1 =
      { b with used := b.used + fee } ∧ b.used + fee ≤ b.maximum := by
  cases quote with
  | none => simp [optionalPay] at success
  | some fee =>
    by_cases refused : before = true ∨ b.sticky = true ∨ b.maximum < b.used + fee
    · simp [optionalPay, refused] at success
    · cases atCharge with
      | true => simp [optionalPay, refused] at success
      | false =>
        exact ⟨fee, rfl, by simp [optionalPay, refused],
          Nat.le_of_not_lt (fun over => refused (Or.inr (Or.inr over)))⟩

inductive Trigger where
  | work | summaryWork | states | transfers
  | mandatoryRetention (state pc : Nat) (expression : Bool)

/-- The root index is the immutable owned anchor, not the visited subdomain. -/
structure Ready (root : Original) where
  payer : Nat

structure Certificate (root : Original) where
  trigger : Trigger

def denotes {root : Original} (_ : Certificate root) : Outcome → Prop :=
  finalizedTerminalEnvelope root.gamma

/-- Called after actual paid prefix/three checked finalizers. Admission is
the source's opt-in/input/constructor checks; it is not a soundness premise. -/
def prepare (root : Original) (paid : Ledger) (admitted : Bool) (lifetime : Option Nat)
    (before atCharge : Bool) : Ledger × Option (Ready root) :=
  if admitted then
    let payment := optionalPay paid lifetime before atCharge
    (payment.1, if payment.2 then some ⟨paid.payer⟩ else none)
  else (paid, none)

theorem bad_input_no_authority (root : Original) (paid : Ledger) (lifetime : Option Nat)
    (before atCharge : Bool) : prepare root paid false lifetime before atCharge = (paid, none) := rfl

theorem ready_fully_prepaid (root : Original) (paid : Ledger) (lifetime : Option Nat)
    (admitted before atCharge : Bool) (ready : Ready root)
    (minted : (prepare root paid admitted lifetime before atCharge).2 = some ready) :
    admitted = true ∧ ready.payer = paid.payer ∧ ∃ fee, lifetime = some fee ∧
      (prepare root paid admitted lifetime before atCharge).1 =
        { paid with used := paid.used + fee } ∧ paid.used + fee ≤ paid.maximum := by
  cases admitted with
  | false => simp [prepare] at minted
  | true =>
    simp only [prepare, ↓reduceIte] at minted ⊢
    split at minted
    · rename_i funded
      have same : ready = ⟨paid.payer⟩ := (Option.some.inj minted).symm
      subst ready
      exact ⟨trivial, rfl, successful_payment_exact _ _ _ _ funded⟩
    · simp at minted

/-- Arbitrary payload: equality frames status/frontiers/SSA/observed outcomes
and any other detailed projections, without assuming they are complete. -/
structure APIState (root : Original) (Detailed : Type) where
  ledger : Ledger
  cap : Option (Ready root)
  consumed : Bool
  detail : Detailed
  coverage : Option (Certificate root)

structure Guard where
  trigger : Option Trigger
  blocked : Bool
  externalFailure : Bool
  cancelled : Nat → Bool
  currentToken : Nat

/-- One attempted transfer/disposal, including engine status/boundary gates,
session delivery cancellation and RPC suppression. The current scope is NOT
the permit's captured cancellation token; both checks retain their own identity.
The Bool records a NEW transfer, not existing immutable report authority. -/
def publish {root : Original} {D : Type} (incomplete : D → Bool) (g : Guard)
    (s : APIState root D) : APIState root D × Bool :=
  let candidate := match s.cap with
    | none => (s, false)
    | some ready =>
      let dropped := { s with cap := none, consumed := true }
      if s.consumed = false ∧ incomplete s.detail = true ∧ g.blocked = false ∧
          g.cancelled ready.payer = false then
        match g.trigger with
        | none => (dropped, false)
        | some trigger => ({ dropped with coverage := some ⟨trigger⟩ }, true)
      else (dropped, false)
  if g.externalFailure = true ∨ g.cancelled g.currentToken = true then
    ({ candidate.1 with coverage := none }, false) else candidate

theorem publication_frames {root : Original} {D : Type} (incomplete : D → Bool)
    (g : Guard) (s : APIState root D) :
    (publish incomplete g s).1.ledger = s.ledger ∧
    (publish incomplete g s).1.detail = s.detail ∧ (publish incomplete g s).1.cap = none := by
  cases cap : s.cap with
  | none =>
    simp only [publish, cap]
    split <;> simp_all
  | some ready =>
    by_cases allowed : s.consumed = false ∧ incomplete s.detail = true ∧
        g.blocked = false ∧ g.cancelled ready.payer = false
    · simp only [publish, cap, if_pos allowed]
      cases g.trigger <;> split <;> exact ⟨rfl, rfl, rfl⟩
    · simp only [publish, cap, if_neg allowed]
      split <;> exact ⟨rfl, rfl, rfl⟩

theorem one_use {root : Original} {D : Type} (incomplete : D → Bool)
    (first second : Guard) (s : APIState root D) :
    (publish incomplete second (publish incomplete first s).1).2 = false := by
  rw [publish, (publication_frames incomplete first s).2.2]
  split <;> rfl

theorem authority_requires_guards {root : Original} {D : Type} (incomplete : D → Bool)
    (g : Guard) (s : APIState root D) (transferred : (publish incomplete g s).2 = true) :
    ∃ ready, s.cap = some ready ∧ s.consumed = false ∧ incomplete s.detail = true ∧
      g.blocked = false ∧ g.externalFailure = false ∧
      g.cancelled ready.payer = false ∧ g.cancelled g.currentToken = false ∧
      g.trigger.isSome = true := by
  by_cases blocked : g.externalFailure = true ∨ g.cancelled g.currentToken = true
  · simp [publish, blocked] at transferred
  · have delivery : g.externalFailure = false ∧ g.cancelled g.currentToken = false := by
      simpa only [not_or, Bool.not_eq_true] using blocked
    cases cap : s.cap with
    | none => simp [publish, cap, blocked] at transferred
    | some ready =>
      by_cases allowed : s.consumed = false ∧ incomplete s.detail = true ∧
          g.blocked = false ∧ g.cancelled ready.payer = false
      · cases trigger : g.trigger with
        | none => simp [publish, cap, allowed, trigger, blocked] at transferred
        | some actual =>
          exact ⟨ready, rfl, allowed.1, allowed.2.1, allowed.2.2.1, delivery.1,
            allowed.2.2.2, delivery.2, rfl⟩
      · simp [publish, cap, allowed, blocked] at transferred

theorem cancelled_cannot_transfer {root : Original} {D : Type} (incomplete : D → Bool)
    (g : Guard) (s : APIState root D)
    (cancelled : ∀ ready, s.cap = some ready → g.cancelled ready.payer = true) :
    (publish incomplete g s).2 = false := by
  cases transferred : (publish incomplete g s).2 with
  | false => rfl
  | true =>
    obtain ⟨ready, cap, _, _, _, _, notCancelled, _, _⟩ :=
      authority_requires_guards incomplete g s transferred
    simp [cancelled ready cap] at notCancelled

theorem external_failure_cannot_transfer {root : Original} {D : Type}
    (incomplete : D → Bool) (g : Guard) (s : APIState root D) (failed : g.externalFailure = true) :
    (publish incomplete g s).2 = false ∧ (publish incomplete g s).1.coverage = none := by
  simp [publish, failed]

theorem current_cancel_blocks_delivery {root : Original} {D : Type} (incomplete : D → Bool)
    (g : Guard) (s : APIState root D) (cancelled : g.cancelled g.currentToken = true) :
    (publish incomplete g s).2 = false ∧ (publish incomplete g s).1.coverage = none := by
  simp [publish, cancelled]

theorem prepaid_publication_after_work {root : Original} {D : Type} (ready : Ready root)
    (paid : Ledger) (detail : D) (incomplete : D → Bool) (honest : incomplete detail = true)
    (ambient : Nat) :
    let s : APIState root D := ⟨{ paid with sticky := true }, some ready, false, detail, none⟩
    let g : Guard := ⟨some .work, false, false, (fun _ => false), ambient⟩
    (publish incomplete g s).2 = true ∧
      (publish incomplete g s).1.coverage = some ⟨.work⟩ ∧
      (publish incomplete g s).1.consumed = true ∧ (publish incomplete g s).1.ledger = s.ledger ∧
      (publish incomplete g s).1.detail = detail := by
  simp [publish, honest]

/-- Ready is local to one run. A revised input starts without old authority;
this forgetful transition models that scope, without a ledger reset/refund. -/
def revisedInput {old : Original} {D : Type} (fresh : Original) (s : APIState old D) :
    APIState fresh D := ⟨s.ledger, none, s.consumed, s.detail, none⟩

theorem no_reuse_after_injection {old : Original} {D : Type} (fresh : Original)
    (s : APIState old D) (incomplete : D → Bool) (g : Guard) :
    (publish incomplete g (revisedInput fresh s)).1.coverage = none ∧
      (publish incomplete g (revisedInput fresh s)).1.ledger = s.ledger := by
  simp only [publish, revisedInput]
  split <;> exact ⟨rfl, rfl⟩

theorem all_original_initials (root : Original) (certificate : Certificate root)
    (initial : Anchored) (member : root.gamma initial) (sticky : Bool)
    (returnedWorld : World) (returnData revertData : Bytes) :
    denotes certificate ⟨.returned, finalize returnedWorld, returnData⟩ ∧
    denotes certificate ⟨.reverted, rollbackFinish initial sticky, revertData⟩ ∧
    denotes certificate ⟨.failed, rollbackFinish initial sticky, []⟩ :=
  finalized_terminal_inclusion _ _ member _ _ _ _

/-- One saved/current pair per valuation; CURRENT is 9, SAVED is 1 or 7. -/
def valuation (value : Word256) : Anchored :=
  let saved : World := ⟨fun _ => { saved3 with balance := 1, persistent := fun _ => value }, []⟩
  let current : World := ⟨fun _ => { current3 with persistent := fun _ => 9 }, []⟩
  ⟨⟨0, saved⟩, ⟨1, saved⟩, ⟨2, current⟩⟩

def originalGamma : JointGamma := fun state => state = valuation 1 ∨ state = valuation 7
def visitedGamma : JointGamma := fun state => state = valuation 1

theorem visited_is_not_all_initial :
    (∀ state, visitedGamma state → originalGamma state) ∧
    terminalEnvelope originalGamma ⟨.failed, rollbackFinish (valuation 7) false, []⟩ ∧
    ¬ terminalEnvelope visitedGamma ⟨.failed, rollbackFinish (valuation 7) false, []⟩ := by
  refine ⟨fun _ member => Or.inl member,
    (rollback_terminal_data originalGamma (valuation 7) (Or.inr rfl) false []).2, ?_⟩
  rintro ⟨⟨state, member, choice⟩, _⟩
  change state = valuation 1 at member
  subst state
  rcases choice with same | same <;>
    have bad := congrArg (fun world => (world.accounts 4).persistent 0) same <;>
    change (7 : Word256) = 1 at bad <;> exact (by decide : (7 : Word256) ≠ 1) bad

#print axioms optional_decline_exact
#print axioms paid_prefix_kept
#print axioms cancellation_race_keeps_paid_work
#print axioms successful_payment_exact
#print axioms bad_input_no_authority
#print axioms ready_fully_prepaid
#print axioms publication_frames
#print axioms one_use
#print axioms authority_requires_guards
#print axioms cancelled_cannot_transfer
#print axioms external_failure_cannot_transfer
#print axioms current_cancel_blocks_delivery
#print axioms prepaid_publication_after_work
#print axioms no_reuse_after_injection
#print axioms all_original_initials
#print axioms visited_is_not_all_initial

end NoSmt.RootCoverageCapability
