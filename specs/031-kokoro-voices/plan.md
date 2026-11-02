# Implementation plan: Offline Kokoro voices

## Technical context and decision

Kotlin/Compose Android SDK35/min26, existing wake/recognition and Android TTS. Reuse pure policy in Android-free core. Add pinned MediaPipe tasks-audio1.2.0 and the CPU-compatible `kokoro_int8_fp16.litertlm` bundle from an immutable Hugging Face revision (Kokoro82M, all stock voice assets/full text frontend; expose George/Daniel only). Native API inspected from the exact AAR and official source/docs. CPU avoids silently degrading the GPU-only model on unsupported accelerators. Larger APK/private model cache accepted by owner; no server changes.

Existing Rust agentix-tts supports both IDs on the server, but owner explicitly selected phone execution; do not modify or deploy that path. Current sherpa2.0.1 Kokoro frontend lacks unknown-word G2P fallback, so do not reuse its lexicon-only shortcut for natural arbitrary replies.

## Constitution check

Native Android scope follows024 and030; no Rust crate/listener/router/sandbox changes. Local-first inference and all dependencies pinned by Nix+Gradle snapshot. Scoped JVM tests/lint/native synthesis and APK build; unrelated Cargo/model rebuilds unnecessary. No constitution exception beyond established Android native integration.

## Design

- Core SpeechOutput enum maps stable preferences to Android, bm_george and bm_daniel; Unicode-safe bounded speech chunking and generation policy tested.
- AssistantSpeaker facade preserves OfflineSpeaker Android state and delegates chosen Kokoro utterances to a lazy local worker. Provider choice stored separately; switch cancels both. Selection/preview state reports unsupported ABI honestly.
- Kokoro worker serializes model install/init/synthesis/playback/close off main. Model copied atomically into backup-excluded private versioned path, matching original filename required by runtime. Fixed model identity/size/hash verified on installation. Never write input text or generated audio to disk.
- Worker cancellation token plus native lock-free cancel rejects late initialization/synthesis results; bounded nonblocking AudioTrack playback and focus cleanup. Hold wake before execution until worker quiet; a cancelled pending hold cannot start work later. Native engine closed on worker after in-flight use finishes.
- Voice UI shows Android / Kokoro George / Kokoro Daniel, Preview and clear cold-start/error/support status. Existing Android profiles/voice picker only for Android provider, preventing misleading Kokoro pitch/language controls.
- Pin model fetch with Nix, expose Gradle model path and include assets/model identity manifest. Update dependency recording with project Nix script; preserve all app journals/configuration.

## Validation

Core preference/ID/chunking/cancellation policy tests, canonical Nix build/lint, packaged asset/native/16KiB and same-key signing. Real model/native sample generation for each ID where supported locally; Pixel offline playback/cancellation/audio focus/memory/latency and perceived voice quality separate physical acceptance. No phone install/role/permission change or push as incidental work.
