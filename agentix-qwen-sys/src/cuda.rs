//! Bounded CUDA primitives built on the pinned GGML backend API.

use crate::{ffi, GgufMatrixRows, GgufRows, QuantizedRows, RowQuantization};
use std::{
    ffi::CStr,
    fmt,
    os::unix::fs::FileExt,
    ptr::{self, NonNull},
};

const MAX_TENSOR_PAYLOAD_BYTES: usize = 64 * 1024 * 1024;
const GRAPH_CONTEXT_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CudaDeviceInfo {
    pub name: String,
    pub description: String,
    pub free_memory_bytes: usize,
    pub total_memory_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CudaError {
    NoDevice,
    InvalidInput(&'static str),
    BackendUnavailable,
    AllocationFailed,
    ComputeFailed,
    Storage(String),
}

impl fmt::Display for CudaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoDevice => write!(f, "no GGML CUDA device is available"),
            Self::InvalidInput(reason) => write!(f, "invalid CUDA matvec input: {reason}"),
            Self::BackendUnavailable => write!(f, "cannot initialize the GGML CUDA backend"),
            Self::AllocationFailed => write!(f, "cannot allocate bounded GGML CUDA tensors"),
            Self::ComputeFailed => write!(f, "GGML CUDA matvec failed"),
            Self::Storage(error) => write!(f, "cannot read GGUF rows for CUDA matvec: {error}"),
        }
    }
}

impl std::error::Error for CudaError {}

/// Load registered GGML backends and report their CUDA devices and memory.
pub fn cuda_devices() -> Vec<CudaDeviceInfo> {
    // SAFETY: backend registration and device descriptors are process-global
    // GGML objects; this call initializes the pinned library's backend registry.
    unsafe { ffi::ggml_backend_load_all() };

    cuda_device_handles()
        .into_iter()
        .map(|device| {
            // SAFETY: GGML owns these NUL-terminated strings for the process
            // lifetime; null pointers are handled by `backend_string`.
            let name = unsafe { backend_string(ffi::ggml_backend_dev_name(device)) };
            // SAFETY: same lifetime and null-pointer invariant as the device name.
            let description = unsafe { backend_string(ffi::ggml_backend_dev_description(device)) };
            let (mut free_memory_bytes, mut total_memory_bytes) = (0, 0);
            // SAFETY: both output pointers refer to initialized writable usize values.
            unsafe {
                ffi::ggml_backend_dev_memory(
                    device,
                    &mut free_memory_bytes,
                    &mut total_memory_bytes,
                )
            };
            CudaDeviceInfo {
                name,
                description,
                free_memory_bytes,
                total_memory_bytes,
            }
        })
        .collect()
}

unsafe fn backend_string(value: *const std::ffi::c_char) -> String {
    if value.is_null() {
        return String::new();
    }
    // SAFETY: caller guarantees the pointer is a GGML-owned NUL-terminated string.
    unsafe { CStr::from_ptr(value) }
        .to_string_lossy()
        .into_owned()
}

/// Compute one bounded `weights * input` F32 matvec on the selected CUDA device.
///
/// `weights` is row-major with `rows` output rows and `input.len()` columns.
/// The 64 MiB limit bounds the copied weights and output; callers can stream a
/// larger matrix in output-row chunks and concatenate the results.
pub fn cuda_f32_matvec(
    device_index: usize,
    weights: &[f32],
    rows: usize,
    input: &[f32],
) -> Result<Vec<f32>, CudaError> {
    if rows == 0 || input.is_empty() {
        return Err(CudaError::InvalidInput("dimensions must be positive"));
    }
    let expected_weights = rows
        .checked_mul(input.len())
        .ok_or(CudaError::InvalidInput("weight element count overflow"))?;
    let weight_bytes = expected_weights
        .checked_mul(std::mem::size_of::<f32>())
        .ok_or(CudaError::InvalidInput("weight byte count overflow"))?;
    let input_bytes = input
        .len()
        .checked_mul(std::mem::size_of::<f32>())
        .ok_or(CudaError::InvalidInput("input byte count overflow"))?;
    let output_bytes = rows
        .checked_mul(std::mem::size_of::<f32>())
        .ok_or(CudaError::InvalidInput("output byte count overflow"))?;
    let tensor_payload_bytes = weight_bytes
        .checked_add(input_bytes)
        .and_then(|bytes| bytes.checked_add(output_bytes))
        .ok_or(CudaError::InvalidInput(
            "tensor payload byte count overflow",
        ))?;
    if tensor_payload_bytes > MAX_TENSOR_PAYLOAD_BYTES {
        return Err(CudaError::InvalidInput(
            "bounded tensor payload limit exceeded",
        ));
    }
    if weights.len() != expected_weights {
        return Err(CudaError::InvalidInput("weight matrix shape mismatch"));
    }
    if weights.iter().chain(input).any(|value| !value.is_finite()) {
        return Err(CudaError::InvalidInput("values must be finite"));
    }
    let backend = cuda_backend(device_index)?;
    ggml_matvec_on_backend(
        &backend,
        ffi::GGML_TYPE_F32,
        weights.as_ptr().cast(),
        weight_bytes,
        rows,
        input,
    )
}

