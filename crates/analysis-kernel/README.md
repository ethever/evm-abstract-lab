# analysis-kernel

This std-only crate owns typed read dependencies, publication, admission and joint
scheduling. Semantic values stay in explicit `Channel<Q,P>` routes or application
stores with `SlotMeta<V>` sidecars. There is no language IR, erased value registry,
or dynamic analyzer dispatch.

`Analyzer<Cx>` implementations state the `Read<Q,Route>`, `Accepts<Q,Route>` and
`Charges` capabilities they need. `Cons`/`Here`/`There` route providers explicitly;
`Pair` composes task and error types. `solve` runs those independent components on
the same dependency graph. Missing, absent and directory reads all participate.
The external Bounds/Masks fixture demonstrates mutual refinement in every initial
evaluation order; another fixture aggregates two independent provider routes.

## Semantic contracts

- `Ascending<JoinPolicy>` combines alternative behaviors. Joins cover both inputs
  and use semantic equivalence, independently of representation identity.
- `Descending<RefinePolicy>` exchanges guarantees for **one fixed concrete input
  universe**. Only explicitly `Registered` reduction/initialization rules can make
  descending writes, including the first write. They can write only their own
  generation. Registration accepts a semantic law; Rust cannot prove it.
- Widened/replaced upstream inputs require a fresh, independently seeded scope.
  Meeting old narrowed facts into that new input is invalid. A stable
  `GenerationEntry` lets consumers discover its replacement; keep an explicit
  owner obligation until required seeding and selector publication succeed.
  `replace_generation` atomically switches the entry, retires the old scope and
  wakes readers. Retired handles never reactivate.
- `Availability::Pending` is outstanding computation. `Absent` means no answer
  currently published on that route; it is not an immutable negative proof or a
  semantic Bottom. Both retain dependencies. First values can replace absence;
  absence cannot erase any provider's published value.
- Sealing attests the initial/external producer inventory is complete. Running
  work can still discover and register successors/providers. Truly future external
  work must retain an explicit obligation before sealing.

## Installation and interruption

Publication is atomic **per update**, rather than for a whole heterogeneous job:
pay pure preparation, reserve the full value/version/directory/fanout installation,
then install without another fallible operation. Earlier sound publications and
all successful charges survive a later refusal. A failed required operation must
propagate; dropping its active `Run` leaves an interrupted obligation. Policies
must not rely on a later publication to repair an unsound intermediate answer.

`AdmissionPolicy` supplies pure whole-state covering preparation plus detached
metadata. `Run::admit` performs the common preparation/reservation/installation
state machine. `publish_prepared_parts` or `publish_prepared_with` supports already
paid complete-state joins/acceleration, including coverage history and frozen
mode. These adapters must report semantic growth accurately. Nonsemantic metadata
can change without advancing the semantic version or waking consumers.

`Run::suspend` releases kernel/meter borrows while keeping the evaluation in
flight. `Kernel::resume` reacquires checked temporary access; only `finish` completes
it. Notifications received while running, suspended or admitted remain separate
pending obligations. An abandoned token keeps closure open. `closure_in` checks
one relative scope while the outer consumer remains in flight elsewhere.

`Kernel::fork` copies unpublished metadata after reserving its cost. The adapter
separately prepays and copies its semantic stores, including their sidecars, then
mutates only those detached values against the fork. `commit_with` performs the
last reservation before installing both graphs. Rejection leaves the live graph
unchanged; charges are never refunded. Source mutations and unrelated fork
lineages reject stale commits. Fresh allocation/evaluation identities prevent
escaped aborted handles from aliasing later allocations. Sidecar copies are for
detached snapshots; each graph view keeps one authoritative value per fact.

These atomicity guarantees cover typed errors and cancellation, not panics or
process allocation failure. The adapter remains responsible for semantic
rollback, state correlation and truthful coverage.

## Cost and closure

The same cumulative `Meter` is borrowed throughout. Work quotations must be
allocation-free and conservative; key/value clone and comparison costs are part
of the capability/policy contract. External slot operations explicitly quote
application data costs. Kernel allocations, dependency retention, directory
scans, metadata snapshots, retirement and notification fanout are also charged.
Costs that saturate are rejected, not silently accepted as bounded work.

Read edges are conservative within a live generation. Retirement wakes consumers
before releasing edges belonging to or targeting that generation, including
cross-scope readers. Identity tombstones and semantic channel cells remain owned
by the run until dropped; allocations stay bounded by the shared cumulative
ledger. `statistics` exposes retained graph sizes, dependencies, evaluations and
wake attempts. Worklist physical records include stale occurrences; deepest
selection preserves its `2 * records + 1` tariff and cancellation atomicity.

`LocalClosure::Limited` means an optional precision loss with sound coverage.
`Closure::Closed` additionally requires no queued, admitted, in-flight,
interrupted, missing-read, unsealed-scope or explicit semantic obligations.
Bounded `solve` never fabricates global closure from queue emptiness or a local
precision limit.
