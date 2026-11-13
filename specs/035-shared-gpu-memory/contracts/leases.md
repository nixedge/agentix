# Local lease and reclamation contract

The coordinator owns per-device admission. Workers use a permission-restricted
Unix socket and newline-delimited JSON (bounded messages); the coordinator
binary binds it. Worker identity includes PID and Linux process start ticks.
Requests contain a unique lease ID, exact device index and worker/model label.

## Reservations and ownership

Before native allocation a worker reserves a calibrated peak budget. Reservation
is charged while loading, resident, active and idle. A lease moves to the native
allocation owner; releasing a request guard or cancelling an async waiter does
not release model memory. Native destruction must complete before lease release.
Lease IDs make release idempotent. Unknown/invalid identity, stale observation,
overflow and capacity errors fail closed. Cancelled blocking acquisition drops
its returned lease without starting native allocation.

Model plans are explicit per model and contain the full loading/execution peak
(weights, context, scratch and growth). Missing plans in managed GPU mode fail
before load; uncalibrated defaults must not masquerade as measured estimates.

Device observations contain total/free bytes and per-process NVIDIA allocation
where available. Remaining reservations above measured process allocations
are retained as future-growth slack, and measurement credit is capped by the
worker's guaranteed persistent resident floor. Transient scratch measurements
never cancel future-growth slack. Registered process memory not represented
by leases and unrelated process usage are conservatively external. Unknown
per-process usage counts as external rather than an assumed zero.

## Reclaim and refill

Qwen has a non-reclaimable peak lease and a separate low-priority cache lease.
The cache lease identifies a local control socket. Before admitting a competing
reservation the coordinator requests cache release. The engine handles this on
its persistent inference thread between completed operations, joins outstanding
copies, synchronizes the GPU, invalidates graph pointers, marks all experts as
CPU-backed and drops the actual cache pool owners. Context and model mappings
remain intact. Acknowledgement reports remaining expert bytes; fresh device
measurement must confirm usable capacity before a competing allocation grant.

After release, cache refill requires a new grant, no pending competitor, a
cooldown and a minimum useful refill increment. Allocation happens only after
the new grant. A failed refill leaves a safe empty cache and rolls back the
unused grant; it never initiates whole-model reload or backend fallback.

## Queue, failures and recovery

Pending hard reservations use priority, FIFO for equal priority and bounded
deadlines. Cancellation stops waiting and removes the pending request. Reclaim
timeouts are failures, not freed-byte acknowledgements. Heartbeat loss never
proves allocations destroyed. Confirm process incarnation death before removing
orphan leases; fresh device usage still accounts for delayed frees externally.

Coordinator restart starts fail-closed for existing worker connections: workers
must re-register their full outstanding peak/cache leases before new allocation
or refill. Until reconciliation, measured live memory remains external usage.
No worker silently falls back to unmanaged operation on coordinator failure.

Each client also has a session identity; a second client cannot replace a live
worker's state, even with the same PID. Released unique lease IDs are tombstoned
until process death so an older snapshot cannot revive a destroyed allocation.
Unreadable process identity is retained conservatively, not treated as a crash.

Metrics expose per-worker peak reserved, device-measured resident allocation,
cache/reclaimable and pending bytes, plus reclaim outcomes, deadlines, device
free/total, external usage and sample success/age. Unknown measurements are
absent/error-marked, not fabricated zeroes.
