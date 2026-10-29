# Native OurGroceries adapter

Agentix can use one existing OurGroceries shopping list as its grocery source. Reads fetch its current contents; single-item add, rename, purchased/reopen and explicit removal operate through the remote API. Other personal notes/tasks/lists stay local. There is no synchronization mirror, list creation, bulk clearing or automatic migration of an old local grocery list.

The unofficial protocol is based on [py-our-groceries1.6.0](https://github.com/ljmerza/py-our-groceries); this implementation is Rust with no Python dependency. Upstream attribution is in NOTICE.md and LICENSE.upstream. The protocol may change independently of Agentix.

## Discover the existing list

Provision OURGROCERIES_EMAIL and OURGROCERIES_PASSWORD in the runtime environment on the owner-controlled host. Do not put passwords in commands, Git, Nix expressions, build arguments or chat. The CLI deliberately exposes read-only operations:

```bash
nix run .#agentix-ourgroceries -- lists
nix run .#agentix-ourgroceries -- show LIST_ID
```

The first prints existing shopping list IDs/names; the second prints a selected list. These commands contact the real service using the environment. Their output is personal data; they do not log or print credentials/session cookies. No live account was used in development tests.

## Enable in agentix-agent

Add to the existing TOML, using an ID from discovery:

```toml
[assistant.ourgroceries]
list_id = "EXISTING_LIST_ID"
# Defaults; these are names of runtime variables, not credential values:
email_env = "OURGROCERIES_EMAIL"
password_env = "OURGROCERIES_PASSWORD"
```

Continue supplying the existing assistant db_path/index_path, gateway/model, legacy db_path, [ntfy] and [scheduler] sections. With this subsection omitted, existing local-list behavior is preserved. Startup rejects missing/invalid configured runtime credentials; the remote login itself is lazy, so the service does not have to contact OurGroceries to start. A remote outage does not turn a grocery request into a local save.

For NixOS, use a secret-manager runtime environment file, for example `services.agentix.agent.environmentFile = "/run/secrets/agentix-agent.env";`, and add `assistant.ourgroceries.list_id` to your generated non-secret TOML. Include required owner bearer credentials if your current configuration explicitly sets assistant.token_env. The personal-assistant example derives that requirement whenever an environmentFile is set; use the standalone OurGroceries example for an explicit WireGuard-only configuration with grocery credentials and optional bearer authentication.

See `../agentix-agent/examples/nixos-ourgroceries.nix`. Keep credentials on the actual host's persistent/secret-manager storage, not a jail's temporary home. Do not make credential files part of a Nix derivation.

## Retries and confirmation

Before a remote write, Agentix persists the selected list, item snapshot, requested operation and a dispatch marker in Sled. It validates the remote item again, sends at most one mutation attempt, then reads back the intended state. Renames preserve category/note/purchase state; completion uses an explicit boolean. Ambiguous identical items require clarification; explicit IDs use `id:ITEM_ID`.

The reference API exposes no demonstrated idempotency key or compare-and-swap. A lost response or crash after dispatch yields a durable reply saying the change was not confirmed. Retrying that same request returns its stored result and never resends the write. Read your list before deliberately repeating an uncertain change with a new instruction. Fresh preflight checks cannot make concurrent remote edits atomic.

The deployed Android v1 client needs no change: OurGroceries replies use Answer with zero local receipts/records and explicitly name the remote source. This lets it clear its pending envelope even after an honestly uncertain remote outcome and issue a fresh read. Local Saved responses retain their actual Sled receipts. Journal entries audit the affected operation/item; they are not an independent local grocery list.

New grocery journal states are additive and validated; old local entries remain readable. Older Agentix binaries cannot read new grocery turn states, so keep the new binary when reopening a store containing those turns. No existing local groceries are imported, overwritten or deleted automatically.

## Checks

```bash
nix build .#ourgroceries
nix build .#assistant-core .#assistant-api .#agentix-agent
```

HTTP fixtures and actual-Sled/HTTP integration tests cover reads, field preservation, stale/ambiguous targets, replay, lost responses and restart-after-dispatch. Live account acceptance is separate and needs the owner's configured account/list.
