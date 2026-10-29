# Verification checkpoint — 2026-10-29

The integration is based on master `ecea4ae`. OurGroceries is the configured grocery authority; no existing local groceries are migrated or deleted. No Android update is required for the existing v1 response contract.

## Completed checks

- 66 targeted Rust tests passed across agentix-agent, agentix-assistant, agentix-assistant-core, agentix-assistant-sled and agentix-ourgroceries. Two manual model tests were ignored in this ordinary run, then explicitly run below.
- Installed Qwen passed all 19 synthetic acceptance cases: 13 existing assistant cases and six remote grocery selections. The final retry also passed both suites. These checks contacted the local model, not a real OurGroceries account.
- Canonical Nix builds/checks passed for `assistant-core`, `ourgroceries`, `assistant-api`, `agentix-agent` and `workspace-locks`. The final core/server build and targeted tests include the bounded Unicode grocery preview.
- Strict Clippy passed for all targets of the new adapter, core, Sled and assistant crates, and the production agent target. Formatting and whitespace checks passed.
- NixOS example evaluation verified loopback port 8767, its nginx upstream, runtime environment file, optional bearer authentication and required legacy configuration sections.
- Comparing existing package derivations against master left 15 unrelated packages unchanged, including agentix-llama. Only the five assistant dependents changed. CUDA derivation evaluation passed; a CUDA build was not run.

HTTP fixtures and actual-Sled integration cover all single-item operations, field preservation, stale/ambiguous targets, durable replay, lost responses, restart after dispatch without a second write, retained local groceries and bounded Unicode replies.

## Limits

The broader agent all-targets Clippy run reports 80 existing unwrap/expect lint errors in unchanged legacy tests under google, http, scheduler and store. These remain outside this feature. Full workspace/GPU tests were not run.

Real account acceptance (T006) remains pending owner-provisioned runtime credentials, list selection and an authorized live test. Synthetic tests do not establish compatibility with the owner's live account or current remote service behavior.

## Deployment handoff

Follow [the adapter setup](../../agentix-ourgroceries/README.md) to discover the existing list with the read-only Rust CLI and configure its ID. Keep credentials in the host's runtime secret file. The [standalone NixOS example](../../agentix-agent/examples/nixos-ourgroceries.nix) uses port 8767 and explicitly optional owner bearer authentication.

After deployment, read the selected list, then perform agreed single-item add, rename, purchase/reopen and removal checks while verifying the same list in OurGroceries. Inspect an uncertain outcome before issuing a new mutation. Older binaries cannot read the new grocery journal states; see the adapter's rollback notes before downgrading.
