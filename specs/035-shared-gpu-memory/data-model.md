# Allocation ownership and transitions

A process identity is `(pid, /proc start_ticks)`; a worker also has a unique
client session and role. Another client cannot replace a live role's snapshot.
Permission or parse errors reading `/proc` do not prove death: old leases stay
charged, while new requests require verified identity.

Each allocation has a unique ID, model label, physical device index, reserved
peak bytes and a guaranteed persistent resident floor. Cache allocations also
identify their management socket. A model plan binds the peak to ordered model
SHA256 values and supported context/batch limits. Plans are operator-calibrated,
not inferred from GGUF file size in managed mode.

```text
pending -> reserved -> loading -> resident <-> active -> native destruction -> released
                  \-> load failure/cancellation -> native cleanup -> released
cache: granted -> populated -> revoke persistent floor -> synchronized release
       -> empty -> cooldown -> new grant -> populate -> publish actual floor
```

Resident/active/idle models keep the same lease; dropping an HTTP or pool guard
does not free the lease early. Llama and Whisper native threads own their lease
and local pool reservation through destruction of weights and contexts. Qwen's
native thread retains its fixed lease; cache observers cannot keep a dead
engine's allocation lease alive after actual cleanup.

Accounting for a device observation:

```text
growth_slack(pid) = reserved_peak(pid) - min(guaranteed_floor(pid), measured_resident(pid))
grantable = max(0, device_free - safety_reserve - sum(growth_slack))
```

Missing per-process measurements credit zero resident bytes. The driver reports
whole MiB, so admission rounds measured free/resident values down. Only a
guaranteed persistent floor offsets a peak; transient scratch peaks cannot
erase future-growth slack. A zero floor is safe and conservative, but can leave
some VRAM unused. Live context and scratch reserves are never cache capacity.

Normal release sends the coordinator its release before waiting for the local
snapshot lock, so an acquisition waiting in the same client can progress. Unique
released IDs remain tombstoned while the owner process exists: an older
heartbeat cannot resurrect destroyed memory.

On coordinator restart, new grants wait for every configured role's full
snapshot. Client snapshot/grant updates are serialized so a grant cannot be
forgotten between delivery and native allocation. Known dead roles need not
restart before other workers can proceed; fresh device usage still accounts
for delayed GPU cleanup as external usage. Heartbeat loss alone never frees a
lease. Snapshots retry once per second without enabling unmanaged fallback.

Pending allocations use priority, FIFO within equal priorities and aging every
five seconds. Admission waits are bounded to 30 seconds, plus bounded driver
and local-control I/O. Cache refill is best effort: no pending allocation,
30-second cooldown after reclamation/shrink and at least 256 MiB of new room.
The worker checks refill every five seconds. Failed refill leaves CPU experts
available and refunds unused cache space.