/// Stream one validated quantized GGUF matrix through CUDA and return its matvec.
///
/// The matrix stays file-backed on the host. Quantized rows are read in output
/// chunks, uploaded in their original GGML encoding, and released after each
/// graph completes. The weight, input, and output tensor payload for each chunk
/// is capped at 64 MiB; backend scratch and driver allocations are additional.
pub fn cuda_quantized_gguf_matvec(
    device_index: usize,
    matrix: &GgufMatrixRows,
    input: &[f32],
) -> Result<Vec<f32>, CudaError> {
    if input.is_empty() || matrix.input_features != input.len() {
        return Err(CudaError::InvalidInput("matrix input width mismatch"));
    }
    if input.iter().any(|value| !value.is_finite()) {
        return Err(CudaError::InvalidInput("values must be finite"));
    }
    let GgufRows::Quantized(rows) = &matrix.rows else {
        return Err(CudaError::InvalidInput(
            "CUDA GGUF matvec requires a quantized matrix",
        ));
    };
    let ggml_type = rows.quantization.ggml_type();
    let input_bytes = input
        .len()
        .checked_mul(std::mem::size_of::<f32>())
        .ok_or(CudaError::InvalidInput("input byte count overflow"))?;
    let output_capacity = usize::try_from(matrix.output_features)
        .map_err(|_| CudaError::InvalidInput("output row count exceeds usize"))?;
    if output_capacity == 0 {
        return Err(CudaError::InvalidInput("matrix must have output rows"));
    }
    let matrix_end = matrix
        .first_row
        .checked_add(matrix.output_features)
        .ok_or(CudaError::InvalidInput("GGUF matrix row range overflow"))?;
    if matrix_end > rows.rows {
        return Err(CudaError::InvalidInput("GGUF matrix rows exceed tensor"));
    }
    let mut output = Vec::new();
    output
        .try_reserve_exact(output_capacity)
        .map_err(|_| CudaError::AllocationFailed)?;

    let row_payload_bytes = rows
        .row_bytes
        .checked_add(std::mem::size_of::<f32>())
        .ok_or(CudaError::InvalidInput("row payload byte count overflow"))?;
    let available_bytes =
        MAX_TENSOR_PAYLOAD_BYTES
            .checked_sub(input_bytes)
            .ok_or(CudaError::InvalidInput(
                "input exceeds bounded tensor payload",
            ))?;
    let chunk_rows = available_bytes / row_payload_bytes;
    if chunk_rows == 0 {
        return Err(CudaError::InvalidInput(
            "one quantized row exceeds bounded tensor payload",
        ));
    }

    let backend = cuda_backend(device_index)?;
    let mut completed_rows = 0_u64;
    while completed_rows < matrix.output_features {
        let remaining = matrix.output_features - completed_rows;
        let chunk_rows_u64 = u64::try_from(chunk_rows)
            .map_err(|_| CudaError::InvalidInput("chunk row count exceeds u64"))?;
        let count = usize::try_from(remaining.min(chunk_rows_u64))
            .map_err(|_| CudaError::InvalidInput("chunk row count exceeds usize"))?;
        let first_row = matrix
            .first_row
            .checked_add(completed_rows)
            .ok_or(CudaError::InvalidInput("GGUF row index overflow"))?;
        let bytes = read_quantized_rows(rows, first_row, count)?;
        let chunk_output = ggml_matvec_on_backend(
            &backend,
            ggml_type,
            bytes.as_ptr().cast(),
            bytes.len(),
            count,
            input,
        )?;
        output.extend_from_slice(&chunk_output);
        completed_rows = completed_rows
            .checked_add(count as u64)
            .ok_or(CudaError::InvalidInput("completed row count overflow"))?;
    }
    if output.iter().any(|value| !value.is_finite()) {
        return Err(CudaError::ComputeFailed);
    }
    Ok(output)
}

