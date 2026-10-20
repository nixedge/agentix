# Iviron CUDA check — user-provided result

The user supplied this output after the hybrid CUDA implementation checkpoint
`467086d`. The exact tested revision and invocation were not included in the
transcript. Host: `iviron`; this is a separate machine from the original
RTX 3090 target. CPU/RAM similarity is user-reported, without an exact inventory.

```text
running 3 tests
test cuda::tests::cuda_matvec_matches_cpu_reference_when_device_is_available ... ggml_cuda_init: found 1 CUDA devices (Total VRAM: 23983 MiB):
  Device 0: NVIDIA GeForce RTX 5090 Laptop GPU, compute capability 12.0, VMM: yes, VRAM: 23983 MiB
ok
test tests::cuda_matrix_dispatch_preserves_nonzero_stateful_decoder_and_prefill ... ok
test tests::cuda_matrix_storage_types_match_decoded_reference ... ok

test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 35 filtered out; finished in 0.44s
```

This establishes execution of the analytical matvec, supported-storage-type
checks and compact nonzero stateful decoder/prefill parity on this CUDA device.
The reported 0.44 seconds is the synthetic test-suite duration. It is not a
production-model prefill/decode benchmark. No full-model response, memory-budget
measurement, driver version, original-checkpoint parity or RTX 3090 execution
result was supplied.
