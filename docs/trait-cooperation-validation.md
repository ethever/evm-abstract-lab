# Trait cooperation migration acceptance

This guide maps the reusable semantic kernel, independently defined cooperating analyzers and production EVM scalar carriers to committed implementation and regression evidence. The [architecture guide](trait-cooperation.md) explains the public API and its contracts. The gate and replay results below identify their historical source revisions. They do not validate later audit corrections; the final PR must bind its new clean head to fresh gate, artifact and performance evidence.

## Historical focused results

The integrated test run used clean commit `67ad711cb5980b054e0ea5c56dc1a3c0ee343c32`, with the same commit and clean state recorded after execution:

```bash
nix develop --no-update-lock-file --command cargo test --offline --locked \
  -p analysis-kernel -p evm-abstract --all-targets --no-fail-fast -- --include-ignored
```

The actual exit code was **0**: 51 target results, **1758 passed, 0 failed, 0 ignored**. This includes all 24 tests in the [kernel cooperation target](../crates/analysis-kernel/tests/cooperation.rs), all 16 tests in the [production cooperation target](../crates/evm-abstract/tests/cooperation.rs), and the [public EVM example](../crates/evm-abstract/examples/cooperative.rs).

Workspace Clippy used clean commit `bd1e43db15c7542de2ebe504ff4e3e408d81b8f1`, again unchanged and clean after execution:

```bash
nix develop --no-update-lock-file --command cargo clippy --offline --locked \
  --workspace --all-targets --all-features -- -D warnings
```

Its actual exit code was **0**. These are historical focused results. The later complete code-freeze gate below provides integrated coverage; final publication follows the [local Nix gate](local-ci.md).

The standalone interpreter source corresponding to commit `73e3e58aa9c2e18600f6858ea5b2f1df88198a4e` passed focused Nix kernel tests, strict Clippy, formatting and `no_dyn`: **74 tests passed**, including three complete-client interpreter cases. Source hashes bind the checked files to that commit. The witness supplements the production EVM checks; it does not replace the final integrated gate.

## Historical complete code-freeze gate

The complete implementation at clean commit `20505f19efe8cae5d2472d347a2e65b1ec5a830d` (tree `293f8eb927e610031b8ef48e08bed67606c333dd`) includes the performance repairs and caller cost contracts. It contains the synchronized main baseline `c81b04e96cbab358a52c519285ca6f8ddef2b063`. The same clean commit was recorded before and after:

```bash
nix flake check --keep-going --print-build-logs --no-update-lock-file
```

The actual exit code was **0** on `x86_64-linux` in **684.684 seconds**. All **38 browser runtime cases** passed with their matching server/Wasm/assets. The readback checked 409 attributes, 334 unique check derivations and 336 realized outputs, including feature/backend variants, examples, native checks, docs, `no_dyn` and browser reports. The incompatible `aarch64-darwin` and `aarch64-linux` systems were not executed. An earlier complete gate at `2e5addf9c4d069f46337f4a44859eb941e180cd4` also passed; it is historical evidence, not the gate for the repaired code.

The repaired source additionally passed 104 focused scalar/local tests, 281 world-byte tests, 17 product cost tests and the original guarded-byte **20,000,000 work** integration control. That control reached `Converged` at **7,185,583 work**, with zero frontiers. The tests retain exact cost/refusal checks, opaque successful result-query payment, cancellation and custom-carrier facts. The final PR separately records its final clean documentation/delivery commit and fresh gate; this record identifies the code and executables actually measured.

## Requirements and source evidence

The links identify executable assertions and the implementation they exercise. Each claim is limited to those cases and the frozen checks above; it is not a universal proof for arbitrary client implementations.

