import Std

set_option autoImplicit false

/-!
A field projection of the unknown-world envelope. Known executable owners keep
code/existence/creation identity. Storage is protected except at a delegated
writable owner; all balances are forgotten. Pending destruction at a delegated
owner is always forgotten here (production may retain it for a definitely
preexisting owner). Touchedness, logs and transaction finalization are separate
models. Static execution preserves this field projection while optional touch
is modeled by Lifecycle; this is not full-Store identity.

Callbacks have explicit executable semantics: storage set/add, sequencing and
indirect calls. Their transfer functions are proved below rather than assumed.
The trace theorem quantifies over every finite trace and callback syntax tree;
it makes no claim that a Rust worklist has enumerated every such trace.
-/
namespace NoSmt.Effects

abbrev Owner := Nat

inductive Cell where
  | persistent (owner slot : Nat)
  | transient (owner slot : Nat)
  | balance (owner : Nat)
  | code (owner : Nat)
  | nonce (owner : Nat)
  | existence (owner : Nat)
  | created (owner : Nat)
  | pending (owner : Nat)
  deriving DecidableEq, Repr

abbrev Word := BitVec 256
abbrev Concrete := Cell → Word
abbrev Abstract := Cell → Option Word

def maxWord : Word := BitVec.allOnes 256

theorem word_add_modulus (left right : Word) :
    (left + right).toNat = (left.toNat + right.toNat) % 2 ^ 256 := BitVec.toNat_add left right

theorem maxword_plus_one_is_zero : maxWord + 1 = (0 : Word) := by
  decide

structure Policy where
  known : List Owner
  writable : Option Owner
  isStatic : Bool
  deriving DecidableEq

def keeps (policy : Policy) (cell : Cell) : Bool :=
  policy.isStatic || match cell with
  | .persistent owner _ | .transient owner _ | .nonce owner | .pending owner =>
    policy.known.contains owner && policy.writable != some owner
  | .code owner | .existence owner | .created owner => policy.known.contains owner
  | .balance _ => false

def Gamma (abstract : Abstract) (concrete : Concrete) : Prop :=
  ∀ cell value, abstract cell = some value → concrete cell = value

def exactState (state : Concrete) : Abstract := fun cell => some (state cell)

def havoc (policy : Policy) (state : Abstract) : Abstract :=
  fun cell => if keeps policy cell then state cell else none

def setConcrete (state : Concrete) (cell : Cell) (value : Word) : Concrete :=
  fun query => if query = cell then value else state query

def setAbstract (state : Abstract) (cell : Cell) (value : Word) : Abstract :=
  fun query => if query = cell then some value else state query

def addAbstract (state : Abstract) (cell : Cell) (delta : Word) : Abstract :=
  fun query => if query = cell then (state cell).map (· + delta) else state query

theorem exact_inclusion (state : Concrete) : Gamma (exactState state) state := by
  intro cell value h
  exact Option.some.inj h

theorem havoc_inclusion (policy : Policy) (abstract : Abstract) (concrete : Concrete)
    (h : Gamma abstract concrete) : Gamma (havoc policy abstract) concrete := by
  intro cell value fact
  unfold havoc at fact
  split at fact
  · exact h cell value fact
  · contradiction

theorem static_field_projection_identity (policy : Policy) (state : Abstract)
    (static : policy.isStatic = true) : havoc policy state = state := by
  funext cell
  simp [havoc, keeps, static]

theorem havoc_idempotent (policy : Policy) (state : Abstract) :
    havoc policy (havoc policy state) = havoc policy state := by
  funext cell
  cases h : keeps policy cell <;> simp [havoc, h]

theorem set_inclusion (abstract : Abstract) (concrete : Concrete) (cell : Cell) (value : Word)
    (h : Gamma abstract concrete) :
    Gamma (setAbstract abstract cell value) (setConcrete concrete cell value) := by
  intro query result fact
  by_cases equal : query = cell
  · simp [setAbstract, equal] at fact
    simp [setConcrete, equal, fact]
  · simp [setAbstract, equal] at fact
    simpa [setConcrete, equal] using h query result fact

theorem add_inclusion (abstract : Abstract) (concrete : Concrete) (cell : Cell) (delta : Word)
    (h : Gamma abstract concrete) :
    Gamma (addAbstract abstract cell delta)
      (setConcrete concrete cell (concrete cell + delta)) := by
  intro query result fact
  by_cases equal : query = cell
  · subst query
    simp only [addAbstract] at fact
    cases known : abstract cell with
    | none => simp [known] at fact
    | some value =>
      have original := h cell value known
      simp [known] at fact
      simp [setConcrete, original, fact.symm]
  · simp only [addAbstract, if_neg equal] at fact
    simpa [setConcrete, equal] using h query result fact

