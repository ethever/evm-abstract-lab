import NoSmt.Effects
import NoSmt.Lifecycle

set_option autoImplicit false

/-!
Public checked whole-world constructor MODEL corresponding to
analysis::engine::terminal::prepare, Store::root_terminal_all_effects_checked
and Store::finalize_transaction_checked. The model gives ALL successful roots,
including static ones, independent all-account effects and transaction-local
finalization. Revert/Failure use original RootSaved plus future owner-3 touch,
then finalization. The captured input (including is_static) remains unchanged.
Word256 reuses Effects.Word = BitVec 256; addresses are BitVec 160 and slots
are Word256, so their embeddings into the corresponding EVM indices are identity.
An incarnation is a proof-side historical label, not a new Rust/schema field.

Lifecycle and LateFailure prove three-flag/account projections. They do not
prove payload cleanup. Here deletion is one operation on a whole Account;
the SAME deletion choice clears every field, including all storage keys.
Saved/current coupling is one predicate on ONE Anchored state, never a separate
existential witness per field. The envelope is the derived image of this set
under explicit restore/sticky/finalize operations. Its inclusion theorem does
not prove that Rust's finite Store representation denotes that image.

Snapshots' immutable-read discipline is used as a design boundary; its Nat
cell-value model is not substituted for Word256 or assumed to prove this model.
Ordered finite log lists retain repetitions and captured source identity.
Bytes are mathematical finite lists: no Rust ByteArray source/Word256-length
representation proof is supplied, and lengths beyond Word256 are not claimed
realizable by Rust. Actual fresh SourceId allocation, owner scoping, omitted
account defaults and logs_unknown interpretation remain MANUAL correspondence
obligations. Whole-world assignment below is a direct mathematical extension,
not a new oracle or a proof of those implementation obligations. EVM semantic
contracts/outer transaction rules, callback/CFG closure and fee upper bounds
are outside these constructor/publication proofs.
General-finalizer correspondence, including every owner's transaction cleanup,
remains manual; root-specific tests do not prove that implementation.
-/
namespace NoSmt.RootEnvelope

abbrev Word256 := Effects.Word
abbrev Address := BitVec 160
abbrev Slot := Word256
abbrev Byte := BitVec 8
abbrev Bytes := List Byte

structure Account where
  code : Bytes
  nonce : Word256
  balance : Word256
  presence : Bool
  persistent : Slot → Word256
  transient : Slot → Word256
  created : Bool
  pending : Bool
  touched : Bool
  incarnation : Nat

structure LogSource where
  owner : Address
  codeOwner : Address
  codeHash : Word256
  init : Bool
  pc : Nat
  incarnation : Nat
  deriving DecidableEq

structure Log where
  source : LogSource
  topics : List Word256
  topicBound : topics.length ≤ 4
  data : Bytes

structure World where
  accounts : Address → Account
  logs : List Log

/-- One complete World is the witness for ALL owners, fields and ordered logs.
Only transaction-local fields are constrained; persistent storage, code, nonce,
balance, presence, incarnation and the finite log sequence are otherwise arbitrary.
There is no separate existential choice for each observation. -/
def FinalizedGamma (world : World) : Prop :=
  ∀ owner, (∀ slot, (world.accounts owner).transient slot = 0) ∧
    (world.accounts owner).created = false ∧ (world.accounts owner).pending = false ∧
    (world.accounts owner).touched = false

structure Root where
  version : Nat
  world : World

structure Anchored where
  initial : Root
  saved : Root
  current : Root

/-- An arbitrary COUPLED set of initial/saved/current states. -/
abbrev JointGamma := Anchored → Prop

def empty (account : Account) : Bool :=
  account.code.isEmpty && account.nonce == 0 && account.balance == 0

/-- The same concrete deletion event resets the entire owner payload. -/
def delete (account : Account) : Account :=
  { account with
    code := []
    nonce := 0
    balance := 0
    presence := false
    persistent := fun _ => 0
    transient := fun _ => 0 }

def clearTransaction (account : Account) : Account :=
  { account with transient := fun _ => 0, created := false, pending := false, touched := false }