| Requirement | Committed evidence | Established behavior |
| --- | --- | --- |
| Reusable, language-independent kernel | [Public traits and exports](../crates/analysis-kernel/src/lib.rs), [crate dependencies](../crates/analysis-kernel/Cargo.toml), [complete client interpreter](../crates/analysis-kernel/tests/interpreter.rs) | The client supplies its own instruction language, program points, scoped state, capabilities, transfers and terminal outcomes. Independent interval and parity analyses jointly close a finite loop through public APIs. Both feedback-removal controls lose terminal precision. Concrete finite exploration checks states, outcomes and scoped CFG edges, including a missing-backedge negative control. |
| Independently defined analyzers perform joint solving | [Bounds rule](../crates/analysis-kernel/tests/cooperation/bounds.rs), [candidate rule](../crates/analysis-kernel/tests/cooperation/candidates.rs), [joint cycle](../crates/analysis-kernel/tests/cooperation/cycle.rs), [production product controls](../crates/evm-abstract/tests/cooperation/product.rs) | Bounds and candidates repeatedly exchange facts through registered reductions. Both initial orders reach `x=y=5` for the finite `x+y=10`, `y in 4..6`, odd-`x` case; joint concretization matches enumeration. Removing either information direction loses precision. Production bit/range removal controls also change actual value guarantees. |
| Explicit providers and typed availability | [Provider and availability cases](../crates/analysis-kernel/tests/cooperation/protocol.rs), [capability compile checks](../crates/analysis-kernel/tests/cooperation/capabilities.rs) | Explicit routes support multiple providers and late aggregate updates. Pending, absent, Top and Bottom remain distinct; missing capabilities fail to compile. Reads before publication create dependencies, and absence cannot erase another provider's answer. |
| Immutable-input scope and fresh generations | [Generation regressions](../crates/analysis-kernel/tests/cooperation/generations.rs), [recursive client-owned channels](../crates/evm-abstract/tests/cooperation/bridge/extra.rs) | Kernel epochs isolate equal context labels. Registered descending writes belong to their generation; authoritative selectors replace and retire old scopes. Growing recursive inputs recreate client stores and tasks, including fresh Pending reads at the same Body point. |
| Ordinary and recursive EVM use the shared kernel | [Production entrypoints](../crates/evm-abstract/src/analysis/engine.rs), [engine cooperation](../crates/evm-abstract/src/analysis/engine/cooperation.rs), [procedure adapter](../crates/evm-abstract/src/analysis/transfer/procedure/cooperation.rs), [feedback cases](../crates/evm-abstract/tests/cooperation/bridge.rs) | Default and extended entrypoints reach the same engine. World and Body execution consume scoped feedback on detached execution views. Recursive output growth creates new input generations; the tested summaries close with their external work accounted for. |
| Client-owned capabilities affect real execution | [Client contracts](../crates/evm-abstract/tests/cooperation/bridge/extra/contracts.rs), [provider](../crates/evm-abstract/tests/cooperation/bridge/extra/provider.rs), [consumer](../crates/evm-abstract/tests/cooperation/bridge/extra/consumer.rs), [public API tests](../crates/evm-abstract/tests/cooperation/bridge/extra.rs) | The independently authored components exchange a client-defined `ZeroGuards` fact through typed extra stores on the production kernel. The consumer runs before its provider, wakes automatically, and feeds interpreter-consumed StackFacts. Removing either component restores an impossible CFG path. Feedback wakes the provider; equal publications quiesce. |
| Actual alternate scalar carrier survives representation boundaries | [Custom excluded-word carrier](../crates/evm-abstract/tests/cooperation/carrier.rs), [boundary assertions](../crates/evm-abstract/tests/cooperation/boundaries.rs), [native World carrier regressions](../crates/evm-abstract/src/world/bytes/carrier_tests.rs) | A predicate absent from the standard observation survives whole-word memory, memory summaries, persistent/transient storage, checkpoints and Store summaries. Additional native cases cover conditional retained words, separately instantiated guarded stores and explicit conservative broadening controls. The state retains the actual custom element. |
| Calls, recursion, rollback and native reports preserve carrier semantics | [CALL cases](../crates/evm-abstract/tests/cooperation/calls.rs), [production carrier cases](../crates/evm-abstract/tests/cooperation/production.rs), [joint concrete oracle](../crates/evm-abstract/tests/cooperation/oracle.rs) | Semantic singletons resolve targets without finite-constant enumeration. Delegate checkpoints retain native facts and roll back child logs. Recursive summaries and resumed terminal returns retain exclusions. Native JSON contains the complete custom numeric object; a coarsened-policy control loses CFG precision under the same input assumptions. |
| Dependency updates and interruption retain obligations | [Late successors and running notifications](../crates/analysis-kernel/tests/cooperation/protocol.rs), [publication atomicity](../crates/analysis-kernel/tests/cooperation/atomicity.rs), [staged admission and suspension](../crates/analysis-kernel/tests/cooperation/staging.rs), [production resource case](../crates/evm-abstract/tests/cooperation/bridge.rs) | Late successors remain pending after initial sealing. Per-update installation and detached staging preserve live state on refusal. An external shared-Work stop after useful feedback yields `Incomplete`, attributable Work frontiers and no unfinished Return/Revert; cancellation returns typed `Cancelled`. |
| Shared work accounting includes custom operations | [Quotation and copy refusal](../crates/evm-abstract/tests/cooperation/boundaries.rs), [numeric cost regressions](../crates/evm-abstract/src/domain/product/cost_tests.rs), [literal and byte-writer costs](../crates/evm-abstract/src/analysis/transfer/budget/cost_tests.rs), [last-copy cancellation](../crates/evm-abstract/src/world/bytes/carrier_tests/cancellation.rs) | Tests check refusal before deep copies or literal construction, actual BYTE indices and writer passes, custom whole-quote fallback, owned/borrowed semantic parity and cancellation during the final native payload copy. These are accounting and semantic checks; the separate elapsed-time/RSS measurements are recorded below. |
| Production recursive and unknown-world semantics remain covered by adapter regressions | [Recursive suite](../crates/evm-abstract/tests/known_recursion.rs), [unknown-world suite](../crates/evm-abstract/tests/unknown_world.rs), [Store checkpoints](../crates/evm-abstract/tests/store_checkpoints.rs), [world joins](../crates/evm-abstract/tests/world_join_facts.rs) | The frozen all-target run includes admission, caller continuation, context, interaction and rollback checks. The EVM adapter remains responsible for unknown-target remainders, callback behavior, owner/static restrictions and recursive summary coverage. |

