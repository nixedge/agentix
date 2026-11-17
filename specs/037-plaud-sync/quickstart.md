# Plaud recording sync

## Firmware check on the owner's laptop

The phone and Plaud are with the owner elsewhere. Build/install on that laptop; no ADB connection to this development host is expected.

The updated debug app has **Devices → Scan → Read device information** for each discovered recorder. Disconnect the official app's Bluetooth connection, wake the recorder, then select this action before entering a token or switching binding. It reads firmware, hardware, software, model and manufacturer strings over Bluetooth, requires no Plaud binding token, and sends no vendor commands. Use **Copy device report** and return the text for remote diagnosis. Service/characteristic UUIDs are captured even if a later read times out. Values absent or denied by the recorder are explicit; protocol number and advertisement project code are not firmware versions.

For a local build on a Linux laptop with Nix, from this branch:

```bash
nix build .#agentix-android --out-link result-plaud-firmware
# Set this to your existing persistent signing state on the laptop:
android_state="$PWD/../.android-state"
nix run .#agentix-android-sign -- \
  result-plaud-firmware/share/agentix-android/agentix-android-debug-unsigned.apk \
  "$android_state/apks/agentix-plaud-firmware.apk" \
  "$android_state/signing/debug.keystore" \
  agentix-debug \
  "$android_state/signing/keystore-password"
nix develop .#android --command adb install -r "$android_state/apks/agentix-plaud-firmware.apk"
```

Create the output directory first and use a fresh APK filename. The signing key must match the installed app; no secrets belong in the checkout or Nix store. Alternatively, transfer the prepared signed firmware-inspection APK from this host and install it directly with `adb install -r`; no signing key transfer is needed for that route. The canonical package supports Linux; a macOS laptop needs a Linux Nix builder for the build and local Android platform-tools for USB installation. See [Android build/install notes](../../agentix-android/README.md).

## Build and update

From `/home/sam/home/agentix/037-plaud-sync`:

```bash
nix build .#agentix-android .#agentix-agent .#assistant-core .#assistant-api .#assistant-lance .#workspace-locks
```

The prepared, verified APK uses the existing persistent owner key:
`/home/sam/home/agentix/.android-state/apks/agentix-android-037-plaud-server-sync.apk`.

Update in place on the host with the phone connected:

```bash
nix develop .#android --command adb install -r /home/sam/home/agentix/.android-state/apks/agentix-android-037-plaud-server-sync.apk
```

Do not uninstall or replace the signing key: recordings, reminders and pending operations must survive. The APK has not been installed by this session.

## Server deployment

Update the existing agentix-agent package and restart its service through the owner's normal NixOS deployment. No extra listener or nginx location is required: the recording routes use `/v1/assistant/`. Increase the existing nginx location's `client_max_body_size` from16k to64m; the Rust application still limits instruction JSON to16KiB. Updated [NixOS examples](../../agentix-agent/examples/README.md) preserve bearer and WireGuard behavior.

Optional non-secret assistant TOML:

```toml
[assistant]
db_path = "/var/lib/agentix-agent/personal"
index_path = "/var/lib/agentix-agent/personal-index"
recordings_path = "/var/lib/agentix-agent/recordings"
# Set this to your actual local Agentix gateway with a loaded Whisper model:
recordings_transcription_url = "http://127.0.0.1:11434"
```

Without the optional settings, audio goes to `db_path + ".recordings"` and transcription uses `gateway_url`. The Nix package supplies FFmpeg. Local Whisper must already be available; the worker does not download models or use a cloud fallback. It decodes30-second WAV chunks below the existing gateway/Whisper body limits. Failure retains original audio and retries after five minutes.

Back up personal Sled and the recordings directory in the same consistent snapshot. Sled owns metadata, server identity, full transcripts, retries and projection jobs. Audio files are content-addressed under `blobs/<prefix>/<sha256>.ogg`. LanceDB is a rebuildable personal transcript-search projection. No automatic recording deletion or garbage collection is implemented.

## Phone setup

1. Record a15–30second note on the NB100 and stop it. Disconnect the official Plaud app's Bluetooth connection.
2. Open Agentix overflow → Devices → Scan. Grant Nearby devices and scan again.
3. For the recorder, enter its existing32character Plaud binding token or choose **Use this Plaud with Agentix** and review the separate confirmation. Switching clears the previous app binding and recorder settings, preserves recordings and restarts the recorder. Only confirm if you want to move it to Agentix. After an uncertain/disconnected switch, try Sync before repeating the switch.
4. Wake the recorder, scan if needed and select **Sync recordings**. Keep Devices open and the recorder nearby. Leaving/backgrounding stops safely; retry resumes retained partials. Inbox provides local play/share/archive.
5. Configure and explicitly enable the existing HTTPS backend in Settings. Leave Owner token blank for the owner's tokenless WireGuard setup; otherwise supply the configured token.
6. On Devices enable **recording uploads**, then select **Sync to server**. This works on existing imports without an awake recorder. Once opted in, recorder sync also triggers upload.
7. Inbox separately shows phone-only, unresolved upload, stored server audio, transcription and pending search. Local files remain available offline. Lost responses retain the original file identity/hash/destination across restart. Restore that endpoint to retry after changing Settings; a new server cannot silently resolve an old uncertain upload.

Initial recorder support is conservative: protocol10, mono20ms Opus records, contiguous compressed-byte offsets and complete list payloads. Unsupported layouts fail visibly. Server upload bounds are64MiB/four hours; transcription output is bounded to2MiB. Recordings never automatically execute assistant tasks.

## Required physical and deployment acceptance — pending

- Record actual firmware and reported protocol; name/project code alone does not prove compatibility.
- Verify existing token or deliberate binding switch and authentication after device restart.
- Import real audio, compare duration/listen and verify the original remains on the recorder.
- Repeat after app restart: exactly one local/server recording per source identity.
- Interrupt a longer BLE transfer, then verify resume/full playback. Test Stop, navigation, background, process kill, permissions, Bluetooth off, device busy and disk failure.
- Deploy backend/nginx changes and test upload over the phone's WireGuard route. Interrupt the response and retry the same identity; verify original checksum and metadata on disk/Sled.
- Verify real local Whisper completion and transcript retrieval/search, restart recovery and projection rebuild on the owner's server.
- Confirm existing phone WAV recording, assistant/wake, reminders and archive/share survive the same-key update.

ADB reported no attached phone. Automated fixture/storage/API/search tests do not establish physical recorder compatibility, real speech quality or deployment readiness. See [verification.md](verification.md).