def shouldDelete (account : Account) : Bool :=
  account.pending || (account.touched && empty account)

/-- Existing flag soundness supplies the decision; payload reset is proved below. -/
theorem deletion_flag_inclusion (pending touch emptyFlag : Option Bool) (account : Account)
    (hp : Lifecycle.accepts pending account.pending)
    (ht : Lifecycle.accepts touch account.touched)
    (he : Lifecycle.accepts emptyFlag (empty account)) :
    Lifecycle.accepts (Lifecycle.combinedDeletion pending touch emptyFlag)
      (shouldDelete account) :=
  Lifecycle.combined_deletion_inclusion _ _ _ _ _ _ hp ht he

def finishAccount (account : Account) : Account :=
  clearTransaction (if shouldDelete account then delete account else account)

def finalize (world : World) : World :=
  { world with accounts := fun owner => finishAccount (world.accounts owner) }

def restore (saved current : World) (futureSticky : Bool) : World :=
  { saved with accounts := fun owner =>
      if owner = 3 then
        { saved.accounts owner with touched :=
          (saved.accounts owner).touched || (current.accounts owner).touched || futureSticky }
      else saved.accounts owner }

def rollbackFinish (state : Anchored) (futureSticky : Bool) : World :=
  finalize (restore state.saved.world state.current.world futureSticky)

/-- One future-sticky choice shared across every field of the whole world. -/
def rollbackEnvelope (gamma : JointGamma) (world : World) : Prop :=
  ∃ state, gamma state ∧
    (world = rollbackFinish state false ∨ world = rollbackFinish state true)

theorem rollback_inclusion (gamma : JointGamma) (state : Anchored)
    (member : gamma state) (futureSticky : Bool) :
    rollbackEnvelope gamma (rollbackFinish state futureSticky) := by
  refine ⟨state, member, ?_⟩
  cases futureSticky
  · exact Or.inl rfl
  · exact Or.inr rfl

theorem rollback_same_world_witness (gamma : JointGamma) (world : World)
    (member : rollbackEnvelope gamma world) :
    ∃ state, gamma state ∧ ∃ futureSticky, world = rollbackFinish state futureSticky := by
  obtain ⟨state, valid, left | right⟩ := member
  · exact ⟨state, valid, false, left⟩
  · exact ⟨state, valid, true, right⟩

theorem ordinary_owner_restore (saved current : World) (futureSticky : Bool)
    (owner : Address) (ordinary : owner ≠ 3) :
    (restore saved current futureSticky).accounts owner = saved.accounts owner := by
  simp only [restore, if_neg ordinary]

theorem restore_keeps_saved_payload (saved current : World) (futureSticky : Bool) :
    let account := (restore saved current futureSticky).accounts 3
    account.code = (saved.accounts 3).code ∧
    account.nonce = (saved.accounts 3).nonce ∧
    account.balance = (saved.accounts 3).balance ∧
    account.presence = (saved.accounts 3).presence ∧
    account.persistent = (saved.accounts 3).persistent ∧
    account.transient = (saved.accounts 3).transient ∧
    account.created = (saved.accounts 3).created ∧
    account.pending = (saved.accounts 3).pending ∧
    account.incarnation = (saved.accounts 3).incarnation ∧
    (restore saved current futureSticky).logs = saved.logs := by
  simp [restore]

theorem cleanup_all_owner_fields (world : World) (owner : Address)
    (deleted : shouldDelete (world.accounts owner) = true) :
    let out := (finalize world).accounts owner
    out.code = [] ∧ out.nonce = 0 ∧ out.balance = 0 ∧ out.presence = false ∧
    (∀ slot, out.persistent slot = 0) ∧ (∀ slot, out.transient slot = 0) ∧
    out.created = false ∧ out.pending = false ∧ out.touched = false := by
  simp [finalize, finishAccount, deleted, delete, clearTransaction]

theorem transaction_cleanup (world : World) (owner : Address) :
    let out := (finalize world).accounts owner
    (∀ slot, out.transient slot = 0) ∧ out.created = false ∧
      out.pending = false ∧ out.touched = false := by
  simp [finalize, finishAccount, clearTransaction]

