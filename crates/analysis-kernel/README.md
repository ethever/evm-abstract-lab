# analysis-kernel

This std-only crate owns typed read dependencies, publication, admission and joint
scheduling. Semantic values stay in explicit `Channel<Q,P>` routes or application
stores with `SlotMeta<V>` sidecars. There is no language IR, erased value registry,
or dynamic analyzer dispatch.

`Analyzer<Cx>` implementations state the `Read<Q,Route>`, `Accepts<Q,Route>` and
`Charges` and `ReadGeneration` capabilities they need. `Cons`/`Here`/`There` route providers explicitly;
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
  Each target has one authoritative entry; clones share its current selection
  within a kernel view, and forks keep separate staged selections. Duplicate
  ownership, taking another entry's target, selecting a stable owner, or directly
  retiring either a selected scope or a stable owner returns `GenerationInUse`.
  Equal owner/target is `WrongScope`. Stable owners live until the kernel is
  dropped; independent chains may share the same stable owner.
- Create a selected generation in this order: seed/register/seal its immutable
  input, publish its authoritative entry, track that entry from its stable owner,
  then drain cross-scope rules. Only selected generations acquire the automatic
  invalidation contract; earlier reads are not retroactively reinterpreted.
- A selected generation adopts an external semantic assumption when it reads an
  actual `Value` from a typed channel or sidecar. It also depends on that source
  scope's validity. Ascending semantic growth invalidates those adopters and
  their transitive adopters, including users of descending guarantees. Invalidation
  marks each affected selection stale once, wakes its entry readers/owner and
  removes obsolete local queue obligations. Unrelated generations stay fresh.
  `generation_stale` is a small metered read of that authoritative selection.
  `scope_stale` provides the same callback check from a checked scope identity;
  known retired scopes also return true, and meter failures remain errors.
- `observe_slot` tracks an initialized native anchor and its cumulative semantic
  premise without cloning its payload. Register this observation after entry
  selection, and compare its returned version with the version that funded the
  immutable input snapshot. Assumptions persist across all evaluations until
  retirement, even when a later evaluation stops reading an earlier premise.
- Descending refinement within the same input universe only wakes readers; it
  does not invalidate a generation. Same-scope feedback also remains ordinary
  propagation. Pending/Absent observations and `current_generation` are wake
  dependencies, not adopted semantic assumptions. Changing a selector routes
  readers to its new scope without treating the scope number as semantic growth.
  Directory enumeration is a topology assumption: adding a resolved key can
  invalidate its adopters even when all per-key payloads are unchanged. Each
  channel keeps a separate directory for each scope.
- A stale selection keeps closure Open independently of its queue. Its owner must
  seed a fresh input and call `replace_generation`, even if a cached raw-input
  stamp did not change. `current_generation`, typed `query` and `read_slot` expose
  stale selections/guarantees as Pending. They retain wake dependencies without
  adopting the old values. `keys` has no Pending variant, so stale enumeration
  returns recoverable `StaleGeneration`. Consumers reread the stable route after
  a wake. Automatic FIFO/deepest admission skips obsolete jobs; explicit
  `begin`, `resume` or operations on a now-stale Run return `StaleGeneration`.
  Discard those obsolete tokens/runs and let the owner perform replacement.
  After retirement those job handles return `InvalidJob`. A new scope encountering
  a stale peer does not import its old guarantee, preventing A/B reseed churn.
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
mode. These sidecar updates are ascending by contract and must report semantic
growth accurately. Nonsemantic metadata
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
of the capability/policy contract. `Capability::drop_work` quotes possible
last-owner destruction separately from clone work (defaulting to `value_work`).
Typed replacement pays the old retained value's drop quote before assignment;
prepared sidecars include live-value/evidence destruction in `install_work`.
Incoming/prepared temporaries belong to the caller/policy preparation quotation,
including their destruction on failure or equivalent publication.
Arc/shared-handle carriers must override it when a cheap clone can own a deep
payload. Key quotations include key destruction as well as clone/comparison. External slot operations explicitly quote
application data costs. Kernel allocations, dependency retention, directory
scans, metadata snapshots, retirement and notification fanout are also charged.
Costs that saturate are rejected, not silently accepted as bounded work.

Read edges are conservative within a live generation. Retirement wakes consumers
before releasing edges belonging to or targeting that generation, including
cross-scope readers. Retired task identities remain checked tombstones, while
their immutable task `Arc`s are released. Detached task handles and live forks
keep their own ownership until dropped. Opaque task construction and destruction
belong to the adapter's ledger; the kernel charges graph cleanup and Arc release.
After retirement, `reclaim_channel_scope(&mut channel, retired, meter)` can
release that scope's typed keys and payloads. It validates channel and historical
scope identity, rejects active scopes, and reserves traversal, key drops and
`drop_work` for every retained value before deleting any cell. Refusal, overflow
or cancellation leaves all cells unchanged. Other scopes and their dependencies
remain intact; repeated reclamation returns zero. Scope/fact tombstones reject
retired or aborted handles rather than aliasing a later generation.

Scope-owned fact indexes and job-owned provider indexes target retirement at its
own graph and adjacent edges. Invalidation follows reverse semantic edges once
per affected generation, reserving traversal and all resulting wakes before
installation. Neither operation scans the full fact/provider history. The cost
is proportional to touched live edges/jobs plus logarithmic index operations;
reports and explicit snapshots still traverse retained metadata and are paid
separately. `statistics` exposes retained identity counts, dependencies,
evaluations and wake attempts.

`Worklist` holds one dense node and one lookup entry per live pending obligation.
Satisfaction unlinks and removes its node immediately, so requeueing appends at
the current FIFO tail. A moved node's links and map index are repaired in
O(log live) work. FIFO/deepest selection visits at most the live nodes and then
removes one in O(log live); positive deepest priorities use FIFO ties. In-flight
notifications keep their own nodes until eligible, including through admission,
suspension and completion. There are no historical queue records or sweeps.
Spare storage follows peak concurrent live nodes; snapshots copy only live nodes.

Enqueues reserve map/link work and potential geometric vector growth; fanout
quotes the whole batch before installing any values or wakes. Direct `begin` and
retirement reserve removal work. FIFO selection reserves one visit per live node,
deepest reserves two, and both include map/link removal. Selection checks
cancellation through the final callback before changing the queue. Denial or
cancellation preserves membership, FIFO order and priorities; earlier charges
remain spent. Selection charges depend on live work, independent of satisfied
history.

`LocalClosure::Limited` means an optional precision loss with sound coverage.
`Closure::Closed` additionally requires no queued, admitted, in-flight,
interrupted, missing-read, unsealed-scope, stale-generation or explicit semantic
obligations.
Bounded `solve` never fabricates global closure from queue emptiness or a local
precision limit.

`solve` also pays for its final report: it reserves a metadata pass before
counting nested readers, then reserves the checked full traversal cost before
inspecting closure and local limits. Empty queues and zero-step calls still
check the same root meter for refusal/cancellation. The read-only `closure`,
`closure_in`, `local_limits` and `statistics` APIs remain caller-accounted.
