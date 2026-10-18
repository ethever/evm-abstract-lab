import Std

set_option autoImplicit false

/-!
Immutable initial observations and ordered storage writes. The finite observation
map checks consistency per owner/cell; account-wide cross-field validation and
RPC authenticity are separate production obligations. A nonexistent observation
is a distinct tag, never the absence of an observation or a known zero word.
-/
namespace NoSmt.Snapshots

structure SnapshotId where
  chain : Nat
  blockHash : Nat
  deriving DecidableEq, Repr

variable {snapshot : SnapshotId}

structure Owner where
  address : Nat
  deriving DecidableEq, Repr

structure CellKey where
  owner : Owner
  slot : Nat
  deriving DecidableEq, Repr

inductive InitialValue where
  | nonexistent
  | word (value : Nat)
  deriving DecidableEq, Repr

structure Observation (snapshot : SnapshotId) where
  key : CellKey
  value : InitialValue
  deriving DecidableEq

structure InitialRoot (snapshot : SnapshotId) where
  entries : List (CellKey × InitialValue)
  deriving DecidableEq

def lookupEntries : List (CellKey × InitialValue) → CellKey → Option InitialValue
  | [], _ => none
  | (key, value) :: rest, query =>
    if query = key then some value else lookupEntries rest query

def lookup (root : InitialRoot snapshot) (key : CellKey) : Option InitialValue :=
  lookupEntries root.entries key

def observe (root : InitialRoot snapshot) (fact : Observation snapshot) :
    Option (InitialRoot snapshot) :=
  match lookup root fact.key with
  | none => some ⟨(fact.key, fact.value) :: root.entries⟩
  | some old => if old = fact.value then some root else none

theorem observe_conflict (root : InitialRoot snapshot) (fact : Observation snapshot)
    (old : InitialValue) (known : lookup root fact.key = some old)
    (conflict : old ≠ fact.value) : observe root fact = none := by
  simp [observe, known, conflict]

theorem observe_idempotent (root : InitialRoot snapshot) (fact : Observation snapshot)
    (known : lookup root fact.key = some fact.value) : observe root fact = some root := by
  simp [observe, known]

theorem observe_installs (root next : InitialRoot snapshot) (fact : Observation snapshot)
    (success : observe root fact = some next) : lookup next fact.key = some fact.value := by
  unfold observe at success
  split at success
  · cases success
    simp [lookup, lookupEntries]
  · rename_i old known
    split at success
    · rename_i equal
      cases success
      simpa [equal] using known
    · contradiction

theorem observe_other (root next : InitialRoot snapshot) (fact : Observation snapshot)
    (key : CellKey) (different : key ≠ fact.key)
    (success : observe root fact = some next) : lookup next key = lookup root key := by
  unfold observe at success
  split at success
  · cases success
    simp [lookup, lookupEntries, different]
  · split at success
    · cases success
      rfl
    · contradiction

abbrev Concrete := CellKey → InitialValue

/-- Only observed cells constrain the initial world; missing is unconstrained. -/
def Consistent (root : InitialRoot snapshot) (concrete : Concrete) : Prop :=
  ∀ key value, lookup root key = some value → concrete key = value

theorem observe_preserves_known (root next : InitialRoot snapshot)
    (fact : Observation snapshot) (key : CellKey) (value : InitialValue)
    (known : lookup root key = some value) (success : observe root fact = some next) :
    lookup next key = some value := by
  by_cases equal : key = fact.key
  · subst key
    simp only [observe, known] at success
    split at success
    · cases success
      exact known
    · contradiction
  · rw [observe_other root next fact key equal success]
    exact known

theorem observe_consistent (root next : InitialRoot snapshot) (fact : Observation snapshot)
    (concrete : Concrete) (base : Consistent root concrete)
    (observation : concrete fact.key = fact.value) (success : observe root fact = some next) :
    Consistent next concrete := by
  intro key value known
  by_cases equal : key = fact.key
  · subst key
    have installed := observe_installs root next fact success
    rw [installed] at known
    cases known
    exact observation
  · rw [observe_other root next fact key equal success] at known
    exact base key value known

/-- Successful insertion narrows the initial models and keeps all prior facts. -/
theorem observe_refines (root next : InitialRoot snapshot) (fact : Observation snapshot)
    (concrete : Concrete) (success : observe root fact = some next)
    (h : Consistent next concrete) :
    Consistent root concrete ∧ concrete fact.key = fact.value := by
  constructor
  · intro key value known
    exact h key value (observe_preserves_known root next fact key value known success)
  · exact h fact.key fact.value (observe_installs root next fact success)

structure Incoming where
  snapshot : SnapshotId
  key : CellKey
  value : InitialValue

