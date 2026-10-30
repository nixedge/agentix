# Native OurGroceries adapter

Agentix can use one existing OurGroceries shopping list as its grocery source. Reads fetch its current contents; single-item add, rename, purchased/reopen and explicit removal operate through the remote API. Other personal notes/tasks/lists stay local. There is no synchronization mirror, list creation, bulk clearing or automatic migration of an old local grocery list.

The unofficial protocol is based on [py-our-groceries1.6.0](https://github.com/ljmerza/py-our-groceries); this implementation is Rust with no Python dependency. Upstream attribution is in NOTICE.md and LICENSE.upstream. The protocol may change independently of Agentix.

## Discover the existing list

Provision OURGROCERIES_EMAIL and OURGROCERIES_PASSWORD in the runtime environment on the owner-controlled host. Do not put passwords in commands, Git, Nix expressions, build arguments or chat. The CLI deliberately exposes read-only operations:

```bash
nix run .#agentix-ourgroceries -- lists
nix run .#agentix-ourgroceries -- show LIST_ID
nix run .#agentix-ourgroceries -- inspect LIST_ID
```

The first prints existing shopping list IDs/names; the second prints a selected list. These commands contact the real service using the environment. Their output is personal data; they do not log or print credentials/session cookies. No live account was used in development tests.

`inspect` prints the selected list's raw JSON before typed decoding, for diagnosing protocol mismatches. It excludes the outer RPC envelope and does not print credentials or the session cookie held by the client. Its output contains personal grocery data. It never performs a mutation.

To install the CLI on the NixOS host:

```nix
environment.systemPackages = [
  inputs.agentix.packages.${pkgs.stdenv.hostPlatform.system}.agentix-ourgroceries
];
```

Installing it does not load the daemon's credentials into your shell. Reuse the runtime environment file for a read-only command:

```bash
sudo systemd-run --wait --pipe --collect \
  -p EnvironmentFile=/run/secrets/agentix-agent.env \
  /run/current-system/sw/bin/agentix-ourgroceries show LIST_ID
```

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

## Diagnose a failed read

A browser list URL ending in `/your-lists/list/ID` supplies the list ID. A configured integration can still fail during login, household discovery or fetching/decoding that list. The assistant distinguishes missing configuration from a failed remote read.

The Rust CLI prints redacted failure diagnostics to stderr. Run `show LIST_ID` with the same runtime environment file as the daemon to test a read without mutations. Diagnostics contain fixed stage/reason labels and HTTP status numbers, never credentials, cookies, response bodies or list contents. Successful `show` output does contain personal list data.

To enable daemon diagnostics in NixOS:

```nix
services.agentix.agent.extraEnv.RUST_LOG = "warn,agentix_agent::assistant=info";
```

After deploying/restarting, issue a fresh grocery read and inspect:

```bash
journalctl -u agentix-agent -b --since '5 minutes ago' --no-pager
```

Startup logs confirm the adapter is configured. Failure stages include `sign_in`, `household_page`, `getList` and the overall `snapshot` timeout. For example, `missing_session_cookie` means the login response did not provide the expected session; `invalid_or_missing_team_id` means household-page parsing failed; `invalid_list_fields` means the list response could not be decoded. These identify the failing boundary without assuming credentials are wrong. Existing request IDs replay their stored result; use a fresh request after correcting a failure.

## Build checks

```bash
nix build .#ourgroceries
nix build .#assistant-core .#assistant-api .#agentix-agent
```

HTTP fixtures and actual-Sled/HTTP integration tests cover reads, field preservation, stale/ambiguous targets, replay, lost responses and restart-after-dispatch. Live account acceptance is separate and needs the owner's configured account/list.
