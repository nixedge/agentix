# Plaud sync handoff

Updated 2026-11-17. Worktree: `/home/sam/home/agentix/037-plaud-sync`.
Branch: `037-plaud-sync`. Current implementation commit: `e0bed3a` (`feat: sync Plaud recordings through Android to server`), followed by handoff `b73eb24` and GATT firmware diagnostics `7acdd8a` (history was rewritten after the earlier hashes were recorded). The owner installed/tested the debug app on their remote phone. Production server deployment and real recording sync remain unverified. The owner handles merging into master.

## Owner's remote firmware checkpoint

Owner returned the v1 GATT report collected2026-11-17T21:06:29Z on Android API37. Services1800/1801/1910/180F are present;180A is absent, so the standard firmware read cannot work on this recorder. Owner approved passive advertisement diagnostics. The new **Copy advertisement report** action on the scan result collects advertised firmware and separate protocol, manufacturer IDs and a safe header prefix; serial, MAC, name and remaining payload are omitted. No GATT connection or vendor commands are involved. Request this v2 report next; the old GATT report can still show unavailable while advertisement decoding succeeds. Unknown/truncated layouts remain explicit. Automated tests use independent SDK-derived synthetic layouts; real advertisement values remain pending.

The phone and recorder are on the owner's laptop in another state; do not wait for local ADB or assume physical access. The debug app now adds **Devices → Scan → Read device information**, then **Copy device report**. It reads standard Bluetooth firmware/hardware/software/model/manufacturer values without any binding token, vendor handshake or reset. Missing/read-denied values are explicit, and the discovered UUID/property inventory survives later read timeouts. The owner should return the copied report before any unsupported-layout changes. Standard Device Information may be absent on this firmware; do not guess a vendor command or label protocol10 as firmware.