/-- The dynamic adapter boundary checks snapshot identity before typed insertion. -/
def injectChecked (root : InitialRoot snapshot) (incoming : Incoming) :
    Option (InitialRoot snapshot) :=
  if incoming.snapshot = snapshot then observe root ⟨incoming.key, incoming.value⟩ else none

theorem wrongSnapshot_refused (root : InitialRoot snapshot) (incoming : Incoming)
    (different : incoming.snapshot ≠ snapshot) : injectChecked root incoming = none := by
  simp [injectChecked, different]

inductive Overlay (snapshot : SnapshotId) where
  | initial (root : InitialRoot snapshot)
  | write (previous : Overlay snapshot) (key : CellKey) (value : Nat)
  deriving DecidableEq

def read : Overlay snapshot → CellKey → Option InitialValue
  | .initial root, key => lookup root key
  | .write previous key value, query =>
    if query = key then some (.word value) else read previous query

/-- Observation tags remain metadata; an explicitly absent account reads zero. -/
def InitialValue.sloadWord : InitialValue → Nat
  | .nonexistent => 0
  | .word value => value

def readWord (overlay : Overlay snapshot) (key : CellKey) : Option Nat :=
  (read overlay key).map InitialValue.sloadWord

theorem nonexistent_sload_adapter : InitialValue.sloadWord .nonexistent = 0 := rfl

theorem unobserved_sload_remains_unknown (overlay : Overlay snapshot) (key : CellKey)
    (unknown : read overlay key = none) : readWord overlay key = none := by
  simp [readWord, unknown]

theorem known_zero_and_nonexistent_both_read_zero :
    InitialValue.sloadWord (.word 0) = InitialValue.sloadWord .nonexistent := rfl

/-- Slot-only observations assert a read value, not the presence tag. -/
def satisfiesObservation : Option InitialValue → InitialValue → Prop
  | none, _ => True
  | some .nonexistent, concrete => concrete = .nonexistent
  | some (.word value), concrete => concrete.sloadWord = value

theorem absent_compatible_with_zero_and_nonexistent :
    satisfiesObservation (some (.word 0)) .nonexistent ∧
      satisfiesObservation (some .nonexistent) .nonexistent := ⟨rfl, rfl⟩

theorem slot_zero_does_not_prove_presence :
    satisfiesObservation (some (.word 0)) .nonexistent ∧
      InitialValue.nonexistent ≠ .word 0 := by
  exact ⟨rfl, by decide⟩

theorem read_after_write (previous : Overlay snapshot) (key : CellKey) (value : Nat) :
    read (.write previous key value) key = some (.word value) := by
  simp [read]

theorem read_nonalias (previous : Overlay snapshot) (key query : CellKey) (value : Nat)
    (different : query ≠ key) : read (.write previous key value) query = read previous query := by
  simp [read, different]

theorem owner_separation (previous : Overlay snapshot) (a b : Owner) (slot value : Nat)
    (different : a ≠ b) :
    read (.write previous ⟨a, slot⟩ value) ⟨b, slot⟩ = read previous ⟨b, slot⟩ := by
  apply read_nonalias
  intro equal
  have owners := congrArg CellKey.owner equal
  exact different owners.symm

theorem last_write_wins (previous : Overlay snapshot) (key : CellKey) (a b : Nat) :
    read (.write (.write previous key a) key b) key = some (.word b) := by
  exact read_after_write _ _ _

/-- Produces a new root interpretation; existing overlays are immutable values. -/
def inject : Overlay snapshot → Observation snapshot → Option (Overlay snapshot)
  | .initial root, fact => (observe root fact).map Overlay.initial
  | .write previous key value, fact =>
    (inject previous fact).map (fun next => .write next key value)

theorem injection_preserves_write (previous next : Overlay snapshot)
    (fact : Observation snapshot) (key : CellKey) (value : Nat)
    (success : inject (.write previous key value) fact = some next) :
    read next key = some (.word value) := by
  simp only [inject] at success
  cases h : inject previous fact with
  | none => simp [h] at success
  | some base =>
    simp [h] at success
    subst next
    exact read_after_write _ _ _

def hasWrite : Overlay snapshot → CellKey → Bool
  | .initial _, _ => false
  | .write previous key _, query => query == key || hasWrite previous query

/-- Any historical write, at any depth, takes precedence over new initial facts. -/
theorem injection_preserves_written_read (previous next : Overlay snapshot)
    (fact : Observation snapshot) (query : CellKey)
    (written : hasWrite previous query = true) (success : inject previous fact = some next) :
    read next query = read previous query := by
  induction previous generalizing next with
  | initial root => simp [hasWrite] at written
  | write previous key value ih =>
    simp only [inject] at success
    cases h : inject previous fact with
    | none => simp [h] at success
    | some base =>
      simp [h] at success
      subst next
      by_cases equal : query = key
      · simp [read, equal]
      · have prior : hasWrite previous query = true := by
          simpa [hasWrite, equal] using written
        simpa [read, equal] using ih base prior h

