# Kernel-checked local state obligations

These Std-only Lean 4.30.0 models prove concrete state operations and include
executable counterexamples. The proof-to-production relation is a manually
reviewed contract in [correspondence.md](correspondence.md). The Rust executor,
its worklist, and the richer numeric domain have not been translated into Lean.

## Checker and evidence

The checker is pinned to `leanprover/lean4:v4.30.0` in
[lean-toolchain](../../proofs/lean-toolchain), checked by
[check.sh](../../proofs/check.sh), and supplied by the locked `pkgs.lean4` in
[proofs.nix](../../nix/checks/proofs.nix). The leaf exposes `lean-proofs`; its source
boundary contains only `proofs/**`. The checks aggregate and affected-check
selector are owned by the integration work, outside this leaf.

Run an installed 4.30.0 checker without Lake, mathlib or network resolution:

```sh
LEAN_BIN=/absolute/path/to/lean-4.30.0/bin/lean \
  bash proofs/check.sh /absolute/path/to/proof-evidence
```

The script compiles project modules into the evidence directory, checks 260
public theorem and regression targets with `#print axioms`, and writes `audit.json`
with source hashes, theorem dependencies and the actual expected-rejection exit.
Warnings are errors.
The installed pinned checker passed all 260 targets (248 existing plus 12
ByteProduct) at proof source `f15bbbfc16b8df2435d533d0132b3ca3cb7aea1d`, with
old-join rejection exit 1.
The deliberately false [OldJoinRejected.lean](../../proofs/counterexamples/OldJoinRejected.lean)
must fail because `decide` establishes that the old join excludes zero, after its
Facts import was successfully compiled. An import failure is not accepted.

[audit.py](../../proofs/audit.py) rejects source `sorry`, `admit`, axiomatic target
declarations, `native_decide`, compiler-trust shortcuts, missing theorem prints,
`sorryAx`, and dependencies outside the documented Lean core primitives. The
models disable implicit parameter invention. Current proofs use `propext`
(propositional extensionality) and `Quot.sound` (quotient soundness, including
function extensionality derived through quotients). The audit also permits
Lean's foundational `Classical.choice`, used by ByteProduct's arithmetic-reference,
extraction and range targets.
There is no assumed target-soundness axiom. Lean's kernel, the installed Std
library/checker, and the dependency-report audit remain trusted infrastructure.

## Model scope