The public concrete oracle requires one terminal outcome to cover the checked data, owner storage, balances and logs of a revm execution; the delegate witness also checks rolled-back child effects. The CALL cases retain `(status=1, data=42)` and `(status=0, data=0)` as distinct alternatives, with concrete witnesses for forwarded transaction gas and zero gas. For correlation-sensitive validation, require one shared assignment across every claimed data, persistent/transient, balance and lifecycle field; bounded evaluators must reject unsupported expression fragments. Store logs describe possible source sites and payloads; exact event ordering and multiplicity are outside that representation.

## Semantic acceptance boundaries

- Composition uses explicit typed routes and registered sound reductions. Client authors own the semantic laws for their facts, joins, widening, equivalence and work quotations; Rust checks types and permissions and supplies no automatic proof of arbitrary client rules.
- Descending feedback is scoped to one immutable input generation. Production feedback currently constrains active stack slots on detached execution views. Global joins continue to cover incoming alternatives; a wider input requires fresh facts and tasks. Arbitrary dynamic plugins and general fact retraction are outside the current interface.
- The generic kernel accounts for dependencies and obligations. Production unknown-world effects, recursive semantics, semantic rollback and correlated state coverage remain EVM adapter responsibilities. Conservative control-flow edges follow those adapter rules.
- Local precision limits, resource refusal, runtime interruption and semantic closure have distinct meanings. `Closure::Closed` requires all mandatory obligations to be discharged; an arbitrary resource cutoff can leave detailed execution `Incomplete` or return a typed cooperation error. Current tests do not establish closed coverage for every cutoff or program.
- Optional `AllInputStates` root-terminal coverage has its own original-input and session identity requirements. It does not complete the detailed CFG or SSA. The ad-hoc cooperating resource case lacks AnalysisSession snapshot/revision, so it explicitly retains no coverage attachment even when enabled. [Session root-coverage regressions](../crates/evm-abstract/tests/root_terminal_coverage.rs) exercise their separate contract.

