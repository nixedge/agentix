# agentix-android

Native Kotlin/Compose client for the broader Agentix assistant. Plaud is one optional device integration, not the app's identity.

- Follow the repository constitution and use the `speckit` skill for feature work. Foundation artifacts: `../specs/024-agentix-android/`.
- Use `nix develop .#android --command ...` for Android tools. No host SDK, global Gradle/JDK install, or SDK Manager downloads.
- Canonical APK build/check: `nix build .#agentix-android`. Core tests and Android lint run in the derivation.
- `:core` must remain Android-free. Keep BLE, permissions, lifecycle, persistence, and network adapters in `:app`.
- Preserve recorder data and binding. The foundation only scans; do not add reset/unbind/delete/firmware commands as incidental debugging.
- Never infer NB-100 compatibility from its name or project code alone; record protocol version and hardware evidence.
- No provider keys, server tokens, recordings, or release keystores in Git or Nix outputs. A debug signing identity is public development material only.
- Keep backend contracts explicit. Configuration alone must never be shown as a successful connection or synchronization.
- WireGuard is managed externally by the owner's phone; do not implement a VPN service.
- Keep physical-device acceptance distinct from JVM fixture tests and lint.
- Phone audio uses app-private WAV files. Preserve partial recordings for recovery; never route audio through a UI message bridge or auto-upload without a defined backend contract.
- Note recording starts only from an explicit action in a visible activity and uses its microphone foreground service for continued capture. The default-assistant system invocation may start a single on-device recognition turn in the resumed, focused, unlocked assistant activity after prior microphone permission; other assistant entry waits for Talk. Stop assistant capture/TTS on focus loss or pause. Do not add a hotword listener or claim Gemini voice invocation works without a device test.
- Assistant recognition uses the explicit on-device SpeechRecognizer, never an implicit cloud fallback. Reject stale callbacks and arbitrate the microphone with note capture. Recognition drafts stay ephemeral; the explicitly enabled v1 backend persists a pending instruction envelope for safe retry, then retains the latest reply in private backup-excluded preferences. Disabled backend reports not connected. Preserve request identity across uncertain results; stopping the wait does not undo server changes.