/-- Derived from whole-account operations, not an assumed finalization contract. -/
theorem finalize_in_finalized_gamma (world : World) : FinalizedGamma (finalize world) := by
  intro owner
  exact transaction_cleanup world owner

theorem finalization_keeps_ordered_logs (world : World) :
    (finalize world).logs = world.logs := rfl

def withSticky (world : World) (futureSticky : Bool) : World :=
  { world with accounts := fun owner =>
      if owner = 3 then
        { world.accounts owner with touched := (world.accounts owner).touched || futureSticky }
      else world.accounts owner }

/-- Deliberately wrong candidate: finalizes CURRENT instead of restoring SAVED. -/
def naiveEnvelope (current out : World) : Prop :=
  out = finalize (withSticky current false) ∨ out = finalize (withSticky current true)

def saved3 : Account :=
  ⟨[], 0, 0, true, (fun _ => 7), (fun _ => 0), false, false, false, 1⟩

def current3 : Account := { saved3 with balance := 1 }

def savedWorld : World := ⟨fun _ => saved3, []⟩
def currentWorld : World := ⟨fun _ => current3, []⟩
def fixtureAnchor : Anchored := ⟨⟨0, savedWorld⟩, ⟨1, savedWorld⟩, ⟨2, currentWorld⟩⟩

theorem storage_does_not_prevent_emptiness :
    empty saved3 = true ∧ saved3.persistent 0 = 7 := by decide

theorem naive_current_counterexample :
    ((rollbackFinish fixtureAnchor true).accounts 3).presence = false ∧
    ((rollbackFinish fixtureAnchor true).accounts 3).persistent 0 = 0 ∧
    ¬ naiveEnvelope currentWorld (rollbackFinish fixtureAnchor true) := by
  refine ⟨by decide, by decide, ?_⟩
  intro candidate
  rcases candidate with same | same
  · have contradiction := congrArg (fun world => (world.accounts 3).presence) same
    have bad : (false : Bool) = true := contradiction
    cases bad
  · have contradiction := congrArg (fun world => (world.accounts 3).presence) same
    have bad : (false : Bool) = true := contradiction
    cases bad

theorem saved_future_join_includes_both :
    rollbackEnvelope (fun state => state = fixtureAnchor) (rollbackFinish fixtureAnchor false) ∧
    rollbackEnvelope (fun state => state = fixtureAnchor) (rollbackFinish fixtureAnchor true) ∧
    ((rollbackFinish fixtureAnchor false).accounts 3).presence = true ∧
    ((rollbackFinish fixtureAnchor false).accounts 3).persistent 0 = 7 ∧
    ((rollbackFinish fixtureAnchor true).accounts 3).presence = false ∧
    ((rollbackFinish fixtureAnchor true).accounts 3).persistent 0 = 0 := by
  exact ⟨rollback_inclusion (fun state => state = fixtureAnchor) fixtureAnchor rfl false,
    rollback_inclusion (fun state => state = fixtureAnchor) fixtureAnchor rfl true,
    by decide, by decide, by decide, by decide⟩

/-- Static touches change no payload BEFORE transaction cleanup. -/
def staticTouches (entry : World) (touch : Address → Bool) : World :=
  { entry with accounts := fun owner =>
      { entry.accounts owner with touched := (entry.accounts owner).touched || touch owner } }

theorem static_cleanup_can_change_existence :
    ((finalize (staticTouches savedWorld (fun _ => true))).accounts 3).presence = false ∧
    ((finalize (staticTouches savedWorld (fun _ => true))).accounts 3).persistent 0 = 0 := by
  decide

/-- Wrong finite-owner candidate: both possible touch and cleanup visit only
listed owners. Unlisted owners keep their complete entry account. -/
def naiveFiniteStaticFinish (listed : List Address) (entry : World)
    (touch : Address → Bool) : World :=
  { entry with accounts := fun owner =>
      if owner ∈ listed then finishAccount ((staticTouches entry touch).accounts owner)
      else entry.accounts owner }