| Model | Operation and concrete instance |
| --- | --- |
| [Facts](../../proofs/NoSmt/Facts.lean) | Finite partial must-fact map, absent = Top; support intersection with finite-set union; concretization includes both input paths; old left-biased join excludes the permitted right-path `0`, corrected join accepts it; equal roots can have different facts. |
| [PartialFacts](../../proofs/NoSmt/PartialFacts.lean) | The same support-intersection algorithm for an arbitrary component domain, with explicit left/right component-join obligations; the finite-set instance discharges both. Production `NumericValue` inclusion is a separate obligation. |
| [ConditionalFacts](../../proofs/NoSmt/ConditionalFacts.lean) | Common-key Gamma conjoined with guarded residual and previous scopes; whole-value residual filtering under unique keys, including a duplicate-shadow counterexample; left/right inclusion with finite Nat-set and BitVec256-set instances; unknown-guard and inactive-EMPTY witnesses; exact recursive pruning that retains active numeric guarantees and all conjunctions; support-checked fresh injection with numeric-derived guard truths. The pruning proof corresponds to the implemented constant-guard normalization; the source correspondence and twelve-level Rust case-preparation regression are listed separately in the contract. |
| [Snapshots](../../proofs/NoSmt/Snapshots.lean) | Typed snapshot and owner keys; consistent insertion, idempotence, conflict and wrong-snapshot refusal; concretization after injection is the old set intersected with the supplied initial fact; ordered write precedence, nonalias reads, history versions and rollback. |
| [ReturnLengths](../../proofs/NoSmt/ReturnLengths.lean) | Transparent BitVec256 ITE evaluation and explicit expression/source coherence; same-path whole-length bounds imply guarded arm facts through the existing map/scoped Gamma. Imports retain the whole fact and all ancestor guards, preserve old Gamma, and are neutral under the proved path bound; repeated stamping is semantically idempotent. Checked witnesses cover unknown guards, inactive empty/contradictory arms, a whole-only cache missing leaf implications, fresh old-length-1/new-length-2 sources, and a shared-initial saved/current product admitting `(0,1)` through conservative forgetting and distinct fresh keys, with rollback selecting saved. |
| [ExactIte](../../proofs/NoSmt/ExactIte.lean) | Five polymorphic pointwise ITE factoring equalities with independent Word256 guards, where nonzero is true. Complete immutable A/B state identity and the full Rust-to-model binding remain manual obligations; resource, depth or node monotonicity and whole-Rust correctness are not proved. |
| [NumericCarrier](../../proofs/NoSmt/NumericCarrier.lean) | One pointwise equivalence `(x ∈ F ∧ R(c,x)) ↔ R(c,x)`, assuming complete membership `x ∈ F ↔ I(x)` in a nonwrapping unsigned Word256 interval `I` and containment `R(c,x) → I(x)` of the current remaining constraints. It covers finite-only compaction and redundant restoration; density/cardinality detection and the U256-to-model binding remain manual obligations. |
| [ByteProduct](../../proofs/NoSmt/ByteProduct.lean) | Gamma inclusion for the direct intersection of component postimages, with an explicit sound-transfer premise for every component on the same concrete input; forgetting constraints to Top preserves inclusion. Separately, exact BitVec256 BYTE shift/mask semantics equal the natural division/modulo reference and zero-extended eight-bit extraction: index 0 is the top byte, 31 the low byte, and ≥32 zero. AND255 is the low byte; both results lie in `0..255`. Component algorithms, Rust U256/source binding and unreduced-product consumers remain manual obligations; no precision equality or cost result is claimed. |
| [Effects](../../proofs/NoSmt/Effects.lean) | The protected-owner field envelope; local havoc inclusion, idempotence and closure for any finite allowed local-write trace; proved executable callback store/add/sequence/indirect-call rules and every finite allowed mixed trace, including a repeated indirect-call instance. |
| [Engine](../../proofs/NoSmt/Engine.lean) | Generic forward simulation and postfixed finite-prefix coverage; proved local Word256 register, load/store, zero-branch, unknown-envelope, checkpoint, touch and rollback transitions; a checked finite certificate for a looping program containing unknown writes and repeated indirect callbacks, with a protected-code invariant and an empty-queue counterexample. |
| [Lifecycle](../../proofs/NoSmt/Lifecycle.lean) | Three-valued AND/OR, nonce/balance/code emptiness, pending-destruction OR touched-empty decision, definite and optional touch, flag joins, EIP-161 existence projection, static optional-touch completion, ordinary rollback and owner-3 sticky-touch rollback. |
| [LateFailure](../../proofs/NoSmt/LateFailure.lean) | Exact entry-only Failure counterexample after an owner-3 touch; saved/current rollback plus account cleanup; may-touch and ordinary-owner controls; Effects word-write restoration before cleanup; finite register ADD/jump suffix identity with normal and failure alternatives. |
| [Logs](../../proofs/NoSmt/Logs.lean) | Address/code-address/captured-code-hash/mode/PC source identity; old init/runtime collision; separate-source differing arities accepted, equal-source arity mismatch refused, repeated-source payloads aggregated. |

Facts and Snapshots use `Nat` as labels for keys and word values: membership and
write identity are proved, not arithmetic. ReturnLengths expression values,
ExactIte guards, NumericCarrier/ByteProduct words, and Effects/Engine/LateFailure
values and deltas use `BitVec 256`; the ReturnLengths saved/current supplement retains
Snapshots' Nat-valued cell projection.
`word_add_modulus` proves ADD is modulo `2^256`;
`maxword_plus_one_is_zero`, `wraparound_callback_storage` and
`loop_add_wraps_at_actual_transition` check the maximum word overflowing to zero.
The bridge to Rust's U256 representation and the unmodeled opcode transfers
remain separate obligations. The trace theorem requires the displayed `Event.Allowed`
policy and callback syntax; it does not prove that a Rust worklist covers all
concrete EVM callbacks. Resource exhaustion and omitted work must still be
reported by the executor.