/-- Any one permitted local write is covered without executing that write abstractly. -/
theorem local_write_inclusion (policy : Policy) (abstract : Abstract) (concrete : Concrete)
    (cell : Cell) (value : Word) (allowed : keeps policy cell = false)
    (h : Gamma abstract concrete) :
    Gamma (havoc policy abstract) (setConcrete concrete cell value) := by
  intro query result fact
  by_cases equal : query = cell
  · subst query
    simp [havoc, allowed] at fact
  · have old := havoc_inclusion policy abstract concrete h query result fact
    simpa [setConcrete, equal] using old

/-- Reapplying unknown local effects cannot escape an already applied envelope. -/
theorem local_havoc_closed (policy : Policy) (abstract : Abstract) (concrete : Concrete)
    (cell : Cell) (value : Word) (allowed : keeps policy cell = false)
    (h : Gamma (havoc policy abstract) concrete) :
    Gamma (havoc policy abstract) (setConcrete concrete cell value) := by
  have step := local_write_inclusion policy (havoc policy abstract) concrete cell value allowed h
  simpa [havoc_idempotent] using step

def localRun : List (Cell × Word) → Concrete → Concrete
  | [], state => state
  | (cell, value) :: rest, state => localRun rest (setConcrete state cell value)

/-- Arbitrarily many local unknown writes remain inside one applied envelope. -/
theorem arbitrary_local_trace_closed (policy : Policy) (abstract : Abstract)
    (trace : List (Cell × Word)) (concrete : Concrete)
    (allowed : ∀ cell value, (cell, value) ∈ trace → keeps policy cell = false)
    (h : Gamma (havoc policy abstract) concrete) :
    Gamma (havoc policy abstract) (localRun trace concrete) := by
  induction trace generalizing concrete with
  | nil => exact h
  | cons entry rest ih =>
    obtain ⟨cell, value⟩ := entry
    exact ih _ (fun c v mem => allowed c v (List.mem_cons_of_mem _ mem))
      (local_havoc_closed policy abstract concrete cell value (allowed cell value (by simp)) h)

inductive Callback where
  | skip
  | store (slot : Nat) (value : Word)
  | add (slot : Nat) (delta : Word)
  | seq (first second : Callback)
  | call (target : Owner) (body : Callback)
  deriving DecidableEq, Repr

def Callback.run (owner : Owner) : Callback → Concrete → Concrete
  | .skip, state => state
  | .store slot value, state => setConcrete state (.persistent owner slot) value
  | .add slot delta, state =>
    setConcrete state (.persistent owner slot) (state (.persistent owner slot) + delta)
  | .seq first second, state => second.run owner (first.run owner state)
  | .call target body, state => body.run target state

def Callback.abstractRun (owner : Owner) : Callback → Abstract → Abstract
  | .skip, state => state
  | .store slot value, state => setAbstract state (.persistent owner slot) value
  | .add slot delta, state => addAbstract state (.persistent owner slot) delta
  | .seq first second, state => second.abstractRun owner (first.abstractRun owner state)
  | .call target body, state => body.abstractRun target state

def Callback.Known (known : List Owner) : Callback → Prop
  | .skip | .store _ _ | .add _ _ => True
  | .seq first second => first.Known known ∧ second.Known known
  | .call target body => target ∈ known ∧ body.Known known

def Callback.ReadOnly : Callback → Prop
  | .skip => True
  | .store _ _ | .add _ _ => False
  | .seq first second => first.ReadOnly ∧ second.ReadOnly
  | .call _ body => body.ReadOnly

theorem readonly_callback_field_identity (body : Callback) (owner : Owner)
    (state : Abstract) (readonly : body.ReadOnly) : body.abstractRun owner state = state := by
  induction body generalizing owner state with
  | skip => rfl
  | store slot value => contradiction
  | add slot delta => contradiction
  | seq first second ihFirst ihSecond =>
    exact (ihSecond _ _ readonly.2).trans (ihFirst _ _ readonly.1)
  | call target body ih => exact ih _ _ readonly