Read results at each delivery boundary: native exit or external termination, detailed status/frontiers and pending work, summary inputs/outputs and closed flags, optional root coverage, and report/consumer validation. Native exit 2 accompanies an Incomplete report. Preserve a whole-report guard rejection and any streaming supplement as separate observations.

## Historical controlled replay results

The [complete compact record](trait-cooperation-performance.json) retains all 84 sample identities and hashes, per-cell timing samples, memory/output sizes, semantic summaries, exact policy/argv templates, guards and Nix/ELF identities. A is original main `c81b04e96cbab358a52c519285ca6f8ddef2b063`; B is repaired code `20505f19efe8cae5d2472d347a2e65b1ec5a830d`.

The [fixed offline corpus](../examples/no-smt-scale/README.md) preserves unknown caller, call value, calldata content/length and unobserved accounts/storage. WETH/USDC use fixed observed worlds without RPC hydration. Each of seven cases ran three alternating pairs under each policy: **42 pairs / 84 serial executions**. Both roles used CPU 40 and the same ext4 evidence filesystem, without concurrent task builds/tests; existing node services remained active. The native timer includes JSON serialization through process reaping. The original 30/60/120-second deadlines, 2/8 GiB RSS limits, 20 GiB available-memory floor and 64 MiB whole-report guard remained unchanged.

Policies retain identical effective configuration between A and B: current-CLI `max_work=2000000000` with root coverage enabled, and fixed `max_work=1000000000000` with coverage disabled. Their accounting versions differ (**4 → 5**), so equal budget numbers do not mean equal work or precision. No cross-version work ratio is a speedup claim. Medians below are seconds and MiB:

| Policy | Case | Native seconds A → B | Peak RSS MiB A → B |
| --- | --- | ---: | ---: |
| 2B + coverage | unknown-input | 0.0102 → 0.0102 | 23.0 → 23.0 |
| 2B + coverage | dynamic-jump | 0.0124 → 0.0113 | 23.3 → 23.0 |
| 2B + coverage | loop | 0.0218 → 0.0175 | 23.1 → 23.0 |
| 2B + coverage | unknown-callback | 0.1194 → 0.1289 | 36.7 → 42.1 |
| 2B + coverage | memory-copy | 0.0100 → 0.0107 | 23.0 → 23.0 |
| 2B + coverage | weth-fixedstate | 0.3776 → 0.3017 | 92.1 → 74.2 |
| 2B + coverage | usdc-fixedstate | 0.7631 → 0.5352 | 206.3 → 148.2 |
| 1T, coverage off | unknown-input | 0.0102 → 0.0083 | 23.0 → 23.0 |
| 1T, coverage off | dynamic-jump | 0.0124 → 0.0121 | 23.0 → 23.0 |
| 1T, coverage off | loop | 0.0192 → 0.0240 | 23.0 → 23.1 |
| 1T, coverage off | unknown-callback | 0.1190 → 0.1328 | 36.8 → 41.6 |
| 1T, coverage off | memory-copy | 0.0090 → 0.0101 | 23.0 → 23.0 |
| 1T, coverage off | weth-fixedstate | 3.3964 → 3.6815 | 349.9 → 351.3 |
| 1T, coverage off | usdc-fixedstate | 97.5892 → 94.9678 | 1250.6 → 1356.0 |

All 84 native runs produced final outputs: 66 exited 0 and 18 exited 2 (`Incomplete`); none hit an external resource/time stop. All 84 reports passed separate strict streaming and input/configuration binding checks. **21 reports still failed the original 64 MiB whole-report delivery guard.** Their raw outputs and guard failures remain retained; streaming validation does not turn that consumer-boundary failure into a pass.

### Precision and remaining costs