fn cuda_backend(device_index: usize) -> Result<Backend, CudaError> {
    // SAFETY: this initializes GGML's process-global backend registry.
    unsafe { ffi::ggml_backend_load_all() };
    let device = cuda_device_handles()
        .get(device_index)
        .copied()
        .ok_or(CudaError::NoDevice)?;
    // SAFETY: `device` is a live CUDA registry descriptor. A null parameter
    // pointer requests the backend's default device settings.
    NonNull::new(unsafe { ffi::ggml_backend_dev_init(device, ptr::null()) })
        .map(Backend)
        .ok_or(CudaError::BackendUnavailable)
}

fn ggml_matvec_on_backend(
    backend: &Backend,
    weight_type: ffi::ggml_type,
    weight_data: *const std::ffi::c_void,
    weight_bytes: usize,
    rows: usize,
    input: &[f32],
) -> Result<Vec<f32>, CudaError> {
    if rows == 0 || input.is_empty() {
        return Err(CudaError::InvalidInput("dimensions must be positive"));
    }
    let input_bytes = std::mem::size_of_val(input);
    let output_bytes = rows
        .checked_mul(std::mem::size_of::<f32>())
        .ok_or(CudaError::InvalidInput("output byte count overflow"))?;
    let tensor_payload_bytes = weight_bytes
        .checked_add(input_bytes)
        .and_then(|bytes| bytes.checked_add(output_bytes))
        .ok_or(CudaError::InvalidInput(
            "tensor payload byte count overflow",
        ))?;
    if tensor_payload_bytes > MAX_TENSOR_PAYLOAD_BYTES {
        return Err(CudaError::InvalidInput(
            "bounded tensor payload limit exceeded",
        ));
    }
    if input.iter().any(|value| !value.is_finite()) {
        return Err(CudaError::InvalidInput("values must be finite"));
    }
    let rows_i64 = i64::try_from(rows)
        .map_err(|_| CudaError::InvalidInput("row count exceeds GGML dimensions"))?;
    let columns_i64 = i64::try_from(input.len())
        .map_err(|_| CudaError::InvalidInput("input width exceeds GGML dimensions"))?;

    // SAFETY: a null buffer asks GGML to own a bounded context allocation, and
    // `no_alloc` lets the selected backend allocate tensor storage below.
    let context = NonNull::new(unsafe {
        ffi::ggml_init(ffi::ggml_init_params {
            mem_size: GRAPH_CONTEXT_BYTES,
            mem_buffer: ptr::null_mut(),
            no_alloc: true,
        })
    })
    .ok_or(CudaError::AllocationFailed)?;
    let context = Context(context);
    // SAFETY: context is live; dimensions match the contiguous payloads.
    let weight_tensor =
        unsafe { ffi::ggml_new_tensor_2d(context.0.as_ptr(), weight_type, columns_i64, rows_i64) };
    // SAFETY: context is live and the input width fits a GGML dimension.
    let input_tensor =
        unsafe { ffi::ggml_new_tensor_1d(context.0.as_ptr(), ffi::GGML_TYPE_F32, columns_i64) };
    if weight_tensor.is_null() || input_tensor.is_null() {
        return Err(CudaError::AllocationFailed);
    }
    // SAFETY: both inputs are valid tensors in the same live context.
    let output_tensor =
        unsafe { ffi::ggml_mul_mat(context.0.as_ptr(), weight_tensor, input_tensor) };
    if output_tensor.is_null() {
        return Err(CudaError::AllocationFailed);
    }
    // SAFETY: graph and output share a live context; this records the matvec.
    let graph = unsafe { ffi::ggml_new_graph(context.0.as_ptr()) };
    if graph.is_null() {
        return Err(CudaError::AllocationFailed);
    }
    // SAFETY: graph and output are non-null and owned by context.
    unsafe { ffi::ggml_build_forward_expand(graph, output_tensor) };

    // SAFETY: backend/context are live and tensor storage is bounded by the
    // checked payload sizes above.
    let buffer = NonNull::new(unsafe {
        ffi::ggml_backend_alloc_ctx_tensors(context.0.as_ptr(), backend.0.as_ptr())
    })
    .ok_or(CudaError::AllocationFailed)?;
    let buffer = BackendBuffer(buffer);

    // SAFETY: input and weight pointers reference exact contiguous tensor data;
    // GGML copies both slices before returning.
    unsafe {
        ffi::ggml_backend_tensor_set(weight_tensor, weight_data, 0, weight_bytes);
        ffi::ggml_backend_tensor_set(input_tensor, input.as_ptr().cast(), 0, input_bytes);
    }
    // SAFETY: graph and backend remain live; compute is synchronous.
    if unsafe { ffi::ggml_backend_graph_compute(backend.0.as_ptr(), graph) }
        != ffi::GGML_STATUS_SUCCESS
    {
        return Err(CudaError::ComputeFailed);
    }
    let mut output = Vec::new();
    output
        .try_reserve_exact(rows)
        .map_err(|_| CudaError::AllocationFailed)?;
    output.resize(rows, 0.0_f32);
    // SAFETY: compute completed and output has exactly the tensor's F32 size.
    unsafe {
        ffi::ggml_backend_tensor_get(output_tensor, output.as_mut_ptr().cast(), 0, output_bytes)
    };
    drop(buffer);
    if output.iter().any(|value| !value.is_finite()) {
        return Err(CudaError::ComputeFailed);
    }
    Ok(output)
}

