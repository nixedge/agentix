# Wake Word Models

Wake word files (`.rpw`) are created with `rustpotter-cli` from WAV recordings.

## Creating a Custom "Hey Agentix" Wake Word

1. Install rustpotter-cli:
   ```bash
   nix develop --command cargo install rustpotter-cli
   ```

2. Record 3–8 short WAV clips (16kHz, mono, ~1.5s each) of yourself saying "Hey Agentix":
   ```bash
   arecord -d 2 -f S16_LE -r 16000 -c 1 hey_agentix_1.wav
   arecord -d 2 -f S16_LE -r 16000 -c 1 hey_agentix_2.wav
   arecord -d 2 -f S16_LE -r 16000 -c 1 hey_agentix_3.wav
   ```

3. Build the wake word file:
   ```bash
   rustpotter-cli create-wakeword \
     --name hey_agentix \
     hey_agentix_1.wav hey_agentix_2.wav hey_agentix_3.wav \
     -o agentix-voice/wakewords/hey_agentix.rpw
   ```

4. Set `wake_word.model_path = "wakewords/hey_agentix.rpw"` in your config TOML.

## Development / Testing

For initial development without recording sessions, lower the `threshold` in config to 0.3
and experiment with any available rustpotter demo models. The model path is relative to the
working directory where `agentix-voice` is run.

## Wake Word File Naming

- `hey_agentix.rpw` — production model (create from your own recordings)
- Do not commit large WAV source recordings; only commit the resulting `.rpw` file