def initialRoot : Overlay snapshot → InitialRoot snapshot
  | .initial root => root
  | .write previous _ _ => initialRoot previous

/-- Interpret missing observations in an arbitrary concrete initial world. -/
def execute : Overlay snapshot → Concrete → Concrete
  | .initial _, initial => initial
  | .write previous key value, initial =>
    fun query => if query = key then .word value else execute previous initial query

def Gamma (overlay : Overlay snapshot) (initial current : Concrete) : Prop :=
  Consistent (initialRoot overlay) initial ∧ current = execute overlay initial

theorem injection_same_execution (previous next : Overlay snapshot)
    (fact : Observation snapshot) (initial : Concrete)
    (success : inject previous fact = some next) : execute next initial = execute previous initial := by
  induction previous generalizing next with
  | initial root =>
    simp only [inject] at success
    cases h : observe root fact with
    | none => simp [h] at success
    | some base =>
      simp [h] at success
      subst next
      rfl
  | write previous key value ih =>
    simp only [inject] at success
    cases h : inject previous fact with
    | none => simp [h] at success
    | some base =>
      simp [h] at success
      subst next
      simp only [execute, ih base h]

theorem injection_refines_initial (previous next : Overlay snapshot)
    (fact : Observation snapshot) (initial : Concrete)
    (success : inject previous fact = some next) :
    Consistent (initialRoot next) initial ↔
      Consistent (initialRoot previous) initial ∧ initial fact.key = fact.value := by
  induction previous generalizing next with
  | initial root =>
    simp only [inject] at success
    cases h : observe root fact with
    | none => simp [h] at success
    | some base =>
      simp [h] at success
      subst next
      constructor
      · exact observe_refines root base fact initial h
      · intro consistent
        exact observe_consistent root base fact initial consistent.1 consistent.2 h
  | write previous key value ih =>
    simp only [inject] at success
    cases h : inject previous fact with
    | none => simp [h] at success
    | some base =>
      simp [h] at success
      subst next
      exact ih base h

/-- The exact before/after concrete-world intersection; truth is never inferred. -/
theorem injection_models_intersection (previous next : Overlay snapshot)
    (fact : Observation snapshot) (initial current : Concrete)
    (success : inject previous fact = some next) :
    Gamma next initial current ↔ Gamma previous initial current ∧ initial fact.key = fact.value := by
  simp only [Gamma, injection_refines_initial previous next fact initial success,
    injection_same_execution previous next fact initial success]
  exact and_assoc.trans (and_congr_right (fun _ => and_comm)) |>.trans and_assoc.symm

theorem injection_models_subset (previous next : Overlay snapshot)
    (fact : Observation snapshot) (initial current : Concrete)
    (success : inject previous fact = some next) (h : Gamma next initial current) :
    Gamma previous initial current :=
  ((injection_models_intersection previous next fact initial current success).mp h).1

theorem injection_true_world_preserved (previous next : Overlay snapshot)
    (fact : Observation snapshot) (initial current : Concrete)
    (truth : initial fact.key = fact.value) (success : inject previous fact = some next)
    (h : Gamma previous initial current) : Gamma next initial current :=
  (injection_models_intersection previous next fact initial current success).mpr ⟨h, truth⟩

structure ReadSource (snapshot : SnapshotId) where
  version : Nat
  overlay : Overlay snapshot
  key : CellKey

/-- A read captures the write history at its version, not a mutable current store. -/
def ReadSource.denote (source : ReadSource snapshot) (initial : Concrete) : InitialValue :=
  execute source.overlay initial source.key

def ReadSource.refine (source : ReadSource snapshot) (fact : Observation snapshot) :
    Option (ReadSource snapshot) :=
  (inject source.overlay fact).map (fun overlay => { source with overlay := overlay })

def ReadSource.Models (source : ReadSource snapshot) (initial : Concrete) (value : InitialValue) : Prop :=
  Consistent (initialRoot source.overlay) initial ∧ source.denote initial = value

theorem historical_source_survives_later_write (source : ReadSource snapshot)
    (initial : Concrete) (key : CellKey) (value : Nat) :
    source.denote initial = execute source.overlay initial source.key ∧
      execute (.write source.overlay key value) initial key = .word value := by
  exact ⟨rfl, by simp [execute]⟩

