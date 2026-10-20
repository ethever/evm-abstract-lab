# Trait cooperation migration acceptance

This document tracks validation of the migration from the concrete EVM interpreter to a reusable semantic kernel and configurable production EVM scalar carriers. It describes acceptance obligations; the existence of a row does not mean that its implementation or test has passed.

The production scope includes ordinary CFG propagation, known recursion, unknown interactions, and full scalar-carrier customization through machine, byte, Store, summary, rollback and report boundaries. An independent small language is additional evidence that the kernel is reusable.

## Evidence boundaries

Keep these observations separate in every result:

- Native process exit, external guard termination, elapsed time and peak RSS.
- Detailed Converged or Incomplete status, all frontiers and retained pending work.
- Procedure summary input/output changes, subscriber delivery and closed flags.
- Optional AllInputStates root-terminal coverage and its original-input binding.
- Report syntax/identity validation, consumer delivery guards and semantic tests.

A root coverage attachment does not complete the detailed CFG or SSA. Native exit 2 accompanies an Incomplete report. A whole-report parsing guard may reject a complete JSON output; a separate streaming supplement records its own result without changing the original rejection.

The [fixed offline corpus](../examples/no-smt-scale/README.md) preserves unknown caller, call value, calldata content and length, as well as unobserved accounts and storage. The [captured USDC witness](../examples/known-recursion/README.md) is a specific input used for call/delegatecall and rollback checks. It does not replace the unknown-input workloads.

## Requirement mapping

| Obligation | Existing regression foundation | Additional migration evidence |
| --- | --- | --- |
| Language-independent kernel | Current propagation and procedure worklists | External consumers define their own instruction language, state/domain and partition through public traits; the kernel does not import EVM machine types. |
| Independent cooperation cycle | [Product-domain tests](../crates/evm-abstract/tests/product_domains.rs) | Two independent analyzers use declared typed capabilities, explicit compatible routes and registered sound reductions. Both directions change useful results; dependency reads and change rescheduling are managed by the framework. |
| Multiple providers | No general provider registry before the migration | Explicit selection or combination; conjunctive guarantees and alternative behaviors use their declared semantics. Reverse provider order and late publication; no first-provider fallback. |
| Pending, unknown and bottom | [Recursive resources](../crates/evm-abstract/tests/known_recursion/resources.rs), [unknown reporting](../crates/evm-abstract/tests/unknown_world/reporting.rs) | A pending or partially empty result cannot justify unreachability. Unknown is a semantic upper bound; Bottom is an empty set. Cancellation, errors, unsupported queries and budgets remain distinct. |
| Scope and version identity | Initial-fact session tests and [recursive contexts](../crates/evm-abstract/tests/known_recursion/contexts.rs) | Isolate equal labels/PCs across sessions, snapshots, revisions, owners, call modes, partitions and input versions. A weakened incoming state invalidates facts tied to the old local input. |
| Dynamic CFG and feedback | [Context depth](../crates/evm-abstract/tests/context_depth.rs), procedure cycle tests | A later query answer discovers a jump target or backedge after a consumer ran. New edges remain represented even when the target payload is unchanged. |
| Join, reduction and widening | Product-domain semilattice tests and [world joins](../crates/evm-abstract/tests/world_join_facts.rs) | Global joins cover both path states. Local reduction preserves the scoped product meaning. Widening has a separate coverage and termination contract; bounded reduction does not claim unsupported closure. |
| Production EVM use | [Native entrypoints](../crates/evm-abstract/tests/native_core.rs), known-recursion and unknown-world suites | The actual ordinary and recursive EVM paths use the shared generic protocols and selected cooperating providers. Query counters alone do not establish useful semantic integration. |
| Recursive dependency updates | [Recursive admission](../crates/evm-abstract/tests/known_recursion/admission.rs), procedure worklist tests | Growing procedure inputs rerun bodies; new output generations revisit every affected caller. Equal output publications avoid redundant delivery; changed caller input still schedules the required continuation. |
| Continuations and rollback | [Call semantics](../crates/evm-abstract/tests/calls.rs), [Store checkpoints](../crates/evm-abstract/tests/store_checkpoints.rs), recursive/unknown rollback suites | Two callers of one summary retain distinct output ranges and saved checkpoints. Child success, revert, exception and outer rollback cannot cross-contaminate caller state or facts. |
| Unknown interactions | [Unknown interaction tests](../crates/evm-abstract/tests/unknown_world/interactions.rs) | Repeat, nest and indirectly route callbacks while analysis facts change. Preserve known/unknown target remainder coverage and static/owner restrictions. |
| Full scalar-carrier customization | Existing concrete product/constants policies | A second externally supplied carrier retains its own facts through arithmetic, byte packing, storage defaults, calls, recursion, checkpoints and reports. Converting every operation back to the default carrier does not demonstrate this requirement. |
| Resource closure | [Summary import](../crates/evm-abstract/tests/summaries.rs), [cancellation](../crates/evm-abstract/tests/control.rs), [root coverage](../crates/evm-abstract/tests/root_terminal_coverage.rs) | Interrupt query registration, reduction, publication and a later caller delivery. All unfinished obligations remain visible; one shared ledger covers the production paths. |
| Public compatibility | CLI/protocol/server tests, state-backend parity and real browser checks | Preserve report/exit conventions, identity relationships, effects, raw frontiers, valid SSA and fresh matching Web assets. Full feature and no-dynamic-dispatch checks remain required. |

