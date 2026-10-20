//! Persistent native matrix execution. Original GGUF encodings stay on device;
//! only activations cross the boundary on cache hits.

use crate::{ffi, DenseEncoding, Error, GgufMatrixRows, GgufRows};
use std::{
    collections::HashMap,
    fs::File,
    os::unix::fs::FileExt,
    ptr::{self, NonNull},
    sync::Arc,
};

const UPLOAD_CHUNK_BYTES: usize = 16 * 1024 * 1024;
const MAX_ACTIVATION_BYTES: usize = 64 * 1024 * 1024;
pub const CUDA_RESERVE_BYTES: usize = 2 * 1024 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ComputeStats {
    pub capacity_bytes: usize,
    pub retained_bytes: usize,
    pub peak_retained_bytes: usize,
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
    pub weight_upload_bytes: u64,
    pub activation_upload_bytes: u64,
    pub output_download_bytes: u64,
    pub matvecs: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct MatrixKey {
    file: usize,
    offset: u64,
    kind: ffi::ggml_type,
    columns: usize,
    rows: usize,
}

struct MatrixSource {
    file: Arc<File>,
    key: MatrixKey,
    bytes: usize,
}

impl MatrixSource {
    fn from_matrix(matrix: &GgufMatrixRows) -> Result<Self, Error> {
        let (file, base, row_bytes, row_count, kind) = match &matrix.rows {
            GgufRows::Quantized(rows) => (
                &rows.file,
                rows.data_offset,
                rows.row_bytes,
                rows.rows,
                rows.quantization.ggml_type(),
            ),
            GgufRows::Dense(rows) => (
                &rows.file,
                rows.data_offset,
                rows.row_bytes,
                rows.rows,
                match rows.encoding {
                    DenseEncoding::F32 => ffi::GGML_TYPE_F32,
                    DenseEncoding::F16 => ffi::GGML_TYPE_F16,
                    DenseEncoding::Bf16 => ffi::GGML_TYPE_BF16,
                },
            ),
        };
        let rows = usize::try_from(matrix.output_features)
            .map_err(|_| Error::Invalid("native matrix row count overflow"))?;
        if rows == 0
            || matrix.input_features == 0
            || matrix
                .first_row
                .checked_add(matrix.output_features)
                .is_none_or(|end| end > row_count)
        {
            return Err(Error::Invalid("native matrix view outside tensor"));
        }
        let offset = matrix
            .first_row
            .checked_mul(u64::try_from(row_bytes).map_err(|_| Error::Invalid("row size overflow"))?)
            .and_then(|relative| base.checked_add(relative))
            .ok_or(Error::Invalid("native matrix file offset overflow"))?;
        let bytes = rows
            .checked_mul(row_bytes)
            .ok_or(Error::Invalid("native matrix byte count overflow"))?;
        Ok(Self {
            file: Arc::clone(file),
            key: MatrixKey {
                file: Arc::as_ptr(file) as usize,
                offset,
                kind,
                columns: matrix.input_features,
                rows,
            },
            bytes,
        })
    }
}

struct Context(NonNull<ffi::ggml_context>);
impl Context {
    fn new(bytes: usize) -> Result<Self, Error> {
        // SAFETY: GGML owns a bounded metadata allocation; tensors are allocated separately.
        NonNull::new(unsafe {
            ffi::ggml_init(ffi::ggml_init_params {
                mem_size: bytes,
                mem_buffer: ptr::null_mut(),
                no_alloc: true,
            })
        })
        .map(Self)
        .ok_or(Error::Invalid("cannot allocate native graph metadata"))
    }
}
impl Drop for Context {
    fn drop(&mut self) {
        // SAFETY: the uniquely owned context outlives all its tensor operations.
        unsafe { ffi::ggml_free(self.0.as_ptr()) };
    }
}
struct Buffer(NonNull<ffi::ggml_backend_buffer>);
impl Drop for Buffer {
    fn drop(&mut self) {
        // SAFETY: all compute/copy operations complete before this uniquely owned buffer is freed.
        unsafe { ffi::ggml_backend_buffer_free(self.0.as_ptr()) };
    }
}
struct Backend(NonNull<ffi::ggml_backend>);
impl Drop for Backend {
    fn drop(&mut self) {
        // SAFETY: resident buffers are declared before the backend and dropped first.
        unsafe { ffi::ggml_backend_free(self.0.as_ptr()) };
    }
}

struct ResidentMatrix {
    _buffer: Buffer,
    _context: Context,
    // Holding the source prevents an Arc address in MatrixKey being reused by another file.
    _source: Arc<File>,
    tensor: NonNull<ffi::ggml_tensor>,
    allocated_bytes: usize,
    used: u64,
}

pub(crate) struct NativeCompute {
    matrices: HashMap<MatrixKey, ResidentMatrix>,
    backend: Backend,
    stats: ComputeStats,
    clock: u64,
}

// SAFETY: this object exclusively owns all handles and synchronous operations.
// It is accessed only behind the store's Mutex; no backend call or buffer access
// can overlap a move, eviction, compute or destruction on another blocking thread.
unsafe impl Send for NativeCompute {}

impl NativeCompute {
    #[cfg(any(feature = "cuda", test))]
    fn from_device(device: ffi::ggml_backend_dev_t, capacity_bytes: usize) -> Result<Self, Error> {
        if device.is_null() || capacity_bytes == 0 {
            return Err(Error::Invalid(
                "native device and cache capacity must be available",
            ));
        }
        // SAFETY: caller supplies a live GGML registry device, not an owned/freeable pointer.
        let backend = NonNull::new(unsafe { ffi::ggml_backend_dev_init(device, ptr::null()) })
            .map(Backend)
            .ok_or(Error::Invalid("cannot initialize native compute backend"))?;
        Ok(Self {
            matrices: HashMap::new(),
            backend,
            stats: ComputeStats {
                capacity_bytes,
                ..ComputeStats::default()
            },
            clock: 0,
        })
    }

    #[cfg(feature = "cuda")]
    pub(crate) fn cuda(device_index: usize, capacity_bytes: usize) -> Result<Self, Error> {
        let devices = crate::cuda_devices();
        let info = devices
            .get(device_index)
            .ok_or(Error::Invalid("requested GGML CUDA device is unavailable"))?;
        if capacity_bytes == 0
            || capacity_bytes
                .checked_add(CUDA_RESERVE_BYTES)
                .is_none_or(|required| required > info.free_memory_bytes)
        {
            return Err(Error::Invalid(
                "CUDA weight cache plus 2 GiB reserve exceeds free device memory",
            ));
        }
        let device = crate::cuda::cuda_device_handles()
            .get(device_index)
            .copied()
            .ok_or(Error::Invalid("requested GGML CUDA device is unavailable"))?;
        Self::from_device(device, capacity_bytes)
    }

    #[cfg(test)]
    pub(crate) fn cpu(capacity_bytes: usize) -> Result<Self, Error> {
        // SAFETY: registry initialization is process-global and the returned device is registry-owned.
        unsafe { ffi::ggml_backend_load_all() };
        // SAFETY: querying a valid device type returns a registry pointer or null, checked below.
        let device = unsafe { ffi::ggml_backend_dev_by_type(ffi::GGML_BACKEND_DEVICE_TYPE_CPU) };
        Self::from_device(device, capacity_bytes)
    }

    pub(crate) fn stats(&self) -> ComputeStats {
        self.stats
    }

    pub(crate) fn validate_matrix(&self, matrix: &GgufMatrixRows) -> Result<(), Error> {
        let source = MatrixSource::from_matrix(matrix)?;
        let (_context, _tensor, _required) = self.weight_metadata(&source)?;
        Ok(())
    }

    fn weight_metadata(
        &self,
        source: &MatrixSource,
    ) -> Result<(Context, NonNull<ffi::ggml_tensor>, usize), Error> {
        let context = Context::new(4096)?;
        let columns = i64::try_from(source.key.columns)
            .map_err(|_| Error::Invalid("native input width overflow"))?;
        let rows = i64::try_from(source.key.rows)
            .map_err(|_| Error::Invalid("native output width overflow"))?;
        // SAFETY: checked positive dimensions fit GGML's dimensions; context is live.
        let tensor = NonNull::new(unsafe {
            ffi::ggml_new_tensor_2d(context.0.as_ptr(), source.key.kind, columns, rows)
        })
        .ok_or(Error::Invalid("cannot create native weight tensor"))?;
        // SAFETY: tensor metadata is initialized from a validated GGUF matrix view.
        if unsafe { ffi::ggml_nbytes(tensor.as_ptr()) } != source.bytes {
            return Err(Error::Invalid("native encoded matrix byte size mismatch"));
        }
        // SAFETY: backend is live and uniquely owned under the compute lock.
        let buffer_type =
            unsafe { ffi::ggml_backend_get_default_buffer_type(self.backend.0.as_ptr()) };
        // SAFETY: the no_alloc context and backend type are live; this only measures alignment/padding.
        let required = unsafe {
            ffi::ggml_backend_alloc_ctx_tensors_from_buft_size(context.0.as_ptr(), buffer_type)
        };
        if required == 0 || required > self.stats.capacity_bytes {
            return Err(Error::Invalid(
                "one native weight matrix exceeds device cache capacity",
            ));
        }
        Ok((context, tensor, required))
    }

    fn load(&mut self, source: &MatrixSource) -> Result<(), Error> {
        self.stats.misses = self.stats.misses.saturating_add(1);
        let (context, tensor, required) = self.weight_metadata(source)?;
        // SAFETY: backend is live and exclusively accessed under the compute lock.
        let buffer_type =
            unsafe { ffi::ggml_backend_get_default_buffer_type(self.backend.0.as_ptr()) };
        while self.stats.retained_bytes > self.stats.capacity_bytes - required {
            let key = self
                .matrices
                .iter()
                .min_by_key(|(_, matrix)| matrix.used)
                .map(|(key, _)| *key)
                .ok_or(Error::Invalid("native weight cache accounting mismatch"))?;
            if let Some(matrix) = self.matrices.remove(&key) {
                self.stats.retained_bytes -= matrix.allocated_bytes;
                self.stats.evictions = self.stats.evictions.saturating_add(1);
                drop(matrix);
            }
        }
        // SAFETY: all pending backend operations are synchronous; measured allocation fits the cap.
        let buffer = NonNull::new(unsafe {
            ffi::ggml_backend_alloc_ctx_tensors_from_buft(context.0.as_ptr(), buffer_type)
        })
        .map(Buffer)
        .ok_or(Error::Invalid("native weight buffer allocation failed"))?;
        // SAFETY: buffer is initialized and uniquely owned.
        let allocated_bytes = unsafe { ffi::ggml_backend_buffer_get_size(buffer.0.as_ptr()) };
        if allocated_bytes != required {
            return Err(Error::Invalid(
                "native weight allocation differs from measured size",
            ));
        }
        // SAFETY: this buffer contains only immutable model weights, populated below before use.
        unsafe {
            ffi::ggml_backend_buffer_set_usage(
                buffer.0.as_ptr(),
                ffi::GGML_BACKEND_BUFFER_USAGE_WEIGHTS,
            )
        };
        let mut staging = vec![0_u8; source.bytes.min(UPLOAD_CHUNK_BYTES)];
        let mut offset = 0_usize;
        while offset < source.bytes {
            let count = staging.len().min(source.bytes - offset);
            let file_offset = source
                .key
                .offset
                .checked_add(
                    u64::try_from(offset)
                        .map_err(|_| Error::Invalid("native upload offset overflow"))?,
                )
                .ok_or(Error::Invalid("native upload file offset overflow"))?;
            source
                .file
                .read_exact_at(&mut staging[..count], file_offset)?;
            // SAFETY: the chunk is within the validated tensor, and tensor_set copies synchronously.
            unsafe {
                ffi::ggml_backend_tensor_set(
                    tensor.as_ptr(),
                    staging.as_ptr().cast(),
                    offset,
                    count,
                )
            };
            offset += count;
        }
        self.stats.weight_upload_bytes = self
            .stats
            .weight_upload_bytes
            .saturating_add(source.bytes as u64);
        self.stats.retained_bytes += allocated_bytes;
        self.stats.peak_retained_bytes = self
            .stats
            .peak_retained_bytes
            .max(self.stats.retained_bytes);
        self.matrices.insert(
            source.key,
            ResidentMatrix {
                _buffer: buffer,
                _context: context,
                _source: Arc::clone(&source.file),
                tensor,
                allocated_bytes,
                used: self.clock,
            },
        );
        Ok(())
    }

    pub(crate) fn project(
        &mut self,
        matrix: &GgufMatrixRows,
        input: &[f32],
    ) -> Result<Vec<f32>, Error> {
        let source = MatrixSource::from_matrix(matrix)?;
        if input.len() != source.key.columns || input.iter().any(|value| !value.is_finite()) {
            return Err(Error::Invalid(
                "native projection input width or values invalid",
            ));
        }
        let input_bytes = std::mem::size_of_val(input);
        let output_bytes = source
            .key
            .rows
            .checked_mul(4)
            .ok_or(Error::Invalid("native output byte count overflow"))?;
        if input_bytes
            .checked_add(output_bytes)
            .is_none_or(|bytes| bytes > MAX_ACTIVATION_BYTES)
        {
            return Err(Error::Invalid("native activation size exceeds 64 MiB"));
        }
        self.clock = self
            .clock
            .checked_add(1)
            .ok_or(Error::Invalid("native cache clock overflow"))?;
        if self.matrices.contains_key(&source.key) {
            self.stats.hits = self.stats.hits.saturating_add(1);
        } else {
            self.load(&source)?;
        }
        let weights = self
            .matrices
            .get_mut(&source.key)
            .ok_or(Error::Invalid("native weights missing after load"))?;
        weights.used = self.clock;
        let context = Context::new(64 * 1024)?;
        // SAFETY: dimensions were validated at load; both tensors/graph contexts outlive compute.
        let input_tensor = unsafe {
            ffi::ggml_new_tensor_1d(
                context.0.as_ptr(),
                ffi::GGML_TYPE_F32,
                source.key.columns as i64,
            )
        };
        if input_tensor.is_null() {
            return Err(Error::Invalid("cannot create native input tensor"));
        }
        // SAFETY: live weight tensor and input have the same inner dimension.
        let output_tensor =
            unsafe { ffi::ggml_mul_mat(context.0.as_ptr(), weights.tensor.as_ptr(), input_tensor) };
        if output_tensor.is_null() {
            return Err(Error::Invalid("cannot create native projection"));
        }
        // SAFETY: backend/tensor are live and this capability query does not execute the op.
        if !unsafe { ffi::ggml_backend_supports_op(self.backend.0.as_ptr(), output_tensor) } {
            return Err(Error::Invalid(
                "selected backend does not support this GGUF matrix operation",
            ));
        }
        // SAFETY: one operation plus its two leaves fit the bounded graph metadata context.
        let graph = unsafe { ffi::ggml_new_graph_custom(context.0.as_ptr(), 8, false) };
        if graph.is_null() {
            return Err(Error::Invalid("cannot create native compute graph"));
        }
        // SAFETY: output and graph are valid in the same live context.
        unsafe { ffi::ggml_build_forward_expand(graph, output_tensor) };
        // SAFETY: only graph input/output need storage; weights already own an external buffer.
        let buffer = NonNull::new(unsafe {
            ffi::ggml_backend_alloc_ctx_tensors(context.0.as_ptr(), self.backend.0.as_ptr())
        })
        .map(Buffer)
        .ok_or(Error::Invalid("cannot allocate native activation buffer"))?;
        // SAFETY: the checked slice matches the contiguous F32 input tensor; copy is synchronous.
        unsafe {
            ffi::ggml_backend_tensor_set(input_tensor, input.as_ptr().cast(), 0, input_bytes)
        };
        // SAFETY: all tensor contexts/buffers and backend remain live for synchronous compute.
        let status = unsafe { ffi::ggml_backend_graph_compute(self.backend.0.as_ptr(), graph) };
        // SAFETY: completion is explicit even on compute failure, before buffers can be freed/evicted.
        unsafe { ffi::ggml_backend_synchronize(self.backend.0.as_ptr()) };
        if status != ffi::GGML_STATUS_SUCCESS {
            return Err(Error::Invalid("native matrix compute failed"));
        }
        let mut output = vec![0.0_f32; source.key.rows];
        // SAFETY: output slice has the exact initialized tensor byte size after compute completed.
        unsafe {
            ffi::ggml_backend_tensor_get(output_tensor, output.as_mut_ptr().cast(), 0, output_bytes)
        };
        drop(buffer);
        if output.iter().any(|value| !value.is_finite()) {
            return Err(Error::Invalid(
                "native projection produced nonfinite output",
            ));
        }
        self.stats.activation_upload_bytes = self
            .stats
            .activation_upload_bytes
            .saturating_add(input_bytes as u64);
        self.stats.output_download_bytes = self
            .stats
            .output_download_bytes
            .saturating_add(output_bytes as u64);
        self.stats.matvecs = self.stats.matvecs.saturating_add(1);
        Ok(output)
    }
}