/-- Same-snapshot observations may narrow a historical read without changing its version. -/
theorem historical_refinement_intersection (source next : ReadSource snapshot)
    (fact : Observation snapshot) (initial : Concrete) (value : InitialValue)
    (success : source.refine fact = some next) :
    next.version = source.version ∧ next.key = source.key ∧
      (next.Models initial value ↔ source.Models initial value ∧ initial fact.key = fact.value) := by
  unfold ReadSource.refine at success
  cases h : inject source.overlay fact with
  | none => simp [h] at success
  | some overlay =>
    simp [h] at success
    subst next
    refine ⟨rfl, rfl, ?_⟩
    simp only [ReadSource.Models, ReadSource.denote,
      injection_refines_initial source.overlay overlay fact initial h,
      injection_same_execution source.overlay overlay fact initial h]
    exact and_assoc.trans (and_congr_right (fun _ => and_comm)) |>.trans and_assoc.symm

structure Frame (snapshot : SnapshotId) where
  checkpoint : Overlay snapshot
  current : Overlay snapshot
  reads : List (CellKey × Option InitialValue)
  deriving DecidableEq

def recordRead (frame : Frame snapshot) (key : CellKey) : Frame snapshot :=
  { frame with reads := (key, read frame.current key) :: frame.reads }

def refine (frame : Frame snapshot) (fact : Observation snapshot) : Option (Frame snapshot) :=
  (inject frame.current fact).map (fun current => { frame with current := current })

def rollback (frame : Frame snapshot) : Frame snapshot :=
  { frame with current := frame.checkpoint }

theorem currentOnly_refinement_keeps_history (frame next : Frame snapshot) (fact : Observation snapshot)
    (success : refine frame fact = some next) :
    next.checkpoint = frame.checkpoint ∧ next.reads = frame.reads := by
  unfold refine at success
  cases h : inject frame.current fact with
  | none => simp [h] at success
  | some current =>
    simp [h] at success
    subst next
    exact ⟨rfl, rfl⟩

theorem currentOnly_refinement_rollback (frame next : Frame snapshot) (fact : Observation snapshot)
    (success : refine frame fact = some next) :
    (rollback next).current = frame.checkpoint := by
  exact (currentOnly_refinement_keeps_history frame next fact success).1

theorem currentOnly_recorded_read_is_historical (frame next : Frame snapshot) (fact : Observation snapshot)
    (key : CellKey) (success : refine (recordRead frame key) fact = some next) :
    next.reads = (key, read frame.current key) :: frame.reads := by
  exact (currentOnly_refinement_keeps_history _ _ _ success).2

theorem observations_distinct :
    (none : Option InitialValue) ≠ some (.word 0) ∧
    (none : Option InitialValue) ≠ some .nonexistent ∧
    (some (.word 0) : Option InitialValue) ≠ some .nonexistent := by
  decide

def fixtureSnapshot : SnapshotId := ⟨1, 100⟩
def fixtureKey : CellKey := ⟨⟨7⟩, 3⟩
def emptyRoot : InitialRoot fixtureSnapshot := ⟨[]⟩

theorem currentOnly_concrete_inject_then_rollback :
    let base := Overlay.initial emptyRoot
    let frame : Frame fixtureSnapshot := ⟨base, .write base fixtureKey 19, []⟩
    let fact : Observation fixtureSnapshot := ⟨fixtureKey, .word 5⟩
    let updated : Frame fixtureSnapshot :=
      ⟨base, .write (.initial ⟨[(fixtureKey, .word 5)]⟩) fixtureKey 19, []⟩
    refine frame fact = some updated ∧
    read updated.current fixtureKey = some (.word 19) ∧
    read (rollback updated).current fixtureKey = none := by
  decide

#print axioms observe_conflict
#print axioms observe_idempotent
#print axioms observe_installs
#print axioms observe_other
#print axioms observe_preserves_known
#print axioms observe_consistent
#print axioms observe_refines
#print axioms wrongSnapshot_refused
#print axioms nonexistent_sload_adapter
#print axioms unobserved_sload_remains_unknown
#print axioms known_zero_and_nonexistent_both_read_zero
#print axioms absent_compatible_with_zero_and_nonexistent
#print axioms slot_zero_does_not_prove_presence
#print axioms read_after_write
#print axioms read_nonalias
#print axioms owner_separation
#print axioms last_write_wins
#print axioms injection_preserves_write
#print axioms injection_preserves_written_read
#print axioms injection_same_execution
#print axioms injection_refines_initial
#print axioms injection_models_intersection
#print axioms injection_models_subset
#print axioms injection_true_world_preserved
#print axioms historical_source_survives_later_write
#print axioms historical_refinement_intersection
#print axioms currentOnly_refinement_keeps_history
#print axioms currentOnly_refinement_rollback
#print axioms currentOnly_recorded_read_is_historical
#print axioms observations_distinct
#print axioms currentOnly_concrete_inject_then_rollback

end NoSmt.Snapshots