## Prepared semantic cases

Concrete kernel tests will use the published API. The following cases fix the semantic questions without defining a competing interface.

| Case | Input and event sequence | Required observation |
| --- | --- | --- |
| Bidirectional bounded cycle | Non-wrapping values 0..10, x+y=10, y in 4..6 and x odd. Bounds and residue-candidate analyzers exchange facts through both registered reductions. | Both reach x=y=5. Consumers may run before producers. Removing either reduction leaves a wider component result. Concrete enumeration validates the joint satisfying set. |
| Scope twins | Reuse one query key/PC and display label in independent sessions and in different owner/partition/input versions. | No cross-context facts. Cloning the same intended immutable identity remains supported. |
| Late provider | A consumer reads before a producer or before a second compatible provider publishes. | The recorded dependency schedules another run; absence does not become an infeasibility proof. |
| Wider entry after reduction | First reduce a local input to a singleton, then join a new incoming alternative into the global state. | Previous version-local facts cannot incorrectly prune the new alternative. |
| Late dynamic jump | A provider update exposes another valid target, which forms a previously undiscovered cycle. | The target and edge are discovered and scheduled; closure includes their obligations. |
| Two caller checkpoints | Two call sites subscribe to a shared recursive summary with distinct pre-call stores and output destinations. Later publish another callee output generation. | Both continuations receive the change with their own checkpoints; revert and failure restore the proper subtree. |
| Budget during publication | Exhaust the shared budget while recording a reduction, publishing a fact or resuming the second subscriber. | No partially installed success or falsely closed summary; typed unfinished work remains. |
| Alternate carrier round trip | Supply a non-default scalar carrier with distinguishing facts; move its values through memory, persistent/transient storage, calls and recursion; revert one subtree. | Facts survive every supported representation boundary, or explicit sound coarsening is recorded. The production path does not silently replace the carrier with NumericValue. |

The new production branch/jump case must exercise the chosen real EVM providers on ordinary and recursive paths. A finite concrete witness must remain covered; an impossible target may be removed only by a justified reduction.

Existing concrete helpers often check each field against one abstract outcome. For the new correlation-sensitive cases, use one shared assignment for return data, persistent/transient values, balances, lifecycle fields and rollback observations. Bounded evaluators must reject unsupported expression fragments rather than silently approximate their validation. Keep the existing Store log interpretation: possible source sites and payloads do not encode exact event ordering or multiplicity.

## Baseline and candidate replay

Bind each executable to its complete source SHA/tree, clean state, actual Nix derivation/output and ELF hash. An already-built output may be reused when pinned evaluation of the selected commit resolves to that same derivation; record its producer receipt and do not claim a new build.

Retain immutable input bytes, code identities, snapshot/environment/fact boundaries, complete explicit policy, argv, process handles, resource samples, native outputs and harness hashes. Historical archives stay unchanged.

Run two separately named policies:

1. Current CLI policy, explicitly max_work=2000000000 and root_terminal_coverage=true, with all remaining current defaults recorded.
2. Fixed high-budget policy, explicitly max_work=1000000000000 and root_terminal_coverage=false, with the original corpus guards and remaining limits.

The [command generator](../scripts/generate-no-smt-scale-commands.py) produces the fixed policy; deriving the default policy changes only those two reviewed fields. The [guarded runner](../scripts/benchmark-no-smt-scale.py) preserves normal exits, external stops and whole-report guard failures. WETH/USDC use the original fixed observed worlds without live RPC hydration.

Single baseline screens establish reproducible observations, not a speedup. Performance comparisons require interleaved paired runs of frozen baseline/candidate ELFs with the same inputs, policy, resource controls and delivery boundary, without concurrent task builds. Compare elapsed time and RSS alongside semantic precision, closure and output size. Work accounting must carry its version; unlike versions are not interchangeable units.

