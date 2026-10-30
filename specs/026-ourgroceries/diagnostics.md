# Remote read failure diagnostics

The owner configured runtime credentials and a browser-derived list ID but received a reply combining “not configured” and “unavailable.” The snapshot path discarded adapter errors, and the CLI returned only a generic error. No live account failure cause has been established.

Keep configuration and remote availability distinct in replies. Emit redacted stage/reason diagnostics for login/session, household discovery, RPC transport/status/envelope, list decoding/validation and the overall snapshot timeout. Initialize a warning subscriber in the read-only CLI and record adapter activation in the daemon. Keep credential/session/body/list data out of diagnostics, and preserve existing error contracts, single-attempt mutation semantics and request replay.

Verify failure classification and redaction with synthetic HTTP responses, and verify that configured failures produce a different reply from missing configuration without saving local records. Retain existing protocol, field-preservation and replay regressions. Document the required daemon log filter and CLI read workflow; real account diagnosis still requires the resulting host log stage/reason.

Verification checkpoint: 36 targeted tests pass (10 client, 17 application and nine hosted assistant tests; two manual model tests remain ignored). Strict all-targets Clippy for the client/application, production-agent Clippy and formatting pass. The canonical `ourgroceries` package/check passes all 10 client tests. No live account was accessed or root cause established by these synthetic checks.