`ReturnLengths.evaluate` interprets ITE recursively and derives guard truth from
zero/nonzero 256-bit words. `Coherent` binds expression keys and opaque source
inputs to that interpretation; it contains no numeric-bound premise.
`same_path_key_guarded_arms` derives both guarded key memberships from the
whole-key bound under the same predicate F. `same_path_whole_key_import_Gamma_iff`
then connects the import to `PartialFacts.Gamma` and `ConditionalFacts.scopeGamma`.
The true stamp contains both the whole-R fact and the recursive leaf implications.
The whole-only counterexample concerns raw metadata valuations that lack
coherence; `coherent_whole_iff_stamp` proves their equivalence for coherent
concrete meanings. A cache hit on the whole key therefore cannot stand in for
the stored implication tree needed by a leaf lookup. Full stamp equality in
production controls duplicate work; the checked idempotence is semantic.

The production consumer is `analysis::transfer::relations::state::length::refine`:
it takes the active frame's exact `returndata.len()` and, when needed, asks
`RelationState::local_value_budgeted` under the same relations F. Its result is
imported by `ByteArray::import_length_bound` through
`world::bytes::conditional::length::distribute`. The checked operation augments
metadata. NumericValue bound soundness, the query's same-F correspondence,
ExprId/opaque-source interpretation, composite dependency support, immutable
content/source identity and capture-avoiding allocation remain trusted
correspondence obligations. The finite Word256-set instance does not prove the
production numeric domain or byte-window evaluator. The source symbols and
reproducible executable regressions are listed in [correspondence.md](correspondence.md).

The envelope protects the supplied known executable owners. Production must
establish that these are owners with stable nonempty executable code, rather
than every address mentioned in a world. The model forgets all balances and the
delegated owner's storage/nonce/pending flag, including a conservative loss of
the pending flag that production may preserve for a preexisting owner. Touch
flags and logs are separate projections. `static_field_projection_identity`
does not assert full-Store identity: an unknown STATICCALL can still touch a
recipient, and `static_touch_can_change_completion` checks that this can change
empty-account cleanup.

## Snapshot truth, history and rollback

`Consistent` constrains explicitly observed **complete presence-tagged cells**.
Missing facts are unconstrained. `injection_models_intersection` proves the exact refinement
relation. To preserve a particular previously admitted concrete world,
`injection_true_world_preserved` explicitly requires that the observation is
true in that same initial world. The model checks per-cell consistency; account
cross-field validation and observation authenticity remain external obligations.

Unobserved, known zero and explicit nonexistent are different observation tags.
`InitialValue.sloadWord` is the explicit nonexistent-to-zero read adapter;
`readWord` leaves unobserved reads unknown. A nonexistent tag is not an EVM stack
word or a manufactured observation for every unmentioned account.
Tag inequality does not imply incompatible slot-read predicates:
`satisfiesObservation` treats a word fact as a read value without asserting account
presence, and `absent_compatible_with_zero_and_nonexistent` checks both facts at
an absent account. The exact tagged-root insertion model can reject different
complete tags; it must not be mapped directly onto Rust's slot-only observations
to reject a valid combination of zero storage and account absence.

An overlay records ordered runtime writes. Initial injection cannot change any
concrete execution of that write history. A `ReadSource` captures its key, version
and overlay; later runtime writes do not replace that source. New observations
about the same initial snapshot may narrow its permitted values without changing
the source version, as proved by `historical_refinement_intersection`.

The additional `currentOnly_*` frame lemmas model a conservative implementation
that refines only the current root and retains broader checkpoints/read bounds.
They are not a requirement that production preserve every unknown upper bound
exactly. The layers session may create a new immutable fact revision and replay
from the initial root, refining old read/checkpoint bounds consistently. The
models do not verify its memo or dependency-invalidation algorithm.

The ReturnLengths product supplement uses two existing `Snapshots.ReadSource`
components at the same typed snapshot and the same concrete initial world.
Each component keeps its own version and overlay. Conservative forgetting drops
selected read-model constraints while retaining the product: the exact runtime
write witness has saved `0` and current `1`, and remains admitted for every
forgetting mask. Distinct fresh keys can represent that pair; one shared key or
a diagonal value cannot. `rollback_selects_saved_source` restores the saved
overlay and version. This runtime-write example does not inject a new initial
observation. Binding the component product to Rust's full payload, renaming and
saved-store allocation remains an obligation.