/-- C = 0x999 is ordinary and absent from the finite list, not the sticky owner 3.
Its entry is Present/code[]/nonce0/balance0/storage7. Static execution preserves
payload BEFORE cleanup; one deletion then clears its COMPLETE account payload.
Exact account equalities expose the mismatch, including every persistent slot. -/
theorem unlisted_static_cleanup_counterexample :
    let owner : Address := 0x999
    let listed : List Address := [3]
    let touch : Address → Bool := fun address => address == owner
    let before := savedWorld
    let touched := staticTouches before touch
    let after := finalize touched
    let naive := naiveFiniteStaticFinish listed before touch
    owner ≠ 3 ∧ owner ∉ listed ∧ empty (before.accounts owner) = true ∧
    (before.accounts owner).presence = true ∧ (before.accounts owner).persistent 8 = 7 ∧
    touched.accounts owner = { before.accounts owner with touched := true } ∧
    after.accounts owner = clearTransaction (delete (before.accounts owner)) ∧
    naive.accounts owner = before.accounts owner ∧
    (after.accounts owner).presence = false ∧
    (∀ slot, (after.accounts owner).persistent slot = 0) ∧
    (naive.accounts owner).persistent 8 = 7 ∧ after ≠ naive := by
  refine ⟨by decide, by decide, by decide, by decide, by decide,
    rfl, rfl, rfl, by decide, (fun _ => rfl), by decide, ?_⟩
  intro same
  have bad := congrArg (fun world => (world.accounts (0x999 : Address)).presence) same
  change (false : Bool) = true at bad
  cases bad

inductive Kind where
  | returned | reverted | failed

structure Outcome where
  kind : Kind
  world : World
  data : Bytes

def terminalEnvelope (gamma : JointGamma) : Outcome → Prop
  | ⟨.returned, _, _⟩ => True
  | ⟨.reverted, world, _⟩ => rollbackEnvelope gamma world
  | ⟨.failed, world, data⟩ => rollbackEnvelope gamma world ∧ data = []

/-- Old wide constructor primitive only; NOT the finalized relation or Rust Gamma. -/
theorem return_top_contains (gamma : JointGamma) (world : World) (data : Bytes) :
    terminalEnvelope gamma ⟨.returned, world, data⟩ := True.intro

theorem rollback_terminal_data (gamma : JointGamma) (state : Anchored)
    (member : gamma state) (futureSticky : Bool) (revertData : Bytes) :
    terminalEnvelope gamma ⟨.reverted, rollbackFinish state futureSticky, revertData⟩ ∧
    terminalEnvelope gamma ⟨.failed, rollbackFinish state futureSticky, []⟩ :=
  ⟨rollback_inclusion _ _ member _, rollback_inclusion _ _ member _, rfl⟩

/-- The certificate relation requires finalized worlds in EVERY branch.
Return's all-account witness is independent of the old input valuation; rollback
still uses one original coupled state and one shared sticky choice. -/
def finalizedTerminalEnvelope (gamma : JointGamma) : Outcome → Prop
  | ⟨.returned, world, _⟩ => FinalizedGamma world
  | ⟨.reverted, world, _⟩ => rollbackEnvelope gamma world ∧ FinalizedGamma world
  | ⟨.failed, world, data⟩ => rollbackEnvelope gamma world ∧ FinalizedGamma world ∧ data = []

theorem finalized_terminal_inclusion (gamma : JointGamma) (state : Anchored)
    (member : gamma state) (futureSticky : Bool) (returnWorld : World)
    (returnData revertData : Bytes) :
    finalizedTerminalEnvelope gamma ⟨.returned, finalize returnWorld, returnData⟩ ∧
    finalizedTerminalEnvelope gamma ⟨.reverted, rollbackFinish state futureSticky, revertData⟩ ∧
    finalizedTerminalEnvelope gamma ⟨.failed, rollbackFinish state futureSticky, []⟩ :=
  ⟨finalize_in_finalized_gamma returnWorld,
    ⟨rollback_inclusion _ _ member _, finalize_in_finalized_gamma _⟩,
    ⟨rollback_inclusion _ _ member _, finalize_in_finalized_gamma _, rfl⟩⟩