fn read_quantized_rows(
    rows: &QuantizedRows,
    first_row: u64,
    count: usize,
) -> Result<Vec<u8>, CudaError> {
    let count_u64 =
        u64::try_from(count).map_err(|_| CudaError::InvalidInput("chunk row count exceeds u64"))?;
    let end_row = first_row
        .checked_add(count_u64)
        .ok_or(CudaError::InvalidInput("GGUF row range overflow"))?;
    if count == 0 || end_row > rows.rows {
        return Err(CudaError::InvalidInput("GGUF row range is outside matrix"));
    }
    let byte_count = rows
        .row_bytes
        .checked_mul(count)
        .ok_or(CudaError::InvalidInput("GGUF row byte count overflow"))?;
    let offset = u64::try_from(rows.row_bytes)
        .ok()
        .and_then(|row_bytes| first_row.checked_mul(row_bytes))
        .and_then(|relative| rows.data_offset.checked_add(relative))
        .ok_or(CudaError::InvalidInput("GGUF row offset overflow"))?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(byte_count)
        .map_err(|_| CudaError::AllocationFailed)?;
    bytes.resize(byte_count, 0);
    rows.file
        .read_exact_at(&mut bytes, offset)
        .map_err(|error| CudaError::Storage(error.to_string()))?;
    Ok(bytes)
}

impl RowQuantization {
    fn ggml_type(self) -> ffi::ggml_type {
        match self {
            Self::Q4_0 => ffi::GGML_TYPE_Q4_0,
            Self::Iq4Nl => ffi::GGML_TYPE_IQ4_NL,
            Self::Q8_0 => ffi::GGML_TYPE_Q8_0,
            Self::Native(ggml_type) => ggml_type as ffi::ggml_type,
        }
    }
}

fn cuda_device_handles() -> Vec<ffi::ggml_backend_dev_t> {
    // SAFETY: this is the pinned GGML CUDA backend registry name; the call
    // returns a process-global descriptor or null when CUDA is unavailable.
    let registry = unsafe { ffi::ggml_backend_reg_by_name(c"CUDA".as_ptr()) };
    if registry.is_null() {
        return Vec::new();
    }
    let count = unsafe { ffi::ggml_backend_reg_dev_count(registry) };
    (0..count)
        .filter_map(|index| {
            // SAFETY: index is below this registry's count, and returned
            // device descriptors have process-global registry lifetime.
            let device = unsafe { ffi::ggml_backend_reg_dev_get(registry, index) };
            (!device.is_null()
                // SAFETY: `device` is non-null and owned by GGML's registry.
                && unsafe { ffi::ggml_backend_dev_type(device) }
                    == ffi::GGML_BACKEND_DEVICE_TYPE_GPU)
                .then_some(device)
        })
        .collect()
}