Snapshot overlay rollback covers the modeled cell fields. Lifecycle rollback
has its own rule: ordinary owner touches revert, while owner `3` merges saved
and current touchedness. `rollback_inclusion` proves this explicit exception.
It does not prove every protocol touch site or the full account/storage/log
reset in `Store::delete_account`.

## Late failure

`LateFailure.entry_only_counterexample` starts with an exactly present,
untouched, empty account and a late current account whose touch is true. Entry
cleanup admits presence and excludes the concrete absent result after owner-3
rollback and transaction cleanup. `late_failure_repairs_counterexample` applies
the same concrete rollback/cleanup operations to the late state. The general
`rollback_finish_inclusion` composes Lifecycle's rollback and cleanup lemmas
from saved/current Gamma; it assumes no sound failure envelope. May-touch
includes either concrete touch choice, ordinary owners use the saved touch,
non-touch account facts restore, and cleanup resets touchedness.

`restore_inclusion` and the Effects word-write instances concern the restored
field projection **before cleanup**, as required for a relative or child
Failure. `restore_then_cleanup_inclusion` separately relates only the selected
account **after cleanup**. Account deletion can clear persistent storage, code,
balance and other fields; no unchanged Effects field map is related after that
deletion. Transient-state cleanup and logs are outside this minimal projection.

`runPure` is an explicit finite register ADD/jump semantics. Its projection
identity induction supports suffix failure inclusion, while `alternatives`
retains normal execution and adds failure when the selected account's touch is
may/must. Production selects owner `3`. These theorems do not establish actual EVM gas behavior or all paths.

The production binding is `analysis::transfer::late_failure::at_block_entry`,
`transfer::execute_inner`, `calls::{failure,finish}` and `Store::restore`.
[correspondence.md](correspondence.md) lists the exact existing revm and source
unit regressions and commands to reproduce them. CALL/CREATE ending blocks,
actual opcode/touch-site coverage, instruction-local touch/exception ordering
(including SELFDESTRUCT beneficiary touches), frame-relative versus root
continuation, all Store deletion fields, logs/transient behavior, fees, typed
cancellation, frontiers and Rust-to-model simulation remain manual obligations.
The finite suffix theorem does not establish that block-entry alternatives
cover an exception following a touch within the same instruction. The Rust
implementation separately uses `late_failure::after_selfdestruct` to retain a
Failure alternative after a possible beneficiary touch. The
`selfdestruct_dynamic_gas_failure_restores_value_but_keeps_ripemd_touch`
regression contrasts dynamic OutOfGas with sufficient gas and zero-balance
execution across forks and both summary settings. This instruction-local
implementation and concrete regression do not discharge its Lean-to-Rust
opcode, touch-order and gas correspondence obligations.

## Engine certificate scope

`Postfixed` requires a stored summary for every abstract successor of each
retained input summary, together with a sound structural coverage relation.
`postfixed_finite_prefix` also requires initial-input coverage and local transfer
simulation. The concrete instantiation discharges the transfer and coverage
premises for its modeled operations; it does not assume them as axioms.

`loop_postfixed` proves a certificate for labels `0..8`: checkpoint, assign the
maximum word, modular ADD, zero branch, persistent write, arbitrary finite
unknown local writes, nested known callback additions, and rollback along a
back edge. `loop_from_any_admitted_seed` covers every finite execution prefix
from an admitted seed; `loop_preserves_protected_code` checks the protected code
field for the explicit seed. There is no iteration cutoff in these theorems.
`empty_queue_does_not_prove_postfixed` exhibits a missing successor even when
both queue/frontier lists are empty.

This instantiation projects one register, a word-field store, one selected
account's lifecycle facts and a saved checkpoint. It does not encode every EVM
stack operation, dynamic jump, call stack, protocol completion store reset or
Rust scheduler mutation. For the production binding, structural keys must carry
the required owner/code/mode/context identity; each changed input must invalidate
and reprocess its transfer, all successors must propagate, and resource or
unsupported boundaries must remain frontiers. Root must establish that the
actual empty-queue/no-frontier result satisfies `Postfixed`, rather than infer
that theorem from the result's status label alone.