structure Completed where
  input : Anchored
  finalRoot : Root

def complete (input : Anchored) (freshVersion : Nat) (futureSticky : Bool) : Completed :=
  ⟨input, ⟨freshVersion, rollbackFinish input futureSticky⟩⟩

structure HistoricalRead where
  root : Root
  owner : Address
  slot : Slot

def HistoricalRead.read (source : HistoricalRead) : Word256 :=
  (source.root.world.accounts source.owner).persistent source.slot

/-- A new root has a new version; lookup by an old version keeps its interpretation. -/
def installRoot (roots : Nat → Option World) (fresh : Root) : Nat → Option World :=
  fun version => if version = fresh.version then some fresh.world else roots version

theorem old_version_interpretation_preserved (roots : Nat → Option World) (fresh : Root)
    (old : Root) (freshVersion : old.version ≠ fresh.version)
    (known : roots old.version = some old.world) :
    installRoot roots fresh old.version = some old.world := by
  simp only [installRoot, if_neg freshVersion, known]

/-- Direct all-account source assignment: ONE complete finalized world is stored
at the fresh index; equality preserves EVERY old-world observation, including
logs and saved/initial reads. Freshness is an explicit premise to this map lemma;
Rust fresh SourceId allocation and scoped source interpretation remain manual. -/
theorem finalized_source_extension (roots : Nat → Option World) (freshVersion : Nat)
    (world : World) (fresh : roots freshVersion = none) :
    let output := finalize world
    let extended := installRoot roots ⟨freshVersion, output⟩
    FinalizedGamma output ∧ extended freshVersion = some output ∧
      ∀ old oldWorld, roots old = some oldWorld → extended old = some oldWorld := by
  refine ⟨finalize_in_finalized_gamma world, ?_, ?_⟩
  · simp only [installRoot, ↓reduceIte]
  · intro old oldWorld known
    have different : old ≠ freshVersion := by
      intro same
      rw [same, fresh] at known
      cases known
    exact old_version_interpretation_preserved roots ⟨freshVersion, finalize world⟩
      ⟨old, oldWorld⟩ different known

/-- Fresh result identity does not rebind the initial, saved or current roots. -/
theorem fresh_final_root_non_rebinding (input : Anchored) (freshVersion : Nat)
    (futureSticky : Bool) (initialFresh : freshVersion ≠ input.initial.version)
    (savedFresh : freshVersion ≠ input.saved.version)
    (currentFresh : freshVersion ≠ input.current.version) (oldRead : HistoricalRead) :
    let out := complete input freshVersion futureSticky
    out.input.initial = input.initial ∧ out.input.saved = input.saved ∧
    out.input.current = input.current ∧
    out.finalRoot.version ≠ input.initial.version ∧
    out.finalRoot.version ≠ input.saved.version ∧
    out.finalRoot.version ≠ input.current.version ∧
    ({ oldRead with root := out.input.saved } : HistoricalRead).read =
      ({ oldRead with root := input.saved } : HistoricalRead).read :=
  ⟨rfl, rfl, rfl, initialFresh, savedFresh, currentFresh, rfl⟩

#print axioms deletion_flag_inclusion
#print axioms rollback_inclusion
#print axioms rollback_same_world_witness
#print axioms ordinary_owner_restore
#print axioms restore_keeps_saved_payload
#print axioms cleanup_all_owner_fields
#print axioms transaction_cleanup
#print axioms finalize_in_finalized_gamma
#print axioms finalization_keeps_ordered_logs
#print axioms storage_does_not_prevent_emptiness
#print axioms naive_current_counterexample
#print axioms saved_future_join_includes_both
#print axioms static_cleanup_can_change_existence
#print axioms unlisted_static_cleanup_counterexample
#print axioms return_top_contains
#print axioms rollback_terminal_data
#print axioms finalized_terminal_inclusion
#print axioms old_version_interpretation_preserved
#print axioms finalized_source_extension
#print axioms fresh_final_root_non_rebinding

end NoSmt.RootEnvelope
