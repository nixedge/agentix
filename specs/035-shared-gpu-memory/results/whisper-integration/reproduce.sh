#!/usr/bin/env bash
set -euo pipefail
results=/home/sam/home/agentix/035-shared-gpu-memory/specs/035-shared-gpu-memory/results/whisper-integration
packages="$results/packages.json"
coordinator=$(jq -r .coordinator "$packages")
qwen=$(jq -r .qwen "$packages")
llama=$(jq -r .llama "$packages")
whisper=$(jq -r .whisper "$packages")
run_dir=$(mktemp -d /tmp/035-qwen-handoff.XXXXXX)
export LD_LIBRARY_PATH=/run/opengl-driver/lib
real_smi=/nix/store/n5rndkrncrwds1p9cc4vpnqp1c6my4fk-nvidia-x11-595.71.05-bin/bin/nvidia-smi
export AGENTIX_GPU_COORDINATOR="$run_dir/gpu.sock"
export AGENTIX_GPU_WORKERS=qwen,llama,whisper
export AGENTIX_GPU_METRICS_ADDRESS=127.0.0.1:19488
export AGENTIX_GPU_DEVICE=0 AGENTIX_GPU_SAFETY_BYTES=536870912
export AGENTIX_MODELS_DIR="$run_dir/models"
export AGENTIX_CPU_THREADS=2 RAYON_NUM_THREADS=2 OMP_NUM_THREADS=2
export RUST_LOG=info
export GPU_SMI_TRACE="$results/smi-observations.log"
mkdir -p "$run_dir/bin"
cat > "$run_dir/bin/nvidia-smi" <<'SMI'
#!/usr/bin/env bash
set -euo pipefail
value=$(/nix/store/n5rndkrncrwds1p9cc4vpnqp1c6my4fk-nvidia-x11-595.71.05-bin/bin/nvidia-smi "$@")
printf '%s|%s|%s\n' "$(date -u +%FT%T.%3NZ)" "$*" "$value" >> "$GPU_SMI_TRACE"
printf '%s\n' "$value"
SMI
chmod +x "$run_dir/bin/nvidia-smi"
export PATH="$run_dir/bin:$PATH"
: > "$GPU_SMI_TRACE"
initial_temp=$(timeout 10 "$real_smi" --query-gpu=temperature.gpu --format=csv,noheader,nounits)
((initial_temp < 65)) || { echo 'GPU too warm before test'; exit 1; }
pids=()
cleanup() {
  trap - EXIT TERM INT
  for pid in "${pids[@]}"; do kill "$pid" 2>/dev/null || true; done
  for pid in "${pids[@]}"; do wait "$pid" 2>/dev/null || true; done
}
trap cleanup EXIT
trap 'exit 124' TERM INT
shell_pid=$$
(
  while kill -0 "$shell_pid" 2>/dev/null; do
    value=$(timeout 3 "$real_smi" --query-gpu=temperature.gpu,utilization.gpu,memory.used,power.draw --format=csv,noheader,nounits) || { kill -TERM "$shell_pid"; exit 1; }
    mem_available=0
    while read -r key amount unit; do if [[ "$key" == MemAvailable: ]]; then mem_available=$amount; break; fi; done < /proc/meminfo
    printf '%s,%s,%s\n' "$(date -u +%FT%T.%3NZ)" "$value" "$mem_available" >> "$results/handoff-resources.csv"
    temp=${value%%,*}
    if ((temp >= 65 || mem_available < 8388608)); then echo "Thermal/RAM cutoff: $temp C, $mem_available KiB" > "$results/handoff-cutoff.txt"; kill -TERM "$shell_pid"; exit 1; fi
    sleep 0.25
  done
) &
pids+=("$!")
printf 'timestamp,temperature_C,utilization_percent,memory_MiB,power_W,mem_available_KiB\n' > "$results/handoff-resources.csv"
"$coordinator/bin/agentix-gpu-coordinator" > "$results/handoff-coordinator.log" 2>&1 &
pids+=("$!")
for attempt in $(seq 1 100); do [[ -S "$AGENTIX_GPU_COORDINATOR" ]] && break; sleep 0.05; done
export AGENTIX_WHISPER_SOCKET="$run_dir/whisper.sock" AGENTIX_WHISPER_MODELS=
export AGENTIX_LLAMA_SOCKET="$run_dir/llama.sock" AGENTIX_LLAMA_MODELS=
export AGENTIX_MAX_CTX=256 AGENTIX_GPU_LAYERS=-1 AGENTIX_MAX_LOADED_MODELS=1
whisper_model=/nix/store/pbhdzdlc0y9n0bn3yysy7zhp0w7pkw6b-ggml-tiny.en.bin
jina_model=/nix/store/98an1mnnplfcm0yx071jpwngmk5mwx8n-jina-code-embeddings-1.5b-Q8_0.gguf
whisper_name=${whisper_model##*/}
jina_name=${jina_model##*/}
whisper_hash=$(sha256sum "$whisper_model"); whisper_hash=${whisper_hash%% *}
whisper_plan=$(jq -cn --arg name "$whisper_name" --arg hash "$whisper_hash" '{($name):{peak_bytes:1610612736,model_sha256:[$hash],priority:100}}')
AGENTIX_GPU_ROLE=whisper AGENTIX_GPU_PLANS="$whisper_plan" "$whisper/bin/agentix-whisper" > "$results/handoff-whisper.log" 2>&1 &
whisper_pid=$!; pids+=("$whisper_pid")
AGENTIX_GPU_ROLE=llama AGENTIX_GPU_PLANS='{}' "$llama/bin/agentix-llama" > "$results/handoff-llama-register.log" 2>&1 &
llama_pid=$!; pids+=("$llama_pid")
wait_socket() {
  local socket=$1 endpoint=$2
  for attempt in $(seq 1 2000); do
    if curl --silent --fail --max-time 1 --unix-socket "$socket" "http://localhost$endpoint" > /dev/null; then return 0; fi
    sleep 0.05
  done
  return 1
}
wait_socket "$AGENTIX_WHISPER_SOCKET" /v1/models
wait_socket "$AGENTIX_LLAMA_SOCKET" /v1/models
export AGENTIX_QWEN_GGUF_SHARDS=/home/sam/home/agentix/033-rustingstrata-qwen/target/gpu-model.yOlcXd/model-00001-of-00002.gguf:/home/sam/home/agentix/033-rustingstrata-qwen/target/gpu-model.yOlcXd/model-00002-of-00002.gguf
export AGENTIX_QWEN_EXPECTED_SHARD_SHA256=4c1eb2ceb4915e1192f4f386021897bde56a97f40a0bb78bb86465e0f7d2aca3:316b46f3a2dbd68c900f43136ab9449f9dcc3725dfd8c794847c204bc161e113
export AGENTIX_QWEN_TOKENIZER=/nix/store/61sk17pdm2q2ycs3iy73cr0ccizcdk0w-qwen-flash-next-tokenizer.json
export AGENTIX_QWEN_CHAT_TEMPLATE=/nix/store/sm1512hkgnf0w87g3sfw554fg0rmsk78-qwen-flash-next-chat-template.jinja
export AGENTIX_QWEN_SOCKET="$run_dir/qwen.sock" AGENTIX_QWEN_GPU_SOCKET="$run_dir/qwen-gpu.sock"
export AGENTIX_QWEN_CONTEXT=128 AGENTIX_QWEN_PREFILL_CHUNK=64 AGENTIX_QWEN_EXPERT_MAX_BYTES=536870912
export AGENTIX_QWEN_COMPUTE=cuda STRATA_FIXED_EXPERTS=1
qwen_plan=$(jq -cn --arg hashes "$AGENTIX_QWEN_EXPECTED_SHARD_SHA256" '{"qwen3.8-flash-next":{peak_bytes:6442450944,model_sha256:($hashes|split(":")),context_tokens:128,batch_tokens:64,priority:50}}')
AGENTIX_GPU_ROLE=qwen AGENTIX_GPU_PLANS="$qwen_plan" "$qwen/bin/agentix-qwen-worker" > "$results/handoff-qwen.log" 2>&1 &
qwen_pid=$!; pids+=("$qwen_pid")
printf 'qwen_pid=%s\n' "$qwen_pid"
wait_socket "$AGENTIX_QWEN_SOCKET" /health
qwen_request='{"model":"qwen3.8-flash-next","messages":[{"role":"user","content":"Say OK."}],"max_tokens":1,"temperature":0}'
curl --silent --show-error --fail-with-body --max-time 30 --unix-socket "$AGENTIX_QWEN_SOCKET" -H 'Content-Type: application/json' --data-binary "$qwen_request" http://localhost/v1/chat/completions > "$results/qwen-before.json"
curl --silent --show-error --fail --max-time 5 http://127.0.0.1:19488/metrics > "$results/handoff-before.txt"
metric() { rg "^$1\\{worker=\"qwen\"\\} " "$results/handoff-before.txt" | cut -d ' ' -f 2; }
reserved=$(metric agentix_gpu_reserved_bytes)
floor=$(metric agentix_gpu_resident_floor_bytes)
resident=$(metric agentix_gpu_resident_bytes || true); resident=${resident:-0}
cache=$(metric agentix_gpu_reclaimable_bytes)
((cache > 0)) || { echo 'Qwen has no populated cache to reclaim'; exit 1; }
free=$(rg '^agentix_gpu_device_free_bytes\{device="0"\} ' "$results/handoff-before.txt" | cut -d ' ' -f 2)
credit=$((resident < floor ? resident : floor))
room=$((free - 536870912 - reserved + credit))
((room >= 3221225472)) || { echo 'Insufficient safe test headroom'; exit 1; }
# Inflate only the fixture's admission ceiling to force the real reclaim path;
# this is not a production profile and does not allocate the ceiling on GPU.
jina_peak=$((room + 134217728))
printf 'qwen_cache_bytes=%s\nadmission_headroom_bytes=%s\njina_test_ceiling_bytes=%s\n' "$cache" "$room" "$jina_peak"
curl --silent --show-error --fail --max-time 3 --unix-socket "$AGENTIX_LLAMA_SOCKET" -X POST http://localhost/control/shutdown
wait "$llama_pid"
jina_plan=$(jq -cn --arg name "$jina_name" --argjson bytes "$jina_peak" '{($name):{peak_bytes:$bytes,model_sha256:["3a09a8817b852b5a4faaa6ebb1a5590322746d2b570b578d0b7e3b6e849062aa"],context_tokens:8192,batch_tokens:8192,priority:100}}')
AGENTIX_GPU_ROLE=llama AGENTIX_GPU_PLANS="$jina_plan" "$llama/bin/agentix-llama" > "$results/handoff-llama.log" 2>&1 &
llama_pid=$!; pids+=("$llama_pid")
wait_socket "$AGENTIX_LLAMA_SOCKET" /v1/models
jina_pull=$(jq -cn --arg name "$jina_model" '{name:$name}')
curl --silent --show-error --fail --max-time 8 --unix-socket "$AGENTIX_LLAMA_SOCKET" -H 'Content-Type: application/json' --data-binary "$jina_pull" http://localhost/api/pull
printf 'BEGIN_JINA\n' >> "$GPU_SMI_TRACE"
jina_request=$(jq -cn --arg model "$jina_name" '{model:$model,input:"def add(a, b): return a + b"}')
curl --silent --show-error --fail-with-body --max-time 30 --unix-socket "$AGENTIX_LLAMA_SOCKET" -H 'Content-Type: application/json' --data-binary "$jina_request" http://localhost/v1/embeddings > "$results/jina-embedding.json"
printf 'END_JINA\n' >> "$GPU_SMI_TRACE"
jq -e '.data[0].embedding | length == 1536' "$results/jina-embedding.json" > /dev/null
curl --silent --show-error --fail --max-time 5 http://127.0.0.1:19488/metrics > "$results/handoff-after-jina.txt"
rg '^agentix_gpu_cache_reserved_bytes\{worker="qwen"\} 0$' "$results/handoff-after-jina.txt"
rg '^agentix_gpu_reclaimed_bytes_total [1-9][0-9]*$' "$results/handoff-after-jina.txt"
# Release the deliberately inflated test lease before the independent Whisper
# case. This is a handoff test, not calibration of simultaneous resident peaks.
curl --silent --show-error --fail --max-time 3 --unix-socket "$AGENTIX_LLAMA_SOCKET" -X POST http://localhost/control/shutdown
wait "$llama_pid"
whisper_pull=$(jq -cn --arg name "$whisper_model" '{name:$name}')
curl --silent --show-error --fail --max-time 5 --unix-socket "$AGENTIX_WHISPER_SOCKET" -H 'Content-Type: application/json' --data-binary "$whisper_pull" http://localhost/api/pull
curl --silent --show-error --fail --max-time 10 --unix-socket "$AGENTIX_WHISPER_SOCKET" -F file=@/tmp/036-whisper.wav -F "model=$whisper_name" http://localhost/v1/audio/transcriptions > "$results/handoff-transcript.json"
curl --silent --show-error --fail --max-time 5 http://127.0.0.1:19488/metrics > "$results/handoff-after-whisper.txt"
kill -0 "$qwen_pid"
curl --silent --show-error --fail-with-body --max-time 30 --unix-socket "$AGENTIX_QWEN_SOCKET" -H 'Content-Type: application/json' --data-binary "$qwen_request" http://localhost/v1/chat/completions > "$results/qwen-after.json"
jq -e '.choices | length == 1' "$results/qwen-after.json" > /dev/null
curl --silent --show-error --fail --max-time 3 --unix-socket "$AGENTIX_WHISPER_SOCKET" -X POST http://localhost/control/shutdown
wait "$whisper_pid"
if rg -n -i 'CUDA error|ggml_abort|panicked' "$results/handoff-qwen.log" "$results/handoff-llama.log" "$results/handoff-whisper.log"; then exit 1; fi
rg 'CUDA0.*(size|buffer)|using CUDA' "$results/handoff-whisper.log"
# llama suppresses native info logs; require CUDA-enabled all-layer loading and
# driver-observed resident memory rather than a suppressed native log message.
rg 'n_gpu_layers=4294967295.*cuda=true' "$results/handoff-llama.log"
rg '^agentix_gpu_resident_bytes\{worker="llama"\} [1-9][0-9]*$' "$results/handoff-after-jina.txt"
echo 'PASS: Qwen cache reclaimed automatically; Jina embedding and CUDA Whisper succeeded; same Qwen PID answered afterward.'
