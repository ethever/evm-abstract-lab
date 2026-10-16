import Std

set_option autoImplicit false

/-!
Captured LOG-source identity and aggregation by that identity. Hashes and address
identifiers are opaque Nat labels here; no hashing or Word256 arithmetic claim.
The producer must obtain identity from the executing frame, not mutable code.
-/
namespace NoSmt.Logs

inductive CodeMode where
  | init
  | runtime
  deriving DecidableEq, Repr

structure LogSource where
  address : Nat
  codeAddress : Nat
  codeHash : Nat
  mode : CodeMode
  pc : Nat
  deriving DecidableEq, Repr

theorem mode_separation (address codeAddress hash pc : Nat) :
    LogSource.mk address codeAddress hash .init pc ≠
      LogSource.mk address codeAddress hash .runtime pc := by
  intro equal
  have modes := congrArg LogSource.mode equal
  cases modes

theorem code_separation (address codeAddress left right pc : Nat) (different : left ≠ right) :
    LogSource.mk address codeAddress left .runtime pc ≠
      LogSource.mk address codeAddress right .runtime pc := by
  intro equal
  exact different (congrArg LogSource.codeHash equal)

theorem source_identity (a b : LogSource) (equal : a = b) :
    a.address = b.address ∧ a.codeAddress = b.codeAddress ∧
      a.codeHash = b.codeHash ∧ a.mode = b.mode ∧ a.pc = b.pc := by
  cases equal
  exact ⟨rfl, rfl, rfl, rfl, rfl⟩

def oldLogKey (source : LogSource) : Nat × Nat × Nat :=
  (source.address, source.codeAddress, source.pc)

theorem old_log_collision (address codeAddress hash pc : Nat) :
    oldLogKey ⟨address, codeAddress, hash, .init, pc⟩ =
      oldLogKey ⟨address, codeAddress, hash, .runtime, pc⟩ ∧
    LogSource.mk address codeAddress hash .init pc ≠
      LogSource.mk address codeAddress hash .runtime pc :=
  ⟨rfl, mode_separation address codeAddress hash pc⟩

structure LogRecord where
  source : LogSource
  topicCount : Nat
  payloads : List Nat
  deriving DecidableEq, Repr

/-- Models only source grouping, topic arity checks and possible payload union. -/
def emit : List LogRecord → LogRecord → Option (List LogRecord)
  | [], event => some [event]
  | first :: rest, event =>
    if first.source = event.source then
      if first.topicCount = event.topicCount then
        some ({ first with payloads := first.payloads ++ event.payloads } :: rest)
      else none
    else (emit rest event).map (first :: ·)

theorem distinct_sources_do_not_collide (first event : LogRecord)
    (different : first.source ≠ event.source) : emit [first] event = some [first, event] := by
  simp [emit, different]

theorem same_source_arity_refused (source : LogSource) (a b : Nat) (left right : List Nat)
    (different : a ≠ b) : emit [⟨source, a, left⟩] ⟨source, b, right⟩ = none := by
  simp [emit, different]

theorem repeated_same_source_aggregates (source : LogSource) (arity : Nat) (left right : List Nat) :
    emit [⟨source, arity, left⟩] ⟨source, arity, right⟩ =
      some [⟨source, arity, left ++ right⟩] := by
  simp [emit]

/-- An actual differing-arity init/runtime pair is accepted as two source sites. -/
theorem init_runtime_different_arity (address codeAddress hash pc : Nat) :
    emit [⟨⟨address, codeAddress, hash, .init, pc⟩, 1, [11]⟩]
      ⟨⟨address, codeAddress, hash, .runtime, pc⟩, 2, [22]⟩ =
    some [⟨⟨address, codeAddress, hash, .init, pc⟩, 1, [11]⟩,
      ⟨⟨address, codeAddress, hash, .runtime, pc⟩, 2, [22]⟩] :=
  distinct_sources_do_not_collide _ _ (mode_separation _ _ _ _)

#print axioms mode_separation
#print axioms code_separation
#print axioms source_identity
#print axioms old_log_collision
#print axioms distinct_sources_do_not_collide
#print axioms same_source_arity_refused
#print axioms repeated_same_source_aggregates
#print axioms init_runtime_different_arity

end NoSmt.Logs
