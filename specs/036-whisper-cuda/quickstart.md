# Deployment

Build or deploy from this branch (based on master bc91122), using the same package selection as the current service:

```nix
services.agentix.whisper.package = agentix.packages.${pkgs.system}.agentix-whisper;
```

The stock flake already configures CUDA capabilities 8.6 and 12.0. For a custom package-set evaluation, configure capabilities for the target GPU, as for llama. Whisper now enables CUDA under the same condition. The host needs its existing NVIDIA driver/device access; the packaged worker supplies the driver library search path. No new service flags, model files, ports, or shared GPU coordinator are required. Preserve existing preload settings to retain low transcription latency.

For an intentionally CPU-only deployment, select `agentix-whisper-cpu` explicitly, or evaluate the package with empty CUDA capabilities. The CPU variant also supplies the sandboxed socket-startup check, which cannot depend on a host GPU driver.

```sh
nix build .#agentix-whisper .#agentix-whisper-cpu
nix build .#checks.x86_64-linux.whisper-runtime .#checks.x86_64-linux.workspace-locks
nix build .#agentix-test-whisper-model
```

After deployment, verify the model-load log selects CUDA and allocates CUDA model/compute buffers. A successful HTTP response alone does not prove GPU use. Recalibrate Whisper VRAM before enabling feature 035 admission plans: its previous CPU footprint is not a valid GPU budget.

The worker now joins its native thread during cleanup: closing its work channel comes first, and Tokio waits for native state/context destruction before normal runtime exit. Final local acceptance passed with clean CUDA shutdown logs; see validation.md. Recognition accuracy and production-model performance are outside this tiny-fixture check.