struct Backend(NonNull<ffi::ggml_backend>);

impl Drop for Backend {
    fn drop(&mut self) {
        // SAFETY: this wrapper uniquely owns the backend returned by dev_init.
        unsafe { ffi::ggml_backend_free(self.0.as_ptr()) };
    }
}

struct Context(NonNull<ffi::ggml_context>);

impl Drop for Context {
    fn drop(&mut self) {
        // SAFETY: this wrapper uniquely owns the context returned by ggml_init.
        unsafe { ffi::ggml_free(self.0.as_ptr()) };
    }
}

struct BackendBuffer(NonNull<ffi::ggml_backend_buffer>);

impl Drop for BackendBuffer {
    fn drop(&mut self) {
        // SAFETY: this wrapper uniquely owns the buffer returned by GGML.
        unsafe { ffi::ggml_backend_buffer_free(self.0.as_ptr()) };
    }
}

#[cfg(test)]
mod tests {
    use super::{
        cuda_devices, cuda_f32_matvec, read_quantized_rows, CudaError, QuantizedRows,
        RowQuantization,
    };
    use std::{
        fs::{self, File},
        sync::Arc,
        time::{SystemTime, UNIX_EPOCH},
    };

    #[test]
    #[ignore = "requires a CUDA runtime and GPU; run explicitly on that host"]
    fn cuda_matvec_matches_cpu_reference_when_device_is_available(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let devices = cuda_devices();
        let device = devices.first().ok_or(CudaError::NoDevice)?;
        assert!(device.total_memory_bytes > 0);
        let weights = [1.0_f32, 2.0, 3.0, -1.0, 0.5, 4.0];
        let input = [0.25_f32, 2.0, -0.5];
        let actual = cuda_f32_matvec(0, &weights, 2, &input)?;
        let expected = [2.75_f32, -1.25];
        for (actual, expected) in actual.iter().zip(expected) {
            assert!((actual - expected).abs() < 1.0e-5);
        }
        Ok(())
    }

    #[test]
    fn cuda_matvec_rejects_unbounded_or_malformed_inputs_before_backend_use() {
        assert_eq!(
            cuda_f32_matvec(0, &[1.0], 2, &[1.0]),
            Err(CudaError::InvalidInput("weight matrix shape mismatch"))
        );
        assert_eq!(
            cuda_f32_matvec(0, &[1.0], 1, &[f32::NAN]),
            Err(CudaError::InvalidInput("values must be finite"))
        );
        assert_eq!(
            cuda_f32_matvec(0, &[], 1_000, &[1.0; 20_000]),
            Err(CudaError::InvalidInput(
                "bounded tensor payload limit exceeded"
            ))
        );
    }

    #[test]
    fn raw_quantized_row_reads_preserve_exact_gguf_bytes_and_bounds(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let path = std::env::temp_dir().join(format!(
            "agentix-cuda-raw-rows-{}-{nonce}.gguf",
            std::process::id()
        ));
        let mut file_bytes = vec![0xFE, 0xED];
        file_bytes.extend(0_u8..15);
        fs::write(&path, &file_bytes)?;
        let file = Arc::new(File::open(&path)?);
        let rows = QuantizedRows {
            file,
            data_offset: 2,
            rows: 3,
            rows_per_group: 3,
            groups: 1,
            row_width: 32,
            row_bytes: 5,
            quantization: RowQuantization::Q4_0,
        };

        assert_eq!(
            read_quantized_rows(&rows, 1, 2)?,
            (5_u8..15).collect::<Vec<_>>()
        );
        assert!(matches!(
            read_quantized_rows(&rows, 2, 2),
            Err(CudaError::InvalidInput("GGUF row range is outside matrix"))
        ));
        drop(rows);
        fs::remove_file(path)?;
        Ok(())
    }
}
