# Implementation Plan: Hey Agentix

## Technical context

Native Kotlin/Compose app, SDK35/min26, Android-free JVM core. Add local sherpa-onnx 2.0.1 AAR and the Apache-2.0 GigaSpeech 3.3M 2024-01-01 keyword model using Nix fixed-output fetches. Bundle int8 encoder/joiner, fp32 decoder and BPE tokens; repository keyword file spells Hey Agentix in valid model tokens. No new Maven dependencies or Rust crate. Android microphone FGS was already declared for notes; new service is separate and non-exported.

## Constitution check

Native Android integration follows accepted024 boundary, core policy remains independently testable. Local-first/no cloud/key; no library listeners, backend routing or jail changes. Nix is canonical and dependencies pinned. Scope-specific JVM tests/lint/APK build replace unrelated Cargo recompilation because no Rust changes. No weakening of constitution. Battery/DSP limitations and hardware acceptance documented.

## Design

- `core/.../assistant/WakeWordGate.kt`: Android-free hold/quiet callback policy plus generation/debounce admission, exercised under Stop/pause/late-callback cases.
- `app/.../wake/`: explicit setup and preference/runtime separation, microphone FGS, local keyword adapter and runtime coordination. Main-thread gate manages asynchronous holds; worker alone owns/release recorder/native objects. Callback after worker quiet admits competing recognition/recording.
- `AgentixVoiceService`: use selected-service instance to show a protected assistant session after wake capture releases; reject unavailable/removed service. Session carries wake-origin flag. No arbitrary external activity entry.
- Command recognition, backend processing and assistant TTS hold the listener; it rearms when idle even if the assistant screen stays open. TTS waits for wake capture to release before playback, and releases its hold on completion/error/Stop. Unlocked invocation uses existing recognition. Locked wake displays unlock action before Talk; no private content/voice commands while locked.
- Recognition and note recording hold gate until resources released. Stop invalidates pending detections; session launch bounded/debounced. Note recording FGS promotes before asynchronous wait.
- Nix fetches AAR/model independently; Gradle receives paths through properties for hermetic and interactive builds. No secrets in store.

## Validation

Unit tests for gate hold ordering, cancellation, multiple users and stale/debounced wake signals. Nix Android tests/lint/assembly, packaged asset/native ABI inspection and same-key downstream signing. Physical detection accuracy, lockscreen UI/background launches, actual microphone handoff and battery measured separately. No install/permission/default-role change without owner instruction.