Owner-laptop build/sign/install instructions are in the [firmware quickstart](specs/037-plaud-sync/quickstart.md#firmware-check-on-the-owners-laptop). Existing signing identity must match the installed app. A prepared signed firmware APK can be transferred instead of copying the signing secrets. Firmware-reading hardware acceptance remains pending even after the debug build passes.

Latest advertisement-inspection debug APK: `/home/sam/home/agentix/.android-state/apks/agentix-android-037-advertisement.apk`.
SHA256: `92457baeae9725d80f8307e029d09ca1a4e6913e4a1b3fd1c2490ba92acf6c97`.
It supersedes the firmware-v1 and earlier server-sync APKs and includes all of their functionality. Canonical Nix build passed (78 JVM tests, lint zero errors, independent FFmpeg decode), output `/nix/store/7pvailql3hn3aakd5zpp0y46vxbqvad9-agentix-android-0.1.0`. Existing owner-key signature and 16 KiB alignment verified. Logs: `/tmp/agentix-037-advertisement-final.log` and `/tmp/agentix-037-advertisement-sign.log`.

An incremental Git source bundle is prepared outside the checkout at `/home/sam/home/agentix/.android-state/handoffs/agentix-037-advertisement.bundle`, containing this feature branch since base `5e66f35`. Transfer it to the laptop, run `git bundle verify /path/to/agentix-037-advertisement.bundle` in the existing repository (which must contain that base), then fetch its `refs/heads/037-plaud-sync` into the local feature branch. Use the normal sibling-worktree layout. If the laptop still has the previous pre-rewrite history, preserve it and fetch into a new branch/worktree rather than forcing over it. This enables a local build without pushing the branch; the APK can instead be installed directly after transfer.

## Implemented

- Explicit NB100 → Android BLE download, retained partials/resume, duplicate prevention, Ogg Opus output and Inbox playback/share/archive.
- Existing binding-token support and a separate, confirmed binding switch. Ordinary sync does not change binding or delete recorder audio. The owner confirmed this recorder was paired with the official Plaud app.
- Explicit phone → server upload opt-in, durable retries pinned to the original destination and recording identity, and separate upload/transcription/search status.
- Original audio stored as `recordings_path/blobs/<first-two-hash-characters>/<sha256>.ogg`.
- Existing personal Sled stores recording metadata, server identity, upload/source deduplication, transcription retries, full transcripts and projection jobs.
- Local FFmpeg decodes bounded 30-second chunks for the Agentix gateway's Whisper endpoint. Completed transcripts become Notes and searchable LanceDB projections. Failures retain originals and retry after five minutes.
- Recording routes share the existing listener and optional owner authentication: PUT/GET `/v1/assistant/recordings/{id}`, GET `.../audio` and `.../transcript`.

Limits: 64 MiB/four hours per upload; full transcript up to 2 MiB. No automatic audio deletion, garbage collection or execution of tasks from recordings.

## Verification completed

- 84 affected Rust tests passed; three existing live acceptance tests remain ignored.
- 73 Android JVM tests passed; Android lint reported zero errors.
- Real filesystem/Sled restart, replay, corruption and transcript-atomicity tests; real LanceDB search/rebuild tests; authenticated HTTP tests.
- Actual FFmpeg decoding into two bounded WAV requests to a mock transcription gateway. Real Whisper inference and speech quality remain untested.
- Production Clippy for affected crates, Rust formatting, changed Nix formatting and whitespace checks passed. Extra all-targets Clippy exposed existing `expect()` assertions in unrelated legacy agent tests; these were not suppressed.
- Final canonical build passed:

  ```bash
  nix build -L .#agentix-agent .#assistant-core .#assistant-lance .#assistant-api .#workspace-locks .#agentix-android --out-link result-037-validated
  ```

Server output: `/nix/store/pm5gj6k5ajmvml50cfdvln9bj5ciyx8k-agentix-agent-0.1.0`.
Android output: `/nix/store/8pjz1ryq3s4i7s5kyza4jx6crdkj4kjk-agentix-android-0.1.0`.
Build log: `/tmp/agentix-037-validated.log` (temporary, not versioned).

The owner authorized fixing stale Android metadata hashes with “fix it”. Five mutable upstream metadata hashes were refreshed; dependency versions and artifact hashes were unchanged.

## Ready Android update

Signed APK: `/home/sam/home/agentix/.android-state/apks/agentix-android-037-plaud-server-sync.apk`.
SHA256: `328b93e590bfb4ae7306bded24bc415a732505ada52eb16476c95d0fd5f3bdac`.
Uses the persistent owner signing key; signature and 16 KiB alignment verified. Final unsigned build matches the build used for signing byte-for-byte. Do not uninstall the app or replace its signing key.

With the phone connected:

```bash
nix develop .#android --command adb install -r /home/sam/home/agentix/.android-state/apks/agentix-android-037-plaud-server-sync.apk
```

No phone was attached to ADB during implementation.

## Remaining deployment work

1. Make the updated package available through the owner's normal NixOS configuration and deploy/restart `agentix-agent`.
2. In the existing nginx `/v1/assistant/` location, increase `client_max_body_size` to `64m`. Preserve Authorization forwarding, TLS and WireGuard restrictions. The application's instruction JSON limit remains 16 KiB. No extra port or listener is needed.
3. Optionally add to the existing generated agent TOML's `[assistant]` section:

   ```toml
   recordings_path = "/var/lib/agentix-agent/recordings"
   recordings_transcription_url = "http://127.0.0.1:11434"
   ```

   Use the actual Agentix gateway address exposing `/v1/audio/transcriptions` with a loaded Whisper model. Defaults are `assistant.db_path + ".recordings"` and `gateway_url`. The packaged agent supplies FFmpeg. No automatic model download or cloud fallback exists.
4. Preserve existing database/index paths and authentication settings. The service already provides writable `/var/lib/agentix-agent`; a custom location needs appropriate ownership and service access.
5. Back up personal Sled and original audio together in a consistent snapshot. LanceDB is derived and rebuildable.

Updated examples: [personal assistant](agentix-agent/examples/nixos-personal-assistant.nix), [OurGroceries](agentix-agent/examples/nixos-ourgroceries.nix), [deployment notes](agentix-agent/examples/README.md).

## Remaining physical acceptance

Tasks T011 and T017 are still open. No real recorder commands, owner-server uploads or Whisper calls were performed.

1. Install the same-key APK. Record and stop a short note, disconnect the official app's Bluetooth connection, then scan in Agentix → Devices.
2. Use the existing binding token, or review and explicitly confirm **Use this Plaud with Agentix** on the phone. That switch clears the old app binding/settings and preserves recordings; it is never automatic.
3. Verify actual firmware/protocol, real audio playback/duration, repeat-sync deduplication and interrupted-transfer recovery. Initial implementation supports protocol 10, mono 20 ms Opus records and compressed-byte offsets; the owner's firmware and offset geometry are unverified.
4. Enable the existing HTTPS backend in Settings, then explicitly enable recording uploads on Devices and select **Sync to server**. Verify original checksum/metadata, lost-response replay, restart recovery, local Whisper completion and transcript search over the actual phone/WireGuard route.
5. Verify existing WAV recording, reminders, assistant/wake and archive/share still work after updating.

See [quickstart](specs/037-plaud-sync/quickstart.md), [verification](specs/037-plaud-sync/verification.md), [tasks](specs/037-plaud-sync/tasks.md) and [protocol research](specs/037-plaud-sync/research.md). Reuse the existing spec/plan; implementation and automated verification are complete.