- At 2B, both WETH and USDC remain detailed `Incomplete`. B expands **45 vs 56** WETH states and **107 vs 138** USDC states; it also records fewer detailed outcomes. Both versions attach an input-bound `AllInputStates` Return/Revert/Failure coverage summary. The attachment does not upgrade CFG/SSA closure, and its shape/binding check is not an independent gamma-inclusion proof. Lower time/output in this policy includes less detailed expansion and is not presented as a like-for-like speedup.
- At 1T, WETH is `Converged` in all samples of both versions, with the same **137 states, 160 edges, 91 diagnostics and 244 outcomes**. B performs 484 transfers versus 482. Its median is about **8.4% slower** than original main, rather than the roughly 2× regression found in the initial migrated candidate.
- At 1T, USDC remains `Incomplete` in all six runs, with **231 states, 300 edges, 40 diagnostics and 84 outcomes** in both versions. B performs 13,989 transfers versus 13,714. Its 919 frontiers are Work boundaries; A records 803 Work and 22 SummaryWork boundaries. Both procedure-summary groups remain open. B's median is about 95 seconds, with **1356 MiB RSS and 624.5 MiB JSON**, versus A's 1251 MiB and 524.7 MiB. This does not solve detailed recursive convergence or eliminate memory/output growth.
- The small unknown-callback case keeps 27 states, 53 edges and 16 outcomes. B has three closed procedure-summary groups versus two, and 81 transfers versus 79. Its extra time, memory and output are visible in the table/record; the migration is not advertised as a universal performance improvement.

### Diagnosed regression and equivalent repair

The initial migrated candidate at `2e5addf` used the injected 512-member/16-round numerical policy in local word evaluation; the older implementation silently constructed a smaller default policy internally. Profiling localized most extra CPU to word SHL. Captured input showed 322 fallbacks containing 31 unknown bytes and a last byte in `{0,32,64,96,128,160,192,224}`. The last byte carries a real low-five-zero constraint. The whole-word fallback performed 10,304 SHLs and 10,304 ORs, repeatedly producing 256-member finite covers; it was not running all 16 reduction rounds.

The repair keeps the configured policy and its facts: exact congruence GCD/normalization improvements, plus domain-owned packing only for certified exact byte cubes or validated singleton bytes. Custom policies decline unless they supply their own equivalent operation and price. Both local and world callers prepay the hook; opaque result singleton queries have their separate price. [Scalar controls](../crates/evm-abstract/src/domain/byte_projection/tests/packing.rs), [local custom-carrier controls](../crates/evm-abstract/src/domain/relational/local/state/tests/word_packing.rs) and [world payment/query controls](../crates/evm-abstract/src/world/bytes/numeric/packing/tests/payer.rs) exercise these boundaries, including the balanced world fold and interval-only singleton compatibility.

A separate same-cost diagnostic pair changed WETH from **7.370 to 3.665 seconds**. Complete field comparison found only one consistent source-proved process namespace rename and the visible `/work` change `49926037341 → 36244266491`; all other fields and serialized record ordering matched. The old candidate's USDC run hit 120 seconds without a report; the repaired run returned a strictly validated `Incomplete` report in **94.058 seconds**. These single pairs are diagnostics, not substitutes for the 84-run original-main comparison above. No semantic equality is inferred against an absent timeout report.

The [identity normalizer](../scripts/normalize_identity_scopes.py) only permits its source-proved namespace renaming. Counters, diagnostics, aliases, source relationships and ordering remain visible. Benchmark inspection does not itself prove whole-program concrete inclusion; the semantic and concrete regression suites establish their documented cases separately.

## Delivery verification

The final PR records its exact clean head, latest main baseline, full-gate command/exit and remote head/base/mergeability readback. Documentation-only commits still receive a fresh final gate under [the local process](local-ci.md). The [26-case cache-boundary probe](../scripts/check_cache_boundaries.json) verifies actual dependency selection, including kernel implementation versus client-test changes. Artifact readback binds the release CLI/server and Wasm/browser results to their realized derivations; reusing an output requires the same producer identity. These checks do not replace the preserved goal of obtaining sound closed models through appropriate coarsening, or imply that every current budget cutoff already provides one.