theorem callback_inclusion (body : Callback) (owner : Owner)
    (abstract : Abstract) (concrete : Concrete) (h : Gamma abstract concrete) :
    Gamma (body.abstractRun owner abstract) (body.run owner concrete) := by
  induction body generalizing owner abstract concrete with
  | skip => exact h
  | store slot value => exact set_inclusion _ _ _ _ h
  | add slot delta => exact add_inclusion _ _ _ _ h
  | seq first second ihFirst ihSecond => exact ihSecond _ _ _ (ihFirst _ _ _ h)
  | call target body ih => exact ih _ _ _ h

inductive Event where
  | localWrite (cell : Cell) (value : Word)
  | callback (owner : Owner) (body : Callback)
  deriving DecidableEq, Repr

def Event.Allowed (policy : Policy) : Event → Prop
  | .localWrite cell _ => keeps policy cell = false
  | .callback owner body => owner ∈ policy.known ∧ body.Known policy.known ∧
      (policy.isStatic = true → body.ReadOnly)

def Event.run : Event → Concrete → Concrete
  | .localWrite cell value, state => setConcrete state cell value
  | .callback owner body, state => body.run owner state

def Event.abstractRun (policy : Policy) : Event → Abstract → Abstract
  | .localWrite _ _, state => havoc policy state
  | .callback owner body, state => body.abstractRun owner state

def runConcrete : List Event → Concrete → Concrete
  | [], state => state
  | event :: rest, state => runConcrete rest (event.run state)

def runAbstract (policy : Policy) : List Event → Abstract → Abstract
  | [], state => state
  | event :: rest, state => runAbstract policy rest (event.abstractRun policy state)

theorem event_inclusion (policy : Policy) (event : Event) (abstract : Abstract)
    (concrete : Concrete) (allowed : event.Allowed policy) (h : Gamma abstract concrete) :
    Gamma (event.abstractRun policy abstract) (event.run concrete) := by
  cases event with
  | localWrite cell value => exact local_write_inclusion _ _ _ _ _ allowed h
  | callback owner body => exact callback_inclusion _ _ _ _ h

theorem arbitrary_finite_trace_inclusion (policy : Policy) (trace : List Event)
    (abstract : Abstract) (concrete : Concrete)
    (allowed : ∀ event ∈ trace, event.Allowed policy) (h : Gamma abstract concrete) :
    Gamma (runAbstract policy trace abstract) (runConcrete trace concrete) := by
  induction trace generalizing abstract concrete with
  | nil => exact h
  | cons event rest ih =>
    exact ih _ _ (fun next mem => allowed next (List.mem_cons_of_mem event mem))
      (event_inclusion policy event abstract concrete (allowed event (by simp)) h)

theorem static_event_field_identity (policy : Policy) (event : Event) (state : Abstract)
    (static : policy.isStatic = true) (allowed : event.Allowed policy) :
    event.abstractRun policy state = state := by
  cases event with
  | localWrite cell value => simp [Event.Allowed, keeps, static] at allowed
  | callback owner body => exact readonly_callback_field_identity body owner state (allowed.2.2 static)

theorem static_trace_field_identity (policy : Policy) (trace : List Event) (state : Abstract)
    (static : policy.isStatic = true) (allowed : ∀ event ∈ trace, event.Allowed policy) :
    runAbstract policy trace state = state := by
  induction trace generalizing state with
  | nil => rfl
  | cons event rest ih =>
    rw [runAbstract, static_event_field_identity policy event state static (allowed event (by simp))]
    exact ih state (fun next mem => allowed next (List.mem_cons_of_mem event mem))

theorem zero_callbacks (policy : Policy) (abstract : Abstract) :
    runAbstract policy [] abstract = abstract := rfl

theorem known_storage_preserved (known : List Owner) (owner slot : Nat) (state : Abstract)
    (member : owner ∈ known) :
    havoc ⟨known, none, false⟩ state (.persistent owner slot) = state (.persistent owner slot) := by
  simp [havoc, keeps, member]

theorem delegated_storage_forgotten (known : List Owner) (owner slot : Nat) (state : Abstract) :
    havoc ⟨known, some owner, false⟩ state (.persistent owner slot) = none := by
  simp [havoc, keeps]

theorem delegated_code_preserved (known : List Owner) (owner : Nat) (state : Abstract)
    (member : owner ∈ known) :
    havoc ⟨known, some owner, false⟩ state (.code owner) = state (.code owner) := by
  simp [havoc, keeps, member]

