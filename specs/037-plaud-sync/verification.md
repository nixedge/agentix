# Verification — 2026-11-17

## Advertisement diagnostics v2 follow-up

Owner's real GATT report (collected21:06:29Z, API37) establishes service discovery with1910 command/notify and180F battery but no180A Device Information service. Firmware/protocol values remain unknown. Owner approved the next passive advertisement approach explicitly.

Added bounded manufacturer firmware parsing, separate unauthenticated protocol display, manufacturer ID inventory, capture timestamp/RSSI and copyable safe prefix. No GATT connection/commands/token required; no serial, address, device name or complete manufacturer data copied. Eight advertisement tests now cover independent SDK-derived fixtures, multi-byte firmware values, distinct protocol branches, truncation/malformed layouts, manufacturer bounds and redaction. There is no real owner-advertisement fixture yet.

`nix build -L .#agentix-android --out-link result-037-advertisement-final` passed:78 JVM tests total, Android lint zero errors (22 warnings,8 hints) and independent FFmpeg decode. Output `/nix/store/7pvailql3hn3aakd5zpp0y46vxbqvad9-agentix-android-0.1.0`; log `/tmp/agentix-037-advertisement-final.log`. Same-owner-key signed/verified16KiB-aligned update `/home/sam/home/agentix/.android-state/apks/agentix-android-037-advertisement.apk`, SHA256 `92457baeae9725d80f8307e029d09ca1a4e6913e4a1b3fd1c2490ba92acf6c97`; signing log `/tmp/agentix-037-advertisement-sign.log`. No additional Rust checks for this Android/documentation-only follow-up. Owner should install this newer build and return **Copy advertisement report**; successful real firmware decoding remains pending.

## Remote firmware inspection follow-up

Owner clarified that the phone/Plaud are on their laptop in another state. Added token-free **Read device information** and **Copy device report**: serialized standard Bluetooth firmware/hardware/software/model/manufacturer reads, explicit missing/failed values, and bounded GATT UUID/property inventory retained on timeout/cancellation. No vendor commands, subscriptions or binding operations occur in this mode. Actual recorder firmware remains unknown until the owner runs it and shares the report.

Final follow-up `nix build -L .#agentix-android --out-link result-037-firmware-final` passed. All73 existing JVM tests passed; lint zero errors (21 warnings,8 hints); independent FFmpeg Ogg decoding passed. Output `/nix/store/6zikisa342hrn2yd1kaa2rng5ca7gxk5-agentix-android-0.1.0`. Log `/tmp/agentix-037-firmware-final.log`.

Signed/verified same-owner-key debug APK: `/home/sam/home/agentix/.android-state/apks/agentix-android-037-firmware.apk`, SHA256 `7800f576c930d86f1ccb79d0e99beafbcac2d7d885a38e50e223f05e47ace4e8`. Signature and16KiB alignment passed. This supersedes the earlier APK below. Owner-laptop instructions are in quickstart and root handoff. No installation or physical read was performed here. No additional Rust checks were needed for this Android/documentation-only follow-up.

## Implemented behavior

Explicit NB100→phone sync with conservative protocol10 parsing, retained BLE partials, duplicate prevention, playable Ogg Opus and existing Inbox/play/share/archive. Existing token setup or a separately confirmed binding switch; ordinary sync never unbinds or deletes recordings.

Approved phone→server extension: explicit upload opt-in, durable immutable/pinned retries, original SHA256-addressed files and Sled metadata, authenticated upload/read/status/transcript API, bounded local FFmpeg/Whisper transcription, durable failure backoff, and atomic full-transcript/Note/projection work for existing LanceDB search. Inbox distinguishes local/server/transcription/search state. Local originals are retained; recordings do not execute tasks.

## Passed locally

- 84 affected Rust tests pass across assistant-core, assistant-sled, assistant-lance, assistant and agent;3 existing live acceptance tests remain ignored. Final full run `/tmp/agentix-037-all-affected-tests.log`; additional strengthened API/backoff coverage `/tmp/agentix-037-api-all-final.log` and `/tmp/agentix-037-recording-acceptance.log`.
- Real Sled/filesystem tests cover restart/replay, source conflicts, checksum/container corruption, missing blobs, transcript atomicity and retained projection jobs.
- Real LanceDB integration verifies transcript FTS and rebuild.
- HTTP tests verify authorization before effects, upload/replay/conflict, bodies above16KiB instruction limit, invalid Ogg, original retrieval and persisted transcript retrieval.
- Actual FFmpeg decoding spans a30-second boundary and generates two WAV multipart requests under1.1MiB each to a local mock gateway. The failure test preserves original audio and durably defers retry. Real Whisper speech quality/inference is not claimed.
- Canonical Android Nix build passes with73 JVM tests, including11 Plaud and3 upload tests (loss of response, restart, destination pinning, identity validation, duplicate avoidance/status refresh). Android lint has zero errors; general warnings remain.
- Independent Nix postCheck decodes the generated Ogg Opus silence fixture to exactly1280bytes of16kHz mono16-bit PCM (40ms).
- Production Clippy for affected crates with `-D warnings`, Rust formatting, Alejandra for changed Nix files and Git whitespace checks pass.
- An extra `--all-targets` Clippy run exposed existing `expect()` assertions in unrelated legacy agent tests; those were not suppressed or rewritten. The new worker's int-plus-one lint was fixed. Standard production Clippy passes.

## Canonical packages

Final reproducible batch passed with exit0:
`nix build -L .#agentix-agent .#assistant-core .#assistant-lance .#assistant-api .#workspace-locks .#agentix-android --out-link result-037-validated`.

Log: `/tmp/agentix-037-validated.log`. All six requested outputs succeeded: server package, assistant core/storage tests, real LanceDB tests, release HTTP API tests, workspace lock consistency and Android package. The API check passed14 tests with3 existing live acceptance tests ignored. The packaged server's `--help` smoke check also passed.

Server package: `/nix/store/pm5gj6k5ajmvml50cfdvln9bj5ciyx8k-agentix-agent-0.1.0`.

The completed Android build is `/nix/store/8pjz1ryq3s4i7s5kyza4jx6crdkj4kjk-agentix-android-0.1.0`; its signed update is:
`/home/sam/home/agentix/.android-state/apks/agentix-android-037-plaud-server-sync.apk`.
SHA256: `328b93e590bfb4ae7306bded24bc415a732505ada52eb16476c95d0fd5f3bdac`.
The existing persistent owner key was used and the wrapper verified signature and16KiB alignment. The final unsigned APK is byte-for-byte identical to the earlier build used for signing. Signing log `/tmp/agentix-037-server-sign.log`.

## Repairs

Owner authorized repair with “fix it”. Independently verified and refreshed five mutable upstream metadata checksums: AndroidX core/compose, MediaPipe, Google master index and Play SDK index snapshot. Library versions and artifact checksums are unchanged. One wrong-session fixture accidentally retained its original byte; corrected it to actually differ. The new Inbox status needed a local immutable reference for Kotlin's cross-module smart cast. The first standalone storage-test compile needed its fixture file removal to use the existing blocking pool rather than an undeclared Tokio fs feature. All were followed by successful local checks.

## Pending external acceptance

ADB again reports no attached phone. No binding commands, real recording downloads, uploads to the owner's server, real Whisper model calls or phone installation were performed. Hardware/firmware compatibility, offset geometry, real audio quality and actual disconnect recovery remain T011. Backend/nginx deployment and real phone→server upload/replay/transcription/search remain T017. The examples document64MiB nginx bodies; instruction JSON stays16KiB. Back up Sled and original audio together; LanceDB is derived. No push, merge or deployment.