Strict comparison retains all semantic fields. The [identity normalizer](../scripts/normalize_identity_scopes.py) permits only consistent bijective renaming of supported identity_scope namespaces. Changed aliases, source relationships, counters, diagnostics or ordering require visible review rather than field deletion.

## Check integration and handoff

The downstream [kernel cooperation target](../crates/analysis-kernel/tests/cooperation.rs) exercises independent bound/candidate rules, finite joint concretization and both removal controls, explicit providers, missing capabilities, in-flight updates, generations, registered seed authority, prepared publication, replay staging and budget closure. Its source-bound integrated replay passes 21 tests, including authoritative selector ownership/clone replacement and paid zero-step closure reporting after the kernel fixes. Focused Clippy for this target passes with warnings denied. These checks do not replace the final complete gate.

The [production cooperation target](../crates/evm-abstract/tests/cooperation.rs) contains an independent interval/excluded-word carrier, borrowed non-Copy policy, source/default wrappers, CALL/delegate contexts, rollback, saved/current state, memory/Store summaries, recursive output, native report and quotation controls. It exercises the real ProductAlgebra assembly with bit/range removal cases. Against committed integration head `cd696a9` plus a frozen engine overlay, the original 14-test replay passed 9 tests and failed 5. After the evidence-based test refinements and two extensibility cases, 14 of 16 pass; two carrier regressions remain failing. This is a focused integration result, not a full gate.

The public [builtin-port feedback cases](../crates/evm-abstract/tests/cooperation/bridge.rs) pass ordinary and recursive production runs with actual CFG removal controls, Body feedback, growing summary outputs, fresh input generations and equal-publication quiescence. The [client-owned capability cases](../crates/evm-abstract/tests/cooperation/bridge/extra.rs) add independently authored proof provider and consumer modules. Their `ZeroGuards` capability exists only in the client, uses typed extra stores on the same production kernel, and connects to interpreter-consumed StackFacts. The consumer starts before the provider; an automatic Pending-read dependency wakes it. Removing either component loses real CFG precision. One concrete revm execution remains covered by a joint data/storage/balance/log outcome. Recursive summary growth recreates these custom channels; repeated equal publications terminate. Both cases pass without private engine access or manual scheduling.

The native serialization control now passes with the actual complete excluded-word numeric object, after the domain serialization fix. A disabled-EQ control alone became insufficient once generic relational reasoning retained the predicate. The replacement control explicitly coarsens the native scalar during policy projection, keeps the same input assumptions and DomainSpec, and must lose actual CFG precision. Its positive and negative assertions pass.

CALL acceptance preserves the status alongside returned memory. The custom analyzer and default analyzer retain `(status1,data42)` and `(status0,data0)` as distinct alternatives; successful calls must return exact42 and preserve the joint owner Store invariant. Concrete revm runs with forwarded transaction gas and forwarded zero gas witness the two alternatives. An unconditional exact42 assertion after discarding CALL status was incorrect.

The resource case now requires an ordinary shared Work stop after useful feedback to return `Incomplete`, only Work frontiers, an attributable pending continuation, no later detailed Return/Revert, and intact earlier failure rollback. Cancellation independently returns typed `Cancelled` without a report. The current ad-hoc cooperating entry supplies no AnalysisSession snapshot/revision, so root-terminal preparation refuses `MissingIdentity`; the test explicitly checks that coverage is absent even with the option enabled. This does not establish AllInputStates coverage. Existing default-session coverage regressions remain required.

Two genuine scalar-carrier failures remain visible. Whole-word memory and memory-summary roundtrips lose the excluded-word predicate, while direct persistent/transient state, checkpoints and Store-summary roundtrips retain it. In the recursive case, reported summary outputs retain the predicate but one outer resumed root state and its Return lose it. Both tests remain failing and print the boundary values. The source-bound owner note and raw logs are under `/tmp/evm-trait-20261020/validation-carrier-diagnosis.md` and `validation-tests/current/`; this replay predates the later World binding-import followup.

The focused integrated Clippy invocation with `-D clippy::all` currently fails in production `analysis/cooperation/refinement.rs` (large enum) and `analysis/state/projection.rs` (type complexity). These failures are recorded without lint allowances or production edits; no integrated lint pass is claimed.

Existing Nix workspace discovery covers package tests, doctests, Clippy, documentation, feature variants and dynamic-dispatch checks. If a test includes a scale fixture, its exact path must also be admitted by the source filter; a local include alone is insufficient for isolated Nix builds.

Use [affected-check selection](local-ci.md) during implementation. The parent owns the final latest-main clean commit, matching native/Web assets, complete local Nix gate, exact checked SHA/actual exit evidence and PR publication. No complete migration, performance improvement or new gate pass is claimed by this preparation document.