theorem delegated_transient_forgotten (known : List Owner) (owner slot : Nat) (state : Abstract) :
    havoc ⟨known, some owner, false⟩ state (.transient owner slot) = none := by
  simp [havoc, keeps]

theorem all_balances_forgotten (known : List Owner) (writable : Option Owner)
    (owner : Nat) (state : Abstract) :
    havoc ⟨known, writable, false⟩ state (.balance owner) = none := rfl

theorem delegated_nonce_forgotten (known : List Owner) (owner : Nat) (state : Abstract) :
    havoc ⟨known, some owner, false⟩ state (.nonce owner) = none := by
  simp [havoc, keeps]

def fixturePolicy : Policy := ⟨[7, 9], none, false⟩
def fixtureTrace : List Event :=
  [.localWrite (.persistent 8 0) 91,
   .callback 7 (.add 0 3),
   .callback 9 (.call 7 (.seq (.add 0 3) (.call 7 (.add 0 3))))]

theorem fixture_allowed : ∀ event ∈ fixtureTrace, event.Allowed fixturePolicy := by
  intro event member
  simp only [fixtureTrace, List.mem_cons, List.not_mem_nil, or_false] at member
  rcases member with rfl | rfl | rfl <;>
    simp [Event.Allowed, keeps, fixturePolicy, Callback.Known]

theorem repeated_indirect_callbacks :
    runAbstract fixturePolicy fixtureTrace (fun _ => some 10) (.persistent 7 0) = some 19 ∧
    runConcrete fixtureTrace (fun _ => 10) (.persistent 7 0) = 19 ∧
    runAbstract fixturePolicy fixtureTrace (fun _ => some 10) (.persistent 8 0) = none := by
  decide

theorem fixture_full_inclusion :
    Gamma (runAbstract fixturePolicy fixtureTrace (exactState (fun _ => 10)))
      (runConcrete fixtureTrace (fun _ => 10)) :=
  arbitrary_finite_trace_inclusion _ _ _ _ fixture_allowed (exact_inclusion _)

theorem wraparound_callback_storage :
    let body := Callback.add 0 1
    body.abstractRun 7 (fun _ => some maxWord) (.persistent 7 0) = some 0 ∧
      body.run 7 (fun _ => maxWord) (.persistent 7 0) = 0 := by
  change some (maxWord + 1) = some (0 : Word) ∧ maxWord + 1 = 0
  exact ⟨congrArg some maxword_plus_one_is_zero, maxword_plus_one_is_zero⟩

theorem callback_preserves_code (body : Callback) (owner codeOwner : Owner) (state : Abstract) :
    body.abstractRun owner state (.code codeOwner) = state (.code codeOwner) := by
  induction body generalizing owner state with
  | skip => rfl
  | store slot value => simp [Callback.abstractRun, setAbstract]
  | add slot delta => simp [Callback.abstractRun, addAbstract]
  | seq first second ihFirst ihSecond =>
    exact (ihSecond _ _).trans (ihFirst _ _)
  | call target body ih => exact ih _ _

theorem wraparound_callback_inclusion :
    Gamma ((Callback.add 0 1).abstractRun 7 (fun _ => some maxWord))
      ((Callback.add 0 1).run 7 (fun _ => maxWord)) :=
  callback_inclusion _ _ _ _ (exact_inclusion _)

#print axioms word_add_modulus
#print axioms maxword_plus_one_is_zero
#print axioms exact_inclusion
#print axioms havoc_inclusion
#print axioms static_field_projection_identity
#print axioms havoc_idempotent
#print axioms set_inclusion
#print axioms add_inclusion
#print axioms local_write_inclusion
#print axioms local_havoc_closed
#print axioms arbitrary_local_trace_closed
#print axioms callback_inclusion
#print axioms readonly_callback_field_identity
#print axioms event_inclusion
#print axioms arbitrary_finite_trace_inclusion
#print axioms static_event_field_identity
#print axioms static_trace_field_identity
#print axioms zero_callbacks
#print axioms known_storage_preserved
#print axioms delegated_storage_forgotten
#print axioms delegated_code_preserved
#print axioms delegated_transient_forgotten
#print axioms all_balances_forgotten
#print axioms delegated_nonce_forgotten
#print axioms fixture_allowed
#print axioms repeated_indirect_callbacks
#print axioms fixture_full_inclusion
#print axioms wraparound_callback_storage
#print axioms wraparound_callback_inclusion
#print axioms callback_preserves_code

end NoSmt.Effects
