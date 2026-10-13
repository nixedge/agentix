//! Small checked interface over the pinned GGUF metadata parser.
//!
//! This crate is intended for the isolated Qwen worker process. It does not
//! invoke llama.cpp model loading or architecture dispatch.

#![forbid(unsafe_op_in_unsafe_fn)]

#[cfg(feature = "cuda")]
mod cuda;

#[cfg(feature = "cuda")]
pub use cuda::{cuda_devices, cuda_f32_matvec, CudaDeviceInfo, CudaError};

use llama_cpp_sys_2 as ffi;
use std::{
    collections::{HashMap, HashSet},
    ffi::{CStr, CString},
    fmt,
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom},
    os::unix::fs::FileExt,
    path::{Path, PathBuf},
    ptr::{self, NonNull},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

const GGML_MAX_DIMS: usize = 4;
const GGML_TYPE_COUNT: i32 = ffi::GGML_TYPE_COUNT as i32;
const MAX_GGUF_BYTES: u64 = 512 * 1024 * 1024 * 1024;
const MAX_TENSORS: i64 = 1_000_000;
const MAX_METADATA_KEYS: i64 = 65_536;
const MAX_METADATA_STRING_BYTES: usize = 1024 * 1024;
const MAX_METADATA_COPY_BYTES: usize = 64 * 1024 * 1024;
const Q4_0_BLOCK_VALUES: usize = 32;
const Q4_0_BLOCK_BYTES: usize = 18;
const Q8_0_BLOCK_BYTES: usize = 34;
#[cfg(test)]
const K_BLOCK_VALUES: usize = 256;
const MAX_Q4_0_ROW_VALUES: usize = 16 * 1024 * 1024;
const MAX_DENSE_ROW_VALUES: usize = 16 * 1024 * 1024;
const IQ4_NL_VALUES: [i8; 16] = [
    -127, -104, -83, -65, -49, -35, -22, -10, 1, 13, 25, 38, 53, 69, 89, 113,
];

#[derive(Clone, Copy)]
enum RowQuantization {
    Q4_0,
    Iq4Nl,
    Q8_0,
    Native(i32),
}

impl RowQuantization {
    fn block_geometry(self) -> Result<(usize, usize), Error> {
        let (block_values, block_bytes) = match self {
            Self::Q4_0 | Self::Iq4Nl => (Q4_0_BLOCK_VALUES, Q4_0_BLOCK_BYTES),
            Self::Q8_0 => (Q4_0_BLOCK_VALUES, Q8_0_BLOCK_BYTES),
            Self::Native(ggml_type) => {
                if !(0..GGML_TYPE_COUNT).contains(&ggml_type) {
                    return Err(Error::Invalid("unknown native quantized GGML type"));
                }
                // SAFETY: the GGML type ID was checked against the pinned library.
                let block_values = unsafe { ffi::ggml_blck_size(ggml_type as ffi::ggml_type) };
                // SAFETY: the GGML type ID was checked against the pinned library.
                let block_bytes = unsafe { ffi::ggml_type_size(ggml_type as ffi::ggml_type) };
                (
                    usize::try_from(block_values)
                        .map_err(|_| Error::Invalid("invalid native quantized block width"))?,
                    block_bytes,
                )
            }
        };
        if block_values == 0 || block_bytes == 0 {
            return Err(Error::Invalid("invalid native quantized block geometry"));
        }
        Ok((block_values, block_bytes))
    }
}

/// Decode one GGML Q4_0 row into caller-owned FP32 values.
///
/// The input must contain exactly one complete row. This deliberately does not
/// read a tensor or allocate storage proportional to the full PLE table.
pub fn decode_q4_0_row(bytes: &[u8], row_width: usize) -> Result<Vec<f32>, Error> {
    if row_width == 0
        || row_width > MAX_Q4_0_ROW_VALUES
        || row_width.checked_rem(Q4_0_BLOCK_VALUES) != Some(0)
    {
        return Err(Error::Invalid("Q4_0 row width outside supported geometry"));
    }
    let expected_bytes = (row_width / Q4_0_BLOCK_VALUES)
        .checked_mul(Q4_0_BLOCK_BYTES)
        .ok_or(Error::Invalid("Q4_0 row byte size overflow"))?;
    if bytes.len() != expected_bytes {
        return Err(Error::Invalid("Q4_0 row has unexpected byte length"));
    }
    let mut decoded = Vec::new();
    decoded
        .try_reserve_exact(row_width)
        .map_err(|_| Error::Invalid("cannot allocate decoded Q4_0 row"))?;
    decoded.resize(row_width, 0.0);
    decode_q4_0_row_into(bytes, row_width, &mut decoded)?;
    Ok(decoded)
}

/// Decode one GGML Q4_0 row directly into a caller-owned FP32 buffer.
pub fn decode_q4_0_row_into(
    bytes: &[u8],
    row_width: usize,
    decoded: &mut [f32],
) -> Result<(), Error> {
    if row_width == 0
        || row_width > MAX_Q4_0_ROW_VALUES
        || row_width.checked_rem(Q4_0_BLOCK_VALUES) != Some(0)
    {
        return Err(Error::Invalid("Q4_0 row width outside supported geometry"));
    }
    if decoded.len() != row_width {
        return Err(Error::Invalid("Q4_0 output buffer has unexpected length"));
    }
    let block_count = row_width / Q4_0_BLOCK_VALUES;
    let expected_bytes = block_count
        .checked_mul(Q4_0_BLOCK_BYTES)
        .ok_or(Error::Invalid("Q4_0 row byte size overflow"))?;
    if bytes.len() != expected_bytes {
        return Err(Error::Invalid("Q4_0 row has unexpected byte length"));
    }

    let mut output_offset = 0;
    for block in bytes.chunks_exact(Q4_0_BLOCK_BYTES) {
        let scale = f16(u16::from_le_bytes([block[0], block[1]]));
        if !scale.is_finite() {
            return Err(Error::Invalid("Q4_0 row contains a non-finite scale"));
        }
        let quants = &block[2..];
        // GGML stores the first 16 values in the low nibbles and the next 16
        // values in the high nibbles, each centered by subtracting eight.
        for quant in quants.iter().take(16) {
            decoded[output_offset] = f32::from(i16::from(quant & 0x0f) - 8) * scale;
            output_offset += 1;
        }
        for quant in quants.iter().take(16) {
            decoded[output_offset] = f32::from(i16::from(quant >> 4) - 8) * scale;
            output_offset += 1;
        }
    }
    Ok(())
}

/// Decode one GGML IQ4_NL row using GGML's nonlinear 16-value codebook.
pub fn decode_iq4_nl_row(bytes: &[u8], row_width: usize) -> Result<Vec<f32>, Error> {
    if row_width == 0
        || row_width > MAX_Q4_0_ROW_VALUES
        || row_width.checked_rem(Q4_0_BLOCK_VALUES) != Some(0)
    {
        return Err(Error::Invalid(
            "IQ4_NL row width outside supported geometry",
        ));
    }
    let mut decoded = Vec::new();
    decoded
        .try_reserve_exact(row_width)
        .map_err(|_| Error::Invalid("cannot allocate decoded IQ4_NL row"))?;
    decoded.resize(row_width, 0.0);
    decode_iq4_nl_row_into(bytes, row_width, &mut decoded)?;
    Ok(decoded)
}

pub fn decode_iq4_nl_row_into(
    bytes: &[u8],
    row_width: usize,
    decoded: &mut [f32],
) -> Result<(), Error> {
    if row_width == 0
        || row_width > MAX_Q4_0_ROW_VALUES
        || row_width.checked_rem(Q4_0_BLOCK_VALUES) != Some(0)
    {
        return Err(Error::Invalid(
            "IQ4_NL row width outside supported geometry",
        ));
    }
    if decoded.len() != row_width || bytes.len() != (row_width / 32) * 18 {
        return Err(Error::Invalid("IQ4_NL row buffer has unexpected length"));
    }
    for (block_index, block) in bytes.chunks_exact(18).enumerate() {
        let scale = f16(u16::from_le_bytes([block[0], block[1]]));
        if !scale.is_finite() {
            return Err(Error::Invalid("IQ4_NL row contains a non-finite scale"));
        }
        let base = block_index * 32;
        for j in 0..16 {
            decoded[base + j] = scale * f32::from(IQ4_NL_VALUES[usize::from(block[j + 2] & 0x0f)]);
            decoded[base + j + 16] =
                scale * f32::from(IQ4_NL_VALUES[usize::from(block[j + 2] >> 4)]);
        }
    }
    Ok(())
}

/// Decode one GGML Q8_0 row into a caller-owned FP32 buffer.
pub fn decode_q8_0_row(bytes: &[u8], row_width: usize) -> Result<Vec<f32>, Error> {
    if row_width == 0
        || row_width > MAX_Q4_0_ROW_VALUES
        || row_width.checked_rem(Q4_0_BLOCK_VALUES) != Some(0)
    {
        return Err(Error::Invalid("Q8_0 row width outside supported geometry"));
    }
    let mut decoded = Vec::new();
    decoded
        .try_reserve_exact(row_width)
        .map_err(|_| Error::Invalid("cannot allocate decoded Q8_0 row"))?;
    decoded.resize(row_width, 0.0);
    decode_q8_0_row_into(bytes, row_width, &mut decoded)?;
    Ok(decoded)
}

/// Decode one GGML Q8_0 row directly into a caller-owned FP32 buffer.
pub fn decode_q8_0_row_into(
    bytes: &[u8],
    row_width: usize,
    decoded: &mut [f32],
) -> Result<(), Error> {
    if row_width == 0
        || row_width > MAX_Q4_0_ROW_VALUES
        || row_width.checked_rem(Q4_0_BLOCK_VALUES) != Some(0)
    {
        return Err(Error::Invalid("Q8_0 row width outside supported geometry"));
    }
    if decoded.len() != row_width {
        return Err(Error::Invalid("Q8_0 output buffer has unexpected length"));
    }
    let block_count = row_width / Q4_0_BLOCK_VALUES;
    let expected_bytes = block_count
        .checked_mul(Q8_0_BLOCK_BYTES)
        .ok_or(Error::Invalid("Q8_0 row byte size overflow"))?;
    if bytes.len() != expected_bytes {
        return Err(Error::Invalid("Q8_0 row has unexpected byte length"));
    }
    for (block_index, block) in bytes.chunks_exact(Q8_0_BLOCK_BYTES).enumerate() {
        let scale = f16(u16::from_le_bytes([block[0], block[1]]));
        if !scale.is_finite() {
            return Err(Error::Invalid("Q8_0 row contains a non-finite scale"));
        }
        let base = block_index * Q4_0_BLOCK_VALUES;
        for (index, quantized) in block[2..].iter().enumerate() {
            decoded[base + index] = scale * f32::from(i8::from_le_bytes([*quantized]));
        }
    }
    Ok(())
}

fn f16(bits: u16) -> f32 {
    let sign = u32::from(bits & 0x8000) << 16;
    let exponent = u32::from((bits >> 10) & 0x1f);
    let fraction = u32::from(bits & 0x03ff);
    if exponent == 0 {
        if fraction == 0 {
            f32::from_bits(sign)
        } else {
            let value = (fraction as f32) * 2.0_f32.powi(-24);
            if sign == 0 {
                value
            } else {
                -value
            }
        }
    } else if exponent == 0x1f {
        f32::from_bits(sign | 0x7f80_0000 | (fraction << 13))
    } else {
        f32::from_bits(sign | ((exponent + 112) << 23) | (fraction << 13))
    }
}

/// Metadata-validated, on-demand access to one supported quantized tensor.
pub struct QuantizedRows {
    file: Arc<File>,
    data_offset: u64,
    rows: u64,
    rows_per_group: u64,
    groups: u64,
    row_width: usize,
    row_bytes: usize,
    quantization: RowQuantization,
}

#[repr(C)]
struct NativeTypeTraits {
    _type_name: *const std::ffi::c_char,
    _block_size: i64,
    _block_size_interleave: i64,
    _type_size: usize,
    _is_quantized: bool,
    to_float: Option<unsafe extern "C" fn(*const std::ffi::c_void, *mut f32, i64)>,
}

extern "C" {
    fn ggml_get_type_traits(ggml_type: i32) -> *const NativeTypeTraits;
}

fn decode_native_quantized_row_into(
    ggml_type: i32,
    bytes: &[u8],
    output: &mut [f32],
) -> Result<(), Error> {
    let width = i64::try_from(output.len())
        .map_err(|_| Error::Invalid("quantized row width does not fit GGML"))?;
    let aligned_word_count = bytes
        .len()
        .checked_add(std::mem::size_of::<u64>() - 1)
        .ok_or(Error::Invalid("quantized row byte size overflow"))?
        / std::mem::size_of::<u64>();
    let mut aligned_bytes = Vec::<u64>::new();
    aligned_bytes
        .try_reserve_exact(aligned_word_count)
        .map_err(|_| Error::Invalid("cannot allocate aligned quantized row"))?;
    aligned_bytes.resize(aligned_word_count, 0);
    // SAFETY: `aligned_bytes` owns `aligned_word_count * size_of::<u64>()`
    // writable bytes; `bytes.len()` is no greater than that allocation.
    unsafe {
        ptr::copy_nonoverlapping(
            bytes.as_ptr(),
            aligned_bytes.as_mut_ptr().cast::<u8>(),
            bytes.len(),
        );
    }
    // SAFETY: `ggml_get_type_traits` is exported by the pinned llama.cpp build.
    // The supported GGML type callback accepts one complete row of its block
    // encoding and writes exactly `output.len()` floats. Callers validate the
    // tensor's block geometry and row byte length before invoking this helper.
    // The copied input has u64 alignment for GGML's block structs.
    unsafe {
        let traits = ggml_get_type_traits(ggml_type);
        if traits.is_null() {
            return Err(Error::Invalid("native GGML type traits are unavailable"));
        }
        let to_float = (*traits)
            .to_float
            .ok_or(Error::Invalid("native GGML type has no row decoder"))?;
        to_float(aligned_bytes.as_ptr().cast(), output.as_mut_ptr(), width);
    }
    if output.iter().all(|value| value.is_finite()) {
        Ok(())
    } else {
        Err(Error::Invalid("quantized row decoded to non-finite values"))
    }
}

/// Backwards-compatible name for existing Q4_0 callers.
pub type Q4_0Rows = QuantizedRows;

impl QuantizedRows {
    /// Opens a named supported quantized tensor after validating GGUF metadata.
    pub fn open(path: &Path, tensor_name: &str, row_width: usize) -> Result<Self, Error> {
        let info = inspect_gguf(path)?;
        let tensor = info
            .tensors
            .iter()
            .find(|tensor| tensor.name == tensor_name)
            .ok_or(Error::Invalid("quantized tensor name not found"))?;
        Self::open_from_info(path, &info, tensor, row_width)
    }

    fn open_from_info(
        path: &Path,
        info: &GgufInfo,
        tensor: &TensorInfo,
        row_width: usize,
    ) -> Result<Self, Error> {
        Self::open_with_file(Arc::new(File::open(path)?), info, tensor, row_width)
    }

    fn open_with_file(
        file: Arc<File>,
        info: &GgufInfo,
        tensor: &TensorInfo,
        row_width: usize,
    ) -> Result<Self, Error> {
        if row_width == 0
            || row_width > MAX_Q4_0_ROW_VALUES
            || row_width.checked_rem(Q4_0_BLOCK_VALUES) != Some(0)
        {
            return Err(Error::Invalid(
                "quantized row width outside supported geometry",
            ));
        }
        let row_width_u64 =
            u64::try_from(row_width).map_err(|_| Error::Invalid("quantized row width overflow"))?;
        let quantization = match tensor.type_name.as_str() {
            "q4_0" => RowQuantization::Q4_0,
            "iq4_nl" => RowQuantization::Iq4Nl,
            "q8_0" => RowQuantization::Q8_0,
            "iq2_xs" => RowQuantization::Native(ffi::GGML_TYPE_IQ2_XS as i32),
            "iq3_s" => RowQuantization::Native(ffi::GGML_TYPE_IQ3_S as i32),
            "iq2_s" => RowQuantization::Native(ffi::GGML_TYPE_IQ2_S as i32),
            "iq3_xxs" => RowQuantization::Native(ffi::GGML_TYPE_IQ3_XXS as i32),
            "iq4_xs" => RowQuantization::Native(ffi::GGML_TYPE_IQ4_XS as i32),
            "q2_0" => RowQuantization::Native(ffi::GGML_TYPE_Q2_0 as i32),
            "q4_K" | "q4_k" => RowQuantization::Native(ffi::GGML_TYPE_Q4_K as i32),
            "q5_K" | "q5_k" => RowQuantization::Native(ffi::GGML_TYPE_Q5_K as i32),
            "q6_K" | "q6_k" => RowQuantization::Native(ffi::GGML_TYPE_Q6_K as i32),
            _ => {
                return Err(Error::Invalid("unsupported quantized tensor type"));
            }
        };
        if tensor.extents[0] != row_width_u64 {
            return Err(Error::Invalid("quantized tensor row geometry mismatch"));
        }
        let rows_per_group = tensor.extents[1];
        let groups = tensor.extents[2..]
            .iter()
            .try_fold(1_u64, |total, &extent| total.checked_mul(extent))
            .ok_or(Error::Invalid("quantized tensor group count overflow"))?;
        let rows = rows_per_group
            .checked_mul(groups)
            .ok_or(Error::Invalid("quantized tensor row count overflow"))?;
        let (block_values, block_bytes) = quantization.block_geometry()?;
        if row_width.checked_rem(block_values) != Some(0) {
            return Err(Error::Invalid("quantized tensor row width mismatch"));
        }
        let row_bytes = row_width
            .checked_div(block_values)
            .and_then(|blocks| blocks.checked_mul(block_bytes))
            .ok_or(Error::Invalid("quantized row byte size overflow"))?;
        let tensor_bytes = u64::try_from(row_bytes)
            .ok()
            .and_then(|row_bytes| rows.checked_mul(row_bytes))
            .ok_or(Error::Invalid("quantized tensor byte size overflow"))?;
        if tensor.byte_size != tensor_bytes {
            return Err(Error::Invalid(
                "quantized tensor byte size disagrees with its row geometry",
            ));
        }
        let data_offset = info
            .data_offset
            .checked_add(tensor.offset)
            .ok_or(Error::Invalid("quantized data offset overflow"))?;
        let end = data_offset
            .checked_add(tensor_bytes)
            .ok_or(Error::Invalid("quantized tensor extent overflow"))?;
        if file.metadata()?.len() != info.file_size || end > info.file_size {
            return Err(Error::Invalid(
                "GGUF file changed after quantized metadata inspection",
            ));
        }
        Ok(Self {
            file,
            data_offset,
            rows,
            rows_per_group,
            groups,
            row_width,
            row_bytes,
            quantization,
        })
    }

    pub fn row_count(&self) -> u64 {
        self.rows
    }

    pub fn row_width(&self) -> usize {
        self.row_width
    }

    /// Rows along GGUF dimension one, useful for slicing stacked experts.
    pub fn rows_per_group(&self) -> u64 {
        self.rows_per_group
    }

    /// Number of groups in GGUF dimensions two and above.
    pub fn group_count(&self) -> u64 {
        self.groups
    }

    /// Reads and decodes only the requested row.
    pub fn read_row(&self, row: u64) -> Result<Vec<f32>, Error> {
        let mut output = Vec::new();
        output
            .try_reserve_exact(self.row_width)
            .map_err(|_| Error::Invalid("cannot allocate quantized row output"))?;
        output.resize(self.row_width, 0.0);
        self.read_row_into(row, &mut output)?;
        Ok(output)
    }

    /// Reads and decodes one row into a caller-owned buffer.
    pub fn read_row_into(&self, row: u64, output: &mut [f32]) -> Result<(), Error> {
        if row >= self.rows {
            return Err(Error::Invalid("quantized row index outside tensor"));
        }
        if output.len() != self.row_width {
            return Err(Error::Invalid(
                "quantized output buffer has unexpected length",
            ));
        }
        let offset = u64::try_from(self.row_bytes)
            .ok()
            .and_then(|row_bytes| row.checked_mul(row_bytes))
            .and_then(|relative| self.data_offset.checked_add(relative))
            .ok_or(Error::Invalid("quantized row offset overflow"))?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(self.row_bytes)
            .map_err(|_| Error::Invalid("cannot allocate quantized row buffer"))?;
        bytes.resize(self.row_bytes, 0);
        self.file.read_exact_at(&mut bytes, offset)?;
        match self.quantization {
            RowQuantization::Q4_0 => decode_q4_0_row_into(&bytes, self.row_width, output),
            RowQuantization::Iq4Nl => decode_iq4_nl_row_into(&bytes, self.row_width, output),
            RowQuantization::Q8_0 => decode_q8_0_row_into(&bytes, self.row_width, output),
            RowQuantization::Native(ggml_type) => {
                decode_native_quantized_row_into(ggml_type, &bytes, output)
            }
        }
    }
}

impl agentix_qwen::ple::EmbeddingRowReader for QuantizedRows {
    fn read_row(&mut self, row_id: u64, output: &mut [f32]) -> agentix_qwen::Result<()> {
        self.read_row_into(row_id, output)
            .map_err(|_| agentix_qwen::Error::Invalid("GGUF PLE row read or decode failed"))
    }
}

#[derive(Clone, Copy)]
enum DenseEncoding {
    F32,
    F16,
    Bf16,
}

impl DenseEncoding {
    fn element_bytes(self) -> usize {
        match self {
            Self::F32 => 4,
            Self::F16 | Self::Bf16 => 2,
        }
    }
}

/// Metadata-validated bounded row access for F32, F16 and BF16 GGUF tensors.
pub struct DenseRows {
    file: Arc<File>,
    data_offset: u64,
    rows: u64,
    row_width: usize,
    row_bytes: usize,
    encoding: DenseEncoding,
}

impl DenseRows {
    /// Opens a named dense floating-point tensor and validates its row geometry.
    pub fn open(path: &Path, tensor_name: &str, row_width: usize) -> Result<Self, Error> {
        let info = inspect_gguf(path)?;
        let tensor = info
            .tensors
            .iter()
            .find(|tensor| tensor.name == tensor_name)
            .ok_or(Error::Invalid("dense tensor name not found"))?;
        Self::open_from_info(path, &info, tensor, row_width)
    }

    fn open_from_info(
        path: &Path,
        info: &GgufInfo,
        tensor: &TensorInfo,
        row_width: usize,
    ) -> Result<Self, Error> {
        Self::open_with_file(Arc::new(File::open(path)?), info, tensor, row_width)
    }

    fn open_with_file(
        file: Arc<File>,
        info: &GgufInfo,
        tensor: &TensorInfo,
        row_width: usize,
    ) -> Result<Self, Error> {
        if row_width == 0 || row_width > MAX_DENSE_ROW_VALUES {
            return Err(Error::Invalid("dense row width outside supported geometry"));
        }
        let encoding = match tensor.type_name.as_str() {
            "f32" => DenseEncoding::F32,
            "f16" => DenseEncoding::F16,
            "bf16" => DenseEncoding::Bf16,
            _ => return Err(Error::Invalid("unsupported dense tensor type")),
        };
        let row_width_u64 =
            u64::try_from(row_width).map_err(|_| Error::Invalid("dense row width overflow"))?;
        if tensor.extents[0] != row_width_u64
            || tensor.extents[2..].iter().any(|&extent| extent != 1)
        {
            return Err(Error::Invalid("dense tensor row geometry mismatch"));
        }
        let row_bytes = row_width
            .checked_mul(encoding.element_bytes())
            .ok_or(Error::Invalid("dense row byte size overflow"))?;
        let data_offset = info
            .data_offset
            .checked_add(tensor.offset)
            .ok_or(Error::Invalid("dense data offset overflow"))?;
        let rows = tensor.extents[1];
        let end = u64::try_from(row_bytes)
            .ok()
            .and_then(|size| rows.checked_mul(size))
            .and_then(|size| data_offset.checked_add(size))
            .ok_or(Error::Invalid("dense tensor extent overflow"))?;
        if file.metadata()?.len() != info.file_size || end > info.file_size {
            return Err(Error::Invalid(
                "GGUF file changed after dense metadata inspection",
            ));
        }
        Ok(Self {
            file,
            data_offset,
            rows,
            row_width,
            row_bytes,
            encoding,
        })
    }

    pub fn row_count(&self) -> u64 {
        self.rows
    }

    pub fn row_width(&self) -> usize {
        self.row_width
    }

    pub fn read_row(&self, row: u64) -> Result<Vec<f32>, Error> {
        let mut output = Vec::new();
        output
            .try_reserve_exact(self.row_width)
            .map_err(|_| Error::Invalid("cannot allocate dense row output"))?;
        output.resize(self.row_width, 0.0);
        self.read_row_into(row, &mut output)?;
        Ok(output)
    }

    pub fn read_row_into(&self, row: u64, output: &mut [f32]) -> Result<(), Error> {
        if row >= self.rows {
            return Err(Error::Invalid("dense row index outside tensor"));
        }
        if output.len() != self.row_width {
            return Err(Error::Invalid("dense output buffer has unexpected length"));
        }
        let offset = u64::try_from(self.row_bytes)
            .ok()
            .and_then(|row_bytes| row.checked_mul(row_bytes))
            .and_then(|relative| self.data_offset.checked_add(relative))
            .ok_or(Error::Invalid("dense row offset overflow"))?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(self.row_bytes)
            .map_err(|_| Error::Invalid("cannot allocate dense row buffer"))?;
        bytes.resize(self.row_bytes, 0);
        self.file.read_exact_at(&mut bytes, offset)?;
        let element_bytes = self.encoding.element_bytes();
        for (index, element) in bytes.chunks_exact(element_bytes).enumerate() {
            let value = match self.encoding {
                DenseEncoding::F32 => {
                    f32::from_le_bytes([element[0], element[1], element[2], element[3]])
                }
                DenseEncoding::F16 => f16(u16::from_le_bytes([element[0], element[1]])),
                DenseEncoding::Bf16 => {
                    f32::from_bits(u32::from(u16::from_le_bytes([element[0], element[1]])) << 16)
                }
            };
            if !value.is_finite() {
                return Err(Error::Invalid("dense row contains a non-finite value"));
            }
            output[index] = value;
        }
        Ok(())
    }
}

fn tensor_byte_size(ggml_type: i32, extents: [u64; GGML_MAX_DIMS]) -> Result<u64, Error> {
    if !(0..GGML_TYPE_COUNT).contains(&ggml_type) {
        return Err(Error::Invalid("unknown GGML tensor type"));
    }
    // SAFETY: the enum range was validated against this pinned GGML library.
    let block_size = unsafe { ffi::ggml_blck_size(ggml_type as ffi::ggml_type) };
    let block_size = usize::try_from(block_size)
        .ok()
        .filter(|size| *size != 0)
        .ok_or(Error::Invalid("invalid GGML block size"))?;
    let first_extent = usize::try_from(extents[0])
        .map_err(|_| Error::Invalid("tensor extent exceeds address space"))?;
    if first_extent % block_size != 0 {
        return Err(Error::Invalid(
            "tensor row is not divisible by GGML block size",
        ));
    }
    // SAFETY: the enum range was validated against this pinned GGML library.
    let type_size = unsafe { ffi::ggml_type_size(ggml_type as ffi::ggml_type) };
    let row_bytes = (first_extent / block_size)
        .checked_mul(type_size)
        .ok_or(Error::Invalid("tensor row byte size overflow"))?;
    let rows = extents[1..].iter().try_fold(1_usize, |product, extent| {
        usize::try_from(*extent)
            .ok()
            .and_then(|extent| product.checked_mul(extent))
            .ok_or(Error::Invalid("tensor row count overflow"))
    })?;
    let total = row_bytes
        .checked_mul(rows)
        .ok_or(Error::Invalid("tensor byte size overflow"))?;
    u64::try_from(total).map_err(|_| Error::Invalid("tensor byte size overflow"))
}

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Invalid(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => error.fmt(f),
            Self::Invalid(reason) => write!(f, "invalid GGUF: {reason}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Invalid(_) => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TensorInfo {
    pub name: String,
    /// GGML enum value from the pinned native library.
    pub ggml_type: i32,
    /// Human-readable name returned by the same GGML library.
    pub type_name: String,
    /// GGML extents. Dimensions beyond the serialized rank are one.
    pub extents: [u64; GGML_MAX_DIMS],
    /// Byte offset from the beginning of the tensor data section.
    pub offset: u64,
    pub byte_size: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GgufInfo {
    pub version: u32,
    pub file_size: u64,
    pub data_offset: u64,
    pub alignment: u64,
    /// Optional GGUF `general.architecture` string, if present.
    pub architecture: Option<String>,
    /// Scalar metadata values and array descriptors; tensor/tokenizer payloads are not copied.
    pub metadata: Vec<GgufMetadata>,
    pub split_index: Option<u16>,
    pub split_count: u16,
    pub tensors: Vec<TensorInfo>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GgufMetadata {
    pub key: String,
    pub value: GgufMetadataValue,
}

#[derive(Clone, Debug, PartialEq)]
pub enum GgufMetadataValue {
    UInt8(u8),
    Int8(i8),
    UInt16(u16),
    Int16(i16),
    UInt32(u32),
    Int32(i32),
    Float32(f32),
    Bool(bool),
    String(String),
    Array { element_type: i32, length: u64 },
    UInt64(u64),
    Int64(i64),
    Float64(f64),
}

#[derive(Clone, Debug, PartialEq)]
pub struct GgufSetInfo {
    pub split_count: u16,
    pub total_tensors: usize,
    /// Ordered by GGUF `split.no`, regardless of input path order.
    pub shards: Vec<GgufInfo>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum QwenStorageType {
    F32,
    F16,
    Bf16,
    Q4_0,
    Iq4Nl,
    Q8_0,
    Iq2Xs,
    Iq3S,
    Iq2S,
    Iq3Xxs,
    Iq4Xs,
    Q2_0,
    Q4K,
    Q5K,
    Q6K,
}

impl QwenStorageType {
    pub fn type_name(self) -> &'static str {
        match self {
            Self::F32 => "f32",
            Self::F16 => "f16",
            Self::Bf16 => "bf16",
            Self::Q4_0 => "q4_0",
            Self::Iq4Nl => "iq4_nl",
            Self::Q8_0 => "q8_0",
            Self::Iq2Xs => "iq2_xs",
            Self::Iq3S => "iq3_s",
            Self::Iq2S => "iq2_s",
            Self::Iq3Xxs => "iq3_xxs",
            Self::Iq4Xs => "iq4_xs",
            Self::Q2_0 => "q2_0",
            Self::Q4K => "q4_K",
            Self::Q5K => "q5_K",
            Self::Q6K => "q6_K",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QwenTensorEntry {
    pub name: String,
    pub storage: QwenStorageType,
    pub extents: [u64; GGML_MAX_DIMS],
    /// Absolute file offset of this tensor payload.
    pub file_offset: u64,
    pub byte_size: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QwenTensorIndex {
    pub entries: Vec<QwenTensorEntry>,
}

/// Bounded row reader selected from a tensor's inspected GGML storage type.
pub enum GgufRows {
    Dense(DenseRows),
    Quantized(QuantizedRows),
}

/// A matrix view over a contiguous row range of a dense or quantized tensor.
pub struct GgufMatrixRows {
    rows: GgufRows,
    first_row: u64,
    output_features: u64,
    input_features: usize,
}

impl GgufMatrixRows {
    pub fn output_features(&self) -> u64 {
        self.output_features
    }

    pub fn input_features(&self) -> usize {
        self.input_features
    }
}

impl GgufRows {
    pub fn row_count(&self) -> u64 {
        match self {
            Self::Dense(rows) => rows.row_count(),
            Self::Quantized(rows) => rows.row_count(),
        }
    }

    pub fn row_width(&self) -> usize {
        match self {
            Self::Dense(rows) => rows.row_width(),
            Self::Quantized(rows) => rows.row_width(),
        }
    }

    pub fn read_row(&self, row: u64) -> Result<Vec<f32>, Error> {
        match self {
            Self::Dense(rows) => rows.read_row(row),
            Self::Quantized(rows) => rows.read_row(row),
        }
    }

    pub fn read_row_into(&self, row: u64, output: &mut [f32]) -> Result<(), Error> {
        match self {
            Self::Dense(rows) => rows.read_row_into(row, output),
            Self::Quantized(rows) => rows.read_row_into(row, output),
        }
    }

    fn open_with_file(
        file: Arc<File>,
        info: &GgufInfo,
        tensor: &TensorInfo,
    ) -> Result<Self, Error> {
        let row_width = usize::try_from(tensor.extents[0])
            .map_err(|_| Error::Invalid("GGUF tensor row width exceeds usize"))?;
        match tensor.type_name.as_str() {
            "f32" | "f16" | "bf16" => Ok(Self::Dense(DenseRows::open_with_file(
                file, info, tensor, row_width,
            )?)),
            _ => Ok(Self::Quantized(QuantizedRows::open_with_file(
                file, info, tensor, row_width,
            )?)),
        }
    }

    #[cfg(test)]
    fn open_from_info(path: &Path, info: &GgufInfo, tensor: &TensorInfo) -> Result<Self, Error> {
        Self::open_with_file(Arc::new(File::open(path)?), info, tensor)
    }
}

struct QwenGgufShard {
    file: Arc<File>,
    info: GgufInfo,
}

/// Schema-validated, split-aware Qwen GGUF directory with lazy tensor readers.
/// Tensor payloads are not loaded when this index is opened.
pub struct QwenGgufTensorStore {
    shards: Vec<QwenGgufShard>,
    index: QwenTensorIndex,
    locations: HashMap<String, (usize, usize)>,
}

impl QwenGgufTensorStore {
    /// Opens a complete split set and validates its aggregate Qwen tensor schema.
    pub fn open(
        paths: &[PathBuf],
        config: &agentix_qwen::config::QwenTextConfig,
    ) -> Result<Self, Error> {
        if paths.is_empty() {
            return Err(Error::Invalid("Qwen GGUF tensor store has no shards"));
        }
        let path_refs = paths.iter().map(PathBuf::as_path).collect::<Vec<_>>();
        let set = inspect_gguf_set(&path_refs)?;
        let mut path_by_split = HashMap::new();
        path_by_split
            .try_reserve(paths.len())
            .map_err(|_| Error::Invalid("cannot allocate Qwen GGUF shard paths"))?;
        for path in paths {
            let info = inspect_gguf(path)?;
            let split = info.split_index.unwrap_or(0);
            if path_by_split.insert(split, path.clone()).is_some() {
                return Err(Error::Invalid("duplicate Qwen GGUF split path"));
            }
        }

        let mut shards = Vec::new();
        shards
            .try_reserve_exact(set.shards.len())
            .map_err(|_| Error::Invalid("cannot allocate Qwen GGUF shard index"))?;
        for info in set.shards {
            let split = info.split_index.unwrap_or(0);
            let path = path_by_split
                .remove(&split)
                .ok_or(Error::Invalid("Qwen GGUF split path is missing"))?;
            shards.push(QwenGgufShard {
                file: Arc::new(File::open(path)?),
                info,
            });
        }
        let first = shards.first().ok_or(Error::Invalid(
            "Qwen GGUF tensor store has no parsed shards",
        ))?;
        let mut combined = first.info.clone();
        combined.tensors.clear();
        combined
            .tensors
            .try_reserve_exact(set.total_tensors)
            .map_err(|_| Error::Invalid("cannot allocate Qwen GGUF schema view"))?;
        for shard in &shards {
            combined.tensors.extend(shard.info.tensors.iter().cloned());
        }
        let report = validate_qwen4exp_gguf(&combined, config)?;
        if !report.is_valid() {
            return Err(Error::Invalid(
                "Qwen GGUF split set does not match the configured tensor schema",
            ));
        }

        let mut index_entries = Vec::new();
        index_entries
            .try_reserve_exact(set.total_tensors)
            .map_err(|_| Error::Invalid("cannot allocate Qwen GGUF tensor entries"))?;
        let mut locations = HashMap::new();
        locations
            .try_reserve(set.total_tensors)
            .map_err(|_| Error::Invalid("cannot allocate Qwen GGUF tensor lookup"))?;
        for (shard_index, shard) in shards.iter().enumerate() {
            let shard_index_entries = index_qwen4exp_tensors(&shard.info)?;
            for (tensor_index, entry) in shard_index_entries.entries.into_iter().enumerate() {
                if locations
                    .insert(entry.name.clone(), (shard_index, tensor_index))
                    .is_some()
                {
                    return Err(Error::Invalid("duplicate Qwen GGUF tensor name"));
                }
                index_entries.push(entry);
            }
        }
        Ok(Self {
            shards,
            index: QwenTensorIndex {
                entries: index_entries,
            },
            locations,
        })
    }

    pub fn tensor_index(&self) -> &QwenTensorIndex {
        &self.index
    }

    /// Opens one named dense or quantized tensor as an on-demand row reader.
    pub fn open_rows(&self, tensor_name: &str) -> Result<GgufRows, Error> {
        let &(shard_index, tensor_index) = self
            .locations
            .get(tensor_name)
            .ok_or(Error::Invalid("Qwen GGUF tensor is not indexed"))?;
        let shard = self
            .shards
            .get(shard_index)
            .ok_or(Error::Invalid("Qwen GGUF shard index is invalid"))?;
        let tensor = shard
            .info
            .tensors
            .get(tensor_index)
            .ok_or(Error::Invalid("Qwen GGUF tensor index is invalid"))?;
        GgufRows::open_with_file(Arc::clone(&shard.file), &shard.info, tensor)
    }

    /// Opens a rank-one or rank-two tensor as matrix rows for streamed linear ops.
    pub fn open_matrix_rows(&self, tensor_name: &str) -> Result<GgufMatrixRows, Error> {
        let &(shard_index, tensor_index) = self
            .locations
            .get(tensor_name)
            .ok_or(Error::Invalid("Qwen GGUF tensor is not indexed"))?;
        let tensor = self
            .shards
            .get(shard_index)
            .and_then(|shard| shard.info.tensors.get(tensor_index))
            .ok_or(Error::Invalid("Qwen GGUF tensor index is invalid"))?;
        if tensor.extents[2..].iter().any(|&extent| extent != 1) {
            return Err(Error::Invalid(
                "Qwen GGUF tensor is not a rank-one or rank-two matrix",
            ));
        }
        Ok(GgufMatrixRows {
            rows: self.open_rows(tensor_name)?,
            first_row: 0,
            output_features: tensor.extents[1],
            input_features: usize::try_from(tensor.extents[0])
                .map_err(|_| Error::Invalid("Qwen GGUF row width exceeds usize"))?,
        })
    }

    /// Opens one expert matrix from a rank-three `[in, out, experts]` tensor.
    pub fn open_group_matrix_rows(
        &self,
        tensor_name: &str,
        group_index: u64,
    ) -> Result<GgufMatrixRows, Error> {
        let &(shard_index, tensor_index) = self
            .locations
            .get(tensor_name)
            .ok_or(Error::Invalid("Qwen GGUF tensor is not indexed"))?;
        let tensor = self
            .shards
            .get(shard_index)
            .and_then(|shard| shard.info.tensors.get(tensor_index))
            .ok_or(Error::Invalid("Qwen GGUF tensor index is invalid"))?;
        if tensor.extents[2] <= 1 || tensor.extents[3] != 1 || group_index >= tensor.extents[2] {
            return Err(Error::Invalid(
                "Qwen GGUF expert group is outside tensor geometry",
            ));
        }
        let rows_per_group = tensor.extents[1];
        let first_row = group_index
            .checked_mul(rows_per_group)
            .ok_or(Error::Invalid("Qwen GGUF expert row offset overflow"))?;
        Ok(GgufMatrixRows {
            rows: self.open_rows(tensor_name)?,
            first_row,
            output_features: rows_per_group,
            input_features: usize::try_from(tensor.extents[0])
                .map_err(|_| Error::Invalid("Qwen GGUF row width exceeds usize"))?,
        })
    }
}

/// Loaded row-backed tensors and recurrent state for one GatedDeltaNet block.
/// Layer normalization and residual-stream mixing remain owned by the caller.
pub struct GatedDeltaNetGgufLayer {
    qkv: GgufMatrixRows,
    z: GgufMatrixRows,
    beta: GgufMatrixRows,
    decay: GgufMatrixRows,
    convolution: Vec<f32>,
    a_log: Vec<f32>,
    dt_bias: Vec<f32>,
    norm: Vec<f32>,
    output: GgufMatrixRows,
    state: agentix_qwen::reference::GatedDeltaNetState,
    epsilon: f32,
}

/// Bounded GGUF-backed PLE table and operators for one configured PLE layer.
pub struct PleGgufLayer {
    embedding: GgufRows,
    key_projection: GgufMatrixRows,
    value_projection: GgufMatrixRows,
    key_norm: Vec<f32>,
    query_norm: Vec<f32>,
    conv_norm: Vec<f32>,
    conv_weights: Vec<f32>,
    ngram: agentix_qwen::ngram::Ngram,
    convolution: agentix_qwen::reference::DepthwiseConvState,
    row_width: usize,
    hidden_size: usize,
    epsilon: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResidualMixerKind {
    Attention,
    FeedForward,
    Final,
}

/// Actual-GGUF multi-stream residual mixer for a block or final stream collapse.
pub struct ResidualMixerGguf {
    norm: Vec<f32>,
    down: GgufMatrixRows,
    up: GgufMatrixRows,
    injection: Option<GgufMatrixRows>,
    streams: usize,
    hidden_size: usize,
    epsilon: f32,
}

impl ResidualMixerGguf {
    pub fn open(
        store: &QwenGgufTensorStore,
        block: usize,
        kind: ResidualMixerKind,
        config: &agentix_qwen::config::QwenTextConfig,
    ) -> Result<Self, Error> {
        let (norm_name, down_name, up_name, injection_name) = match kind {
            ResidualMixerKind::Attention => (
                format!("blk.{block}.hc_attn_norm.weight"),
                format!("blk.{block}.hc_attn_down.weight"),
                format!("blk.{block}.hc_attn_up.weight"),
                Some(format!("blk.{block}.hc_attn_inject.weight")),
            ),
            ResidualMixerKind::FeedForward => (
                format!("blk.{block}.hc_ffn_norm.weight"),
                format!("blk.{block}.hc_ffn_down.weight"),
                format!("blk.{block}.hc_ffn_up.weight"),
                Some(format!("blk.{block}.hc_ffn_inject.weight")),
            ),
            ResidualMixerKind::Final => (
                "output_hc_norm.weight".to_owned(),
                "output_hc_down.weight".to_owned(),
                "output_hc_up.weight".to_owned(),
                None,
            ),
        };
        let norm = store.open_rows(&norm_name)?.read_row(0)?;
        let down = store.open_matrix_rows(&down_name)?;
        let up = store.open_matrix_rows(&up_name)?;
        let injection = injection_name
            .as_deref()
            .map(|name| store.open_matrix_rows(name))
            .transpose()?;
        let width = config
            .residual_streams
            .checked_mul(config.hidden_size)
            .ok_or(Error::Invalid("Qwen residual mixer width overflow"))?;
        let expected_width = width;
        let expected_rank = config.residual_rank;
        let matrix_matches = |matrix: &GgufMatrixRows, input, output| {
            matrix.input_features() == input
                && usize::try_from(matrix.output_features()).ok() == Some(output)
        };
        if norm.len() != expected_width
            || !matrix_matches(&down, expected_width, expected_rank)
            || !matrix_matches(&up, expected_rank, expected_width)
            || injection.as_ref().is_some_and(|matrix| {
                !matrix_matches(matrix, expected_width, config.residual_streams)
            })
        {
            return Err(Error::Invalid(
                "Qwen residual mixer tensor geometry mismatch",
            ));
        }
        Ok(Self {
            norm,
            down,
            up,
            injection,
            streams: config.residual_streams,
            hidden_size: config.hidden_size,
            epsilon: config.rms_norm_epsilon,
        })
    }

    pub fn mix(
        &mut self,
        hyper_input: &[f32],
    ) -> agentix_qwen::Result<agentix_qwen::reference::GatedResidualOutput> {
        agentix_qwen::reference::gated_residual_from_rows(
            hyper_input,
            &self.norm,
            &mut self.down,
            &mut self.up,
            self.injection.as_mut(),
            self.streams,
            self.hidden_size,
            self.epsilon,
        )
    }
}

impl PleGgufLayer {
    /// Opens the actual PLE tensor rows and validates the layer geometry.
    pub fn open(
        store: &QwenGgufTensorStore,
        block: usize,
        config: &agentix_qwen::config::QwenTextConfig,
    ) -> Result<Self, Error> {
        let one_based_block = block
            .checked_add(1)
            .ok_or(Error::Invalid("Qwen PLE block index overflow"))?;
        if !config.ple_layer_ids.contains(&one_based_block) {
            return Err(Error::Invalid("requested Qwen block has no PLE layer"));
        }
        let tensor_name = |suffix: &str| format!("blk.{block}.{suffix}");
        let stream_width = config
            .residual_streams
            .checked_mul(config.hidden_size)
            .ok_or(Error::Invalid("Qwen PLE stream width overflow"))?;
        let row_width = agentix_qwen::schema::ple_row_width(config)
            .map_err(|_| Error::Invalid("invalid Qwen PLE row geometry"))?;
        let expected_table_rows = agentix_qwen::schema::ple_table_rows(config)
            .map_err(|_| Error::Invalid("invalid Qwen PLE table geometry"))?;
        let embedding = store.open_rows("per_layer_token_embd.weight")?;
        let key_projection = store.open_matrix_rows(&tensor_name("ple_key.weight"))?;
        let value_projection = store.open_matrix_rows(&tensor_name("ple_value.weight"))?;
        let key_norm = store
            .open_rows(&tensor_name("ple_norm_key.weight"))?
            .read_row(0)?;
        let query_norm = store
            .open_rows(&tensor_name("ple_norm_query.weight"))?
            .read_row(0)?;
        let conv_norm = store
            .open_rows(&tensor_name("ple_norm_conv.weight"))?
            .read_row(0)?;
        let conv_rows = store.open_rows(&tensor_name("ple_conv1d.weight"))?;
        if embedding.row_width() != row_width
            || usize::try_from(embedding.row_count()).ok() != Some(expected_table_rows)
            || key_projection.input_features() != config.ple_embedding_dim
            || usize::try_from(key_projection.output_features()).ok() != Some(stream_width)
            || value_projection.input_features() != config.ple_embedding_dim
            || usize::try_from(value_projection.output_features()).ok() != Some(config.hidden_size)
            || key_norm.len() != stream_width
            || query_norm.len() != stream_width
            || conv_norm.len() != stream_width
            || usize::try_from(conv_rows.row_count()).ok() != Some(stream_width)
            || conv_rows.row_width() != config.ple_conv_kernel_size
        {
            return Err(Error::Invalid("Qwen PLE GGUF tensor geometry mismatch"));
        }
        let conv_value_count = stream_width
            .checked_mul(config.ple_conv_kernel_size)
            .ok_or(Error::Invalid("Qwen PLE convolution size overflow"))?;
        let mut conv_weights = Vec::new();
        conv_weights
            .try_reserve_exact(conv_value_count)
            .map_err(|_| Error::Invalid("cannot allocate Qwen PLE convolution weights"))?;
        for row in 0..stream_width {
            conv_weights.extend(
                conv_rows.read_row(
                    u64::try_from(row).map_err(|_| {
                        Error::Invalid("Qwen PLE convolution row index exceeds u64")
                    })?,
                )?,
            );
        }
        let ple_ordinal = config
            .ple_layer_ids
            .iter()
            .position(|layer_id| *layer_id == one_based_block)
            .ok_or(Error::Invalid("Qwen PLE layer ordinal is missing"))?;
        let ngram = agentix_qwen::ngram::Ngram::new(agentix_qwen::ngram::Config {
            vocabulary: u32::try_from(config.vocab_size)
                .map_err(|_| Error::Invalid("Qwen vocabulary exceeds u32"))?,
            eos: config.eos_token_id,
            size: config.ngram_size,
            heads_per_size: config.heads_per_ngram,
            vocabulary_base: u64::try_from(config.ngram_vocab_size_base)
                .map_err(|_| Error::Invalid("Qwen n-gram base exceeds u64"))?,
            layer_ordinal: ple_ordinal,
            seed: config.ngram_seed,
        })
        .map_err(|_| Error::Invalid("invalid Qwen PLE n-gram geometry"))?;
        let convolution = agentix_qwen::reference::DepthwiseConvState::new(
            stream_width,
            config.ple_conv_kernel_size,
            config.ngram_size,
        )
        .map_err(|_| Error::Invalid("invalid Qwen PLE convolution state geometry"))?;
        Ok(Self {
            embedding,
            key_projection,
            value_projection,
            key_norm,
            query_norm,
            conv_norm,
            conv_weights,
            ngram,
            convolution,
            row_width,
            hidden_size: config.hidden_size,
            epsilon: config.rms_norm_epsilon,
        })
    }

    /// Computes one token's PLE contribution and advances its private state.
    pub fn step(
        &mut self,
        token: u32,
        residual_streams: &[f32],
        is_padding: bool,
    ) -> agentix_qwen::Result<Vec<f32>> {
        agentix_qwen::ple::step_from_rows(
            &mut self.ngram,
            token,
            self.row_width,
            &mut self.embedding,
            residual_streams,
            self.hidden_size,
            self.epsilon,
            is_padding,
            &mut self.convolution,
            &mut self.key_projection,
            &mut self.value_projection,
            &self.key_norm,
            &self.query_norm,
            &self.conv_norm,
            &self.conv_weights,
        )
    }

    pub fn convolution_history(&self) -> &[f32] {
        self.convolution.history()
    }
}

impl GatedDeltaNetGgufLayer {
    /// Opens and validates the GGUF tensors for one configured recurrent block.
    pub fn open(
        store: &QwenGgufTensorStore,
        block: usize,
        config: &agentix_qwen::config::QwenTextConfig,
    ) -> Result<Self, Error> {
        if config.layer_types.get(block) != Some(&agentix_qwen::config::LayerType::GatedDeltaNet) {
            return Err(Error::Invalid("requested Qwen block is not GatedDeltaNet"));
        }
        let tensor_name = |suffix: &str| format!("blk.{block}.{suffix}");
        let qkv = store.open_matrix_rows(&tensor_name("attn_qkv.weight"))?;
        let z = store.open_matrix_rows(&tensor_name("attn_gate.weight"))?;
        let beta = store.open_matrix_rows(&tensor_name("ssm_beta.weight"))?;
        let decay = store.open_matrix_rows(&tensor_name("ssm_alpha.weight"))?;
        let output = store.open_matrix_rows(&tensor_name("ssm_out.weight"))?;
        let key_width = config
            .gdn_key_heads
            .checked_mul(config.gdn_key_head_dim)
            .ok_or(Error::Invalid("GatedDeltaNet key width overflow"))?;
        let value_width = config
            .gdn_value_heads
            .checked_mul(config.gdn_value_head_dim)
            .ok_or(Error::Invalid("GatedDeltaNet value width overflow"))?;
        let qkv_width = key_width
            .checked_mul(2)
            .and_then(|width| width.checked_add(value_width))
            .ok_or(Error::Invalid("GatedDeltaNet QKV width overflow"))?;
        let expect_matrix = |matrix: &GgufMatrixRows, input, output| {
            matrix.input_features() == input
                && usize::try_from(matrix.output_features()).ok() == Some(output)
        };
        if !expect_matrix(&qkv, config.hidden_size, qkv_width)
            || !expect_matrix(&z, config.hidden_size, value_width)
            || !expect_matrix(&beta, config.hidden_size, config.gdn_value_heads)
            || !expect_matrix(&decay, config.hidden_size, config.gdn_value_heads)
            || !expect_matrix(&output, value_width, config.hidden_size)
        {
            return Err(Error::Invalid(
                "GatedDeltaNet GGUF projection geometry mismatch",
            ));
        }

        let convolution_rows = store.open_rows(&tensor_name("ssm_conv1d.weight"))?;
        if convolution_rows.row_count()
            != u64::try_from(qkv_width)
                .map_err(|_| Error::Invalid("GatedDeltaNet channel count exceeds u64"))?
            || convolution_rows.row_width() != config.gdn_conv_kernel_size
        {
            return Err(Error::Invalid(
                "GatedDeltaNet GGUF convolution geometry mismatch",
            ));
        }
        let convolution_capacity = qkv_width
            .checked_mul(config.gdn_conv_kernel_size)
            .ok_or(Error::Invalid("GatedDeltaNet convolution size overflow"))?;
        let mut convolution = Vec::new();
        convolution
            .try_reserve_exact(convolution_capacity)
            .map_err(|_| Error::Invalid("cannot allocate GatedDeltaNet convolution"))?;
        for row in 0..convolution_rows.row_count() {
            convolution.extend(convolution_rows.read_row(row)?);
        }

        let vector = |suffix: &str| -> Result<Vec<f32>, Error> {
            store.open_rows(&tensor_name(suffix))?.read_row(0)
        };
        let a_log = vector("ssm_a")?;
        let dt_bias = vector("ssm_dt.bias")?;
        let norm = vector("ssm_norm.weight")?;
        if a_log.len() != config.gdn_value_heads
            || dt_bias.len() != config.gdn_value_heads
            || norm.len() != config.gdn_value_head_dim
        {
            return Err(Error::Invalid(
                "GatedDeltaNet GGUF vector geometry mismatch",
            ));
        }
        let state = agentix_qwen::reference::GatedDeltaNetState::new(
            config.hidden_size,
            config.gdn_key_heads,
            config.gdn_value_heads,
            config.gdn_key_head_dim,
            config.gdn_value_head_dim,
            config.gdn_conv_kernel_size,
        )
        .map_err(|_| Error::Invalid("invalid GatedDeltaNet state geometry"))?;
        Ok(Self {
            qkv,
            z,
            beta,
            decay,
            convolution,
            a_log,
            dt_bias,
            norm,
            output,
            state,
            epsilon: config.rms_norm_epsilon,
        })
    }

    pub fn state(&self) -> &agentix_qwen::reference::GatedDeltaNetState {
        &self.state
    }

    pub fn step(&mut self, hidden: &[f32]) -> agentix_qwen::Result<Vec<f32>> {
        self.state.step_from_rows(
            hidden,
            &mut self.qkv,
            &mut self.z,
            &mut self.beta,
            &mut self.decay,
            &self.convolution,
            &self.a_log,
            &self.dt_bias,
            &self.norm,
            &mut self.output,
            self.epsilon,
        )
    }

    pub fn process(&mut self, hidden_tokens: &[f32]) -> agentix_qwen::Result<Vec<f32>> {
        self.state.process_from_rows(
            hidden_tokens,
            &mut self.qkv,
            &mut self.z,
            &mut self.beta,
            &mut self.decay,
            &self.convolution,
            &self.a_log,
            &self.dt_bias,
            &self.norm,
            &mut self.output,
            self.epsilon,
        )
    }
}

pub struct QsaGgufLayerOutput {
    pub output: Vec<f32>,
    pub selected_token_indices: Vec<usize>,
}

/// Row-backed actual-GGUF QSA block with sequential per-request caches.
/// Padding/batch visibility and outer block normalization remain caller-owned.
pub struct QsaGgufLayer {
    indexer_query: GgufMatrixRows,
    indexer_key: GgufMatrixRows,
    indexer_query_norm: Vec<f32>,
    indexer_key_norm: Vec<f32>,
    attention_query_gate: GgufMatrixRows,
    attention_key: GgufMatrixRows,
    attention_value: GgufMatrixRows,
    attention_query_norm: Vec<f32>,
    attention_key_norm: Vec<f32>,
    attention_output: GgufMatrixRows,
    indexer_rope: agentix_qwen::reference::TextRotaryEmbedding,
    attention_rope: agentix_qwen::reference::TextRotaryEmbedding,
    geometry: agentix_qwen::reference::QsaDecodeGeometry,
    max_positions: usize,
    positions: Vec<u32>,
    raw_index_keys: Vec<f32>,
    attention_keys: Vec<f32>,
    attention_values: Vec<f32>,
}

impl QsaGgufLayer {
    /// Opens and validates one configured QSA block and initializes empty caches.
    pub fn open(
        store: &QwenGgufTensorStore,
        block: usize,
        config: &agentix_qwen::config::QwenTextConfig,
    ) -> Result<Self, Error> {
        if config.layer_types.get(block) != Some(&agentix_qwen::config::LayerType::Qsa)
            || config.indexer_kv_heads != 1
        {
            return Err(Error::Invalid(
                "requested Qwen block is not supported QSA geometry",
            ));
        }
        let tensor_name = |suffix: &str| format!("blk.{block}.{suffix}");
        let indexer_query = store.open_matrix_rows(&tensor_name("indexer.q_proj.weight"))?;
        let indexer_key = store.open_matrix_rows(&tensor_name("indexer.k_proj.weight"))?;
        let attention_query_gate = store.open_matrix_rows(&tensor_name("attn_q.weight"))?;
        let attention_key = store.open_matrix_rows(&tensor_name("attn_k.weight"))?;
        let attention_value = store.open_matrix_rows(&tensor_name("attn_v.weight"))?;
        let attention_output = store.open_matrix_rows(&tensor_name("attn_output.weight"))?;
        let read_vector = |suffix: &str| -> Result<Vec<f32>, Error> {
            store.open_rows(&tensor_name(suffix))?.read_row(0)
        };
        let indexer_query_norm = read_vector("indexer.q_norm.weight")?;
        let indexer_key_norm = read_vector("indexer.k_norm.weight")?;
        let attention_query_norm = read_vector("attn_q_norm.weight")?;
        let attention_key_norm = read_vector("attn_k_norm.weight")?;
        let indexer_query_width = config
            .indexer_query_heads
            .checked_mul(config.indexer_head_dim)
            .ok_or(Error::Invalid("QSA indexer query width overflow"))?;
        let attention_query_width = config
            .attention_heads
            .checked_mul(config.attention_head_dim)
            .ok_or(Error::Invalid("QSA attention query width overflow"))?;
        let attention_kv_width = config
            .attention_kv_heads
            .checked_mul(config.attention_head_dim)
            .ok_or(Error::Invalid("QSA attention KV width overflow"))?;
        let expect_matrix = |matrix: &GgufMatrixRows, input, output| {
            matrix.input_features() == input
                && usize::try_from(matrix.output_features()).ok() == Some(output)
        };
        let query_gate_width = attention_query_width
            .checked_mul(2)
            .ok_or(Error::Invalid("QSA query/gate width overflow"))?;
        if !expect_matrix(&indexer_query, config.hidden_size, indexer_query_width)
            || !expect_matrix(&indexer_key, config.hidden_size, config.indexer_head_dim)
            || !expect_matrix(&attention_query_gate, config.hidden_size, query_gate_width)
            || !expect_matrix(&attention_key, config.hidden_size, attention_kv_width)
            || !expect_matrix(&attention_value, config.hidden_size, attention_kv_width)
            || !expect_matrix(&attention_output, attention_query_width, config.hidden_size)
            || indexer_query_norm.len() != config.indexer_head_dim
            || indexer_key_norm.len() != config.indexer_head_dim
            || attention_query_norm.len() != config.attention_head_dim
            || attention_key_norm.len() != config.attention_head_dim
        {
            return Err(Error::Invalid("QSA GGUF projection geometry mismatch"));
        }
        let indexer_rotary_dim =
            ((config.indexer_head_dim as f32 * config.partial_rotary_factor).round()) as usize;
        let attention_rotary_dim =
            ((config.attention_head_dim as f32 * config.partial_rotary_factor).round()) as usize;
        let indexer_rope = agentix_qwen::reference::TextRotaryEmbedding::new(
            config.indexer_head_dim,
            indexer_rotary_dim,
            config.rope_theta,
        )
        .map_err(|_| Error::Invalid("invalid QSA indexer rotary geometry"))?;
        let attention_rope = agentix_qwen::reference::TextRotaryEmbedding::new(
            config.attention_head_dim,
            attention_rotary_dim,
            config.rope_theta,
        )
        .map_err(|_| Error::Invalid("invalid QSA attention rotary geometry"))?;
        Ok(Self {
            indexer_query,
            indexer_key,
            indexer_query_norm,
            indexer_key_norm,
            attention_query_gate,
            attention_key,
            attention_value,
            attention_query_norm,
            attention_key_norm,
            attention_output,
            indexer_rope,
            attention_rope,
            geometry: agentix_qwen::reference::QsaDecodeGeometry {
                indexer_query_heads: config.indexer_query_heads,
                indexer_head_dim: config.indexer_head_dim,
                query_heads: config.attention_heads,
                kv_heads: config.attention_kv_heads,
                head_dim: config.attention_head_dim,
                compress_ratio: config.indexer_compress_ratio,
                token_budget: config.indexer_token_budget,
                epsilon: config.rms_norm_epsilon,
            },
            max_positions: config.max_position_embeddings,
            positions: Vec::new(),
            raw_index_keys: Vec::new(),
            attention_keys: Vec::new(),
            attention_values: Vec::new(),
        })
    }

    pub fn step(
        &mut self,
        hidden: &[f32],
        position: u32,
    ) -> agentix_qwen::Result<QsaGgufLayerOutput> {
        let position_index =
            usize::try_from(position).map_err(|_| agentix_qwen::Error::Overflow)?;
        if position_index >= self.max_positions
            || position_index != self.positions.len()
            || self
                .positions
                .last()
                .is_some_and(|previous| position <= *previous)
        {
            return Err(agentix_qwen::Error::Invalid(
                "QSA layer positions must be sequential and within configured context",
            ));
        }
        let visible_len = self
            .positions
            .len()
            .checked_add(1)
            .ok_or(agentix_qwen::Error::Overflow)?;
        self.positions
            .try_reserve(1)
            .map_err(|_| agentix_qwen::Error::Capacity("QSA layer positions"))?;
        let mut visible = Vec::new();
        visible
            .try_reserve_exact(visible_len)
            .map_err(|_| agentix_qwen::Error::Capacity("QSA layer visibility"))?;
        visible.resize(visible_len, true);
        let decoded = agentix_qwen::reference::qsa_decode_step_from_rows(
            hidden,
            &self.raw_index_keys,
            &self.positions,
            &visible,
            &self.attention_keys,
            &self.attention_values,
            self.geometry,
            &self.indexer_rope,
            &self.attention_rope,
            position,
            &mut self.indexer_query,
            &mut self.indexer_key,
            &self.indexer_query_norm,
            &self.indexer_key_norm,
            &mut self.attention_query_gate,
            &mut self.attention_key,
            &mut self.attention_value,
            &self.attention_query_norm,
            &self.attention_key_norm,
            &mut self.attention_output,
        )?;
        self.raw_index_keys = decoded.raw_index_key_cache;
        self.attention_keys = decoded.attention_keys;
        self.attention_values = decoded.attention_values;
        self.positions.push(position);
        Ok(QsaGgufLayerOutput {
            output: decoded.output,
            selected_token_indices: decoded.selected_token_indices,
        })
    }

    pub fn cached_tokens(&self) -> usize {
        self.positions.len()
    }
}

/// Row-backed routed/shared expert layer for one Qwen text block.
pub struct MoeGgufLayer<'a> {
    store: &'a QwenGgufTensorStore,
    block: usize,
    expert_count: usize,
    experts_per_token: usize,
    expert_intermediate_size: usize,
    normalize_selected: bool,
    router: GgufMatrixRows,
    shared_gate: GgufMatrixRows,
    shared_up: GgufMatrixRows,
    shared_down: GgufMatrixRows,
    shared_expert_gate: GgufMatrixRows,
}

impl<'a> MoeGgufLayer<'a> {
    /// Opens and validates one block's router, expert stacks and shared expert.
    pub fn open(
        store: &'a QwenGgufTensorStore,
        block: usize,
        config: &agentix_qwen::config::QwenTextConfig,
    ) -> Result<Self, Error> {
        if block >= config.layer_types.len() {
            return Err(Error::Invalid("Qwen MoE block index is out of range"));
        }
        let tensor_name = |suffix: &str| format!("blk.{block}.{suffix}");
        let router = store.open_matrix_rows(&tensor_name("ffn_gate_inp.weight"))?;
        let shared_gate = store.open_matrix_rows(&tensor_name("ffn_gate_shexp.weight"))?;
        let shared_up = store.open_matrix_rows(&tensor_name("ffn_up_shexp.weight"))?;
        let shared_down = store.open_matrix_rows(&tensor_name("ffn_down_shexp.weight"))?;
        let shared_expert_gate =
            store.open_matrix_rows(&tensor_name("ffn_gate_inp_shexp.weight"))?;
        let expect_matrix = |matrix: &GgufMatrixRows, input, output| {
            matrix.input_features() == input
                && usize::try_from(matrix.output_features()).ok() == Some(output)
        };
        if !expect_matrix(&router, config.hidden_size, config.expert_count)
            || !expect_matrix(
                &shared_gate,
                config.hidden_size,
                config.shared_expert_intermediate_size,
            )
            || !expect_matrix(
                &shared_up,
                config.hidden_size,
                config.shared_expert_intermediate_size,
            )
            || !expect_matrix(
                &shared_down,
                config.shared_expert_intermediate_size,
                config.hidden_size,
            )
            || !expect_matrix(&shared_expert_gate, config.hidden_size, 1)
        {
            return Err(Error::Invalid("Qwen MoE projection geometry mismatch"));
        }
        let gate_name = tensor_name("ffn_gate_exps.weight");
        let up_name = tensor_name("ffn_up_exps.weight");
        let down_name = tensor_name("ffn_down_exps.weight");
        let find_extents = |name: &str| {
            store
                .tensor_index()
                .entries
                .iter()
                .find(|entry| entry.name == name)
                .map(|entry| entry.extents)
                .ok_or(Error::Invalid("Qwen MoE expert tensor is missing"))
        };
        let expected_gate_up = [
            u64::try_from(config.hidden_size)
                .map_err(|_| Error::Invalid("Qwen hidden size exceeds u64"))?,
            u64::try_from(config.expert_intermediate_size)
                .map_err(|_| Error::Invalid("Qwen expert size exceeds u64"))?,
            u64::try_from(config.expert_count)
                .map_err(|_| Error::Invalid("Qwen expert count exceeds u64"))?,
        ];
        let expected_down = [
            u64::try_from(config.expert_intermediate_size)
                .map_err(|_| Error::Invalid("Qwen expert size exceeds u64"))?,
            u64::try_from(config.hidden_size)
                .map_err(|_| Error::Invalid("Qwen hidden size exceeds u64"))?,
            u64::try_from(config.expert_count)
                .map_err(|_| Error::Invalid("Qwen expert count exceeds u64"))?,
        ];
        let normalized_extents =
            |extents: [u64; GGML_MAX_DIMS]| [extents[0], extents[1], extents[2]];
        if normalized_extents(find_extents(&gate_name)?) != expected_gate_up
            || normalized_extents(find_extents(&up_name)?) != expected_gate_up
            || normalized_extents(find_extents(&down_name)?) != expected_down
        {
            return Err(Error::Invalid("Qwen MoE expert stack geometry mismatch"));
        }
        Ok(Self {
            store,
            block,
            expert_count: config.expert_count,
            experts_per_token: config.experts_per_token,
            expert_intermediate_size: config.expert_intermediate_size,
            normalize_selected: config.norm_topk_prob,
            router,
            shared_gate,
            shared_up,
            shared_down,
            shared_expert_gate,
        })
    }

    pub fn forward(&mut self, input: &[f32]) -> agentix_qwen::Result<Vec<f32>> {
        let store = self.store;
        let block = self.block;
        let expert_count = self.expert_count;
        let expert_intermediate_size = self.expert_intermediate_size;
        agentix_qwen::reference::moe_block_from_rows(
            input,
            &mut self.router,
            self.experts_per_token,
            self.normalize_selected,
            |expert| {
                if expert >= expert_count {
                    return Err(agentix_qwen::Error::Invalid(
                        "Qwen MoE route exceeds expert count",
                    ));
                }
                let expert = u64::try_from(expert).map_err(|_| agentix_qwen::Error::Overflow)?;
                let open = |suffix: &str| {
                    store
                        .open_group_matrix_rows(&format!("blk.{block}.{suffix}"), expert)
                        .map_err(|_| agentix_qwen::Error::Invalid("cannot open Qwen expert rows"))
                };
                let gate = open("ffn_gate_exps.weight")?;
                let up = open("ffn_up_exps.weight")?;
                let down = open("ffn_down_exps.weight")?;
                if gate.output_features()
                    != u64::try_from(expert_intermediate_size)
                        .map_err(|_| agentix_qwen::Error::Overflow)?
                    || up.output_features() != gate.output_features()
                    || usize::try_from(down.output_features()).ok() != Some(input.len())
                {
                    return Err(agentix_qwen::Error::Invalid(
                        "Qwen selected expert geometry mismatch",
                    ));
                }
                Ok((gate, up, down))
            },
            &mut self.shared_gate,
            &mut self.shared_up,
            &mut self.shared_down,
            &mut self.shared_expert_gate,
        )
    }
}

enum QwenAttentionGgufLayer {
    GatedDeltaNet(GatedDeltaNetGgufLayer),
    Qsa(QsaGgufLayer),
}

struct QwenDecoderGgufBlock<'a> {
    attention_mixer: ResidualMixerGguf,
    attention: QwenAttentionGgufLayer,
    feed_forward_mixer: ResidualMixerGguf,
    moe: MoeGgufLayer<'a>,
}

/// Reusable single-sequence GGUF decoder with persistent recurrent, PLE, and
/// QSA state across tokens. A failed step poisons the decoder because earlier
/// blocks may already have committed state.
pub struct QwenGgufDecoder<'a> {
    config: agentix_qwen::config::QwenTextConfig,
    token_embeddings: GgufRows,
    ple_block: Option<usize>,
    ple: Option<PleGgufLayer>,
    blocks: Vec<QwenDecoderGgufBlock<'a>>,
    final_mixer: ResidualMixerGguf,
    output_projection: GgufMatrixRows,
    position: usize,
    poisoned: bool,
}

pub struct QwenGgufDecoderOutput {
    pub position: usize,
    pub logits: Vec<f32>,
}

/// Greedy output from a tokenizer-prepared prompt and the full GGUF decoder.
pub struct QwenGgufGenerationOutput {
    pub generation: agentix_qwen::tokenizer::GreedyGeneration,
    /// Logits after the final prompt token, before selecting the first output.
    pub prompt_logits: Vec<f32>,
}

impl<'a> QwenGgufDecoder<'a> {
    /// Opens the text embedding, all configured block readers, PLE, final
    /// stream combine, and output projection after the store schema validates.
    pub fn open(
        store: &'a QwenGgufTensorStore,
        config: &agentix_qwen::config::QwenTextConfig,
    ) -> Result<Self, Error> {
        if config.ple_layer_ids.len() > 1 {
            return Err(Error::Invalid("multiple Qwen PLE layers are unsupported"));
        }
        let token_embeddings = store.open_rows("token_embd.weight")?;
        let output_projection = store.open_matrix_rows("output.weight")?;
        if token_embeddings.row_width() != config.hidden_size
            || usize::try_from(token_embeddings.row_count()).ok() != Some(config.vocab_size)
            || output_projection.input_features() != config.hidden_size
            || usize::try_from(output_projection.output_features()).ok() != Some(config.vocab_size)
        {
            return Err(Error::Invalid("Qwen embedding or output geometry mismatch"));
        }
        let ple_block = config
            .ple_layer_ids
            .first()
            .map(|layer| {
                layer
                    .checked_sub(1)
                    .ok_or(Error::Invalid("Qwen PLE layer ID is zero"))
            })
            .transpose()?;
        if ple_block.is_some_and(|block| block >= config.layer_types.len()) {
            return Err(Error::Invalid("Qwen PLE block is outside layer stack"));
        }
        let ple = ple_block
            .map(|block| PleGgufLayer::open(store, block, config))
            .transpose()?;
        let mut blocks = Vec::new();
        blocks
            .try_reserve_exact(config.layer_types.len())
            .map_err(|_| Error::Invalid("cannot allocate Qwen decoder block state"))?;
        for (block, layer_type) in config.layer_types.iter().enumerate() {
            let attention = match layer_type {
                agentix_qwen::config::LayerType::GatedDeltaNet => {
                    QwenAttentionGgufLayer::GatedDeltaNet(GatedDeltaNetGgufLayer::open(
                        store, block, config,
                    )?)
                }
                agentix_qwen::config::LayerType::Qsa => {
                    QwenAttentionGgufLayer::Qsa(QsaGgufLayer::open(store, block, config)?)
                }
            };
            blocks.push(QwenDecoderGgufBlock {
                attention_mixer: ResidualMixerGguf::open(
                    store,
                    block,
                    ResidualMixerKind::Attention,
                    config,
                )?,
                attention,
                feed_forward_mixer: ResidualMixerGguf::open(
                    store,
                    block,
                    ResidualMixerKind::FeedForward,
                    config,
                )?,
                moe: MoeGgufLayer::open(store, block, config)?,
            });
        }
        let final_block = config
            .layer_types
            .len()
            .checked_sub(1)
            .ok_or(Error::Invalid("Qwen decoder has no blocks"))?;
        let final_mixer =
            ResidualMixerGguf::open(store, final_block, ResidualMixerKind::Final, config)?;
        Ok(Self {
            config: config.clone(),
            token_embeddings,
            ple_block,
            ple,
            blocks,
            final_mixer,
            output_projection,
            position: 0,
            poisoned: false,
        })
    }

    /// Processes one token and returns logits for the following token.
    pub fn step(&mut self, token_id: u32) -> agentix_qwen::Result<QwenGgufDecoderOutput> {
        if self.poisoned {
            return Err(agentix_qwen::Error::Invalid(
                "Qwen decoder cannot continue after a failed step",
            ));
        }
        if usize::try_from(token_id).map_or(true, |id| id >= self.config.vocab_size)
            || self.position >= self.config.max_position_embeddings
        {
            return Err(agentix_qwen::Error::Invalid(
                "Qwen decoder token or position is outside configured geometry",
            ));
        }
        let position = self.position;
        match self.forward_step(token_id, position) {
            Ok(logits) => {
                self.position =
                    self.position
                        .checked_add(1)
                        .ok_or(agentix_qwen::Error::Invalid(
                            "Qwen decoder position overflow",
                        ))?;
                Ok(QwenGgufDecoderOutput { position, logits })
            }
            Err(error) => {
                self.poisoned = true;
                Err(error)
            }
        }
    }

    /// Runs an already rendered, tokenized, and context-admitted prompt with
    /// deterministic greedy sampling. The decoder must be fresh for this
    /// request. As with common autoregressive workers, the final sampled token
    /// remains pending in the cache; drop this request-scoped decoder or feed
    /// that token with `step` before continuing the sequence.
    pub fn generate_prepared_greedy(
        &mut self,
        prompt: &agentix_qwen::tokenizer::PreparedPrompt,
        stop_token_ids: &[u32],
    ) -> agentix_qwen::Result<QwenGgufGenerationOutput> {
        let admission = prompt.admission;
        let expected_total = admission
            .prompt_tokens
            .checked_add(admission.requested_output_tokens)
            .ok_or(agentix_qwen::Error::Overflow)?;
        if self.position != 0
            || self.poisoned
            || prompt.token_ids.is_empty()
            || admission.prompt_tokens != prompt.token_ids.len()
            || admission.requested_output_tokens == 0
            || admission.total_tokens != expected_total
            || expected_total > self.config.max_position_embeddings
        {
            return Err(agentix_qwen::Error::Invalid(
                "Qwen prepared prompt is inconsistent or outside decoder context",
            ));
        }

        let mut consumed_tokens = 0_usize;
        let mut prompt_logits = None;
        let generation = agentix_qwen::tokenizer::generate_greedy(
            &prompt.token_ids,
            admission.requested_output_tokens,
            stop_token_ids,
            |context| {
                let mut logits = None;
                for token_id in &context[consumed_tokens..] {
                    logits = Some(self.step(*token_id)?.logits);
                }
                consumed_tokens = context.len();
                let logits = logits.ok_or(agentix_qwen::Error::Invalid(
                    "Qwen generation context did not advance",
                ))?;
                if prompt_logits.is_none() {
                    prompt_logits = Some(logits.clone());
                }
                Ok(logits)
            },
        )?;
        let prompt_logits = prompt_logits.ok_or(agentix_qwen::Error::Invalid(
            "Qwen generation produced no prompt logits",
        ))?;
        Ok(QwenGgufGenerationOutput {
            generation,
            prompt_logits,
        })
    }

    fn forward_step(&mut self, token_id: u32, position: usize) -> agentix_qwen::Result<Vec<f32>> {
        let embedding = self
            .token_embeddings
            .read_row(u64::from(token_id))
            .map_err(|_| agentix_qwen::Error::Invalid("Qwen token embedding row read failed"))?;
        let hyper_width = self
            .config
            .residual_streams
            .checked_mul(self.config.hidden_size)
            .ok_or(agentix_qwen::Error::Overflow)?;
        let mut hidden = Vec::new();
        hidden
            .try_reserve_exact(hyper_width)
            .map_err(|_| agentix_qwen::Error::Capacity("Qwen residual streams"))?;
        for _ in 0..self.config.residual_streams {
            hidden.extend_from_slice(&embedding);
        }
        for (block_index, block) in self.blocks.iter_mut().enumerate() {
            if self.ple_block == Some(block_index) {
                let ple = self
                    .ple
                    .as_mut()
                    .ok_or(agentix_qwen::Error::Invalid("Qwen PLE state is missing"))?;
                let update = ple.step(token_id, &hidden, false)?;
                if update.len() != hidden.len() {
                    return Err(agentix_qwen::Error::Invalid(
                        "Qwen PLE output width mismatch",
                    ));
                }
                for (residual, update) in hidden.iter_mut().zip(update) {
                    *residual += update;
                }
            }
            let mixed_attention = block.attention_mixer.mix(&hidden)?;
            let attention_output = match &mut block.attention {
                QwenAttentionGgufLayer::GatedDeltaNet(layer) => {
                    layer.step(&mixed_attention.mixed_input)?
                }
                QwenAttentionGgufLayer::Qsa(layer) => {
                    layer
                        .step(
                            &mixed_attention.mixed_input,
                            u32::try_from(position).map_err(|_| agentix_qwen::Error::Overflow)?,
                        )?
                        .output
                }
            };
            let after_attention = mixed_attention.inject(
                &attention_output,
                self.config.residual_streams,
                self.config.hidden_size,
            )?;
            let mixed_feed_forward = block.feed_forward_mixer.mix(&after_attention)?;
            let feed_forward_output = block.moe.forward(&mixed_feed_forward.mixed_input)?;
            hidden = mixed_feed_forward.inject(
                &feed_forward_output,
                self.config.residual_streams,
                self.config.hidden_size,
            )?;
            if hidden.len() != hyper_width || hidden.iter().any(|value| !value.is_finite()) {
                return Err(agentix_qwen::Error::Invalid(
                    "Qwen decoder block output is malformed or non-finite",
                ));
            }
        }
        let final_hidden = self.final_mixer.mix(&hidden)?.mixed_input;
        let logits = agentix_qwen::reference::linear_from_rows(
            &final_hidden,
            &mut self.output_projection,
            None,
        )?;
        if logits.len() != self.config.vocab_size || logits.iter().any(|value| !value.is_finite()) {
            return Err(agentix_qwen::Error::Invalid(
                "Qwen decoder logits are malformed or non-finite",
            ));
        }
        Ok(logits)
    }

    pub fn position(&self) -> usize {
        self.position
    }

    pub fn is_poisoned(&self) -> bool {
        self.poisoned
    }
}

impl agentix_qwen::reference::LinearWeightRows for GgufMatrixRows {
    fn output_features(&self) -> u64 {
        self.output_features
    }

    fn input_features(&self) -> usize {
        self.input_features
    }

    fn read_weight_row(&mut self, row: u64, output: &mut [f32]) -> agentix_qwen::Result<()> {
        if row >= self.output_features {
            return Err(agentix_qwen::Error::Invalid(
                "GGUF matrix row is outside selected view",
            ));
        }
        let tensor_row = self
            .first_row
            .checked_add(row)
            .ok_or(agentix_qwen::Error::Overflow)?;
        self.rows
            .read_row_into(tensor_row, output)
            .map_err(|_| agentix_qwen::Error::Invalid("GGUF weight row read failed"))
    }
}

impl agentix_qwen::reference::LinearWeightRows for GgufRows {
    fn output_features(&self) -> u64 {
        self.row_count()
    }

    fn input_features(&self) -> usize {
        self.row_width()
    }

    fn read_weight_row(&mut self, row: u64, output: &mut [f32]) -> agentix_qwen::Result<()> {
        self.read_row_into(row, output)
            .map_err(|_| agentix_qwen::Error::Invalid("GGUF weight row read failed"))
    }
}

impl agentix_qwen::ple::EmbeddingRowReader for GgufRows {
    fn read_row(&mut self, row: u64, output: &mut [f32]) -> agentix_qwen::Result<()> {
        self.read_row_into(row, output)
            .map_err(|_| agentix_qwen::Error::Invalid("GGUF PLE embedding row read failed"))
    }
}

fn qwen_storage_type(name: &str) -> Option<QwenStorageType> {
    match name {
        "f32" => Some(QwenStorageType::F32),
        "f16" => Some(QwenStorageType::F16),
        "bf16" => Some(QwenStorageType::Bf16),
        "q4_0" => Some(QwenStorageType::Q4_0),
        "iq4_nl" => Some(QwenStorageType::Iq4Nl),
        "q8_0" => Some(QwenStorageType::Q8_0),
        "iq2_xs" => Some(QwenStorageType::Iq2Xs),
        "iq3_s" => Some(QwenStorageType::Iq3S),
        "iq2_s" => Some(QwenStorageType::Iq2S),
        "iq3_xxs" => Some(QwenStorageType::Iq3Xxs),
        "iq4_xs" => Some(QwenStorageType::Iq4Xs),
        "q2_0" => Some(QwenStorageType::Q2_0),
        "q4_K" | "q4_k" => Some(QwenStorageType::Q4K),
        "q5_K" | "q5_k" => Some(QwenStorageType::Q5K),
        "q6_K" | "q6_k" => Some(QwenStorageType::Q6K),
        _ => None,
    }
}

/// Build a strict metadata-only directory for the currently supported Qwen GGUF types.
pub fn index_qwen4exp_gguf(
    info: &GgufInfo,
    config: &agentix_qwen::config::QwenTextConfig,
) -> Result<QwenTensorIndex, Error> {
    let report = validate_qwen4exp_gguf(info, config)?;
    if !report.is_valid() {
        return Err(Error::Invalid(
            "Qwen4-Exp GGUF names or extents do not match config",
        ));
    }
    index_qwen4exp_tensors(info)
}

fn index_qwen4exp_tensors(info: &GgufInfo) -> Result<QwenTensorIndex, Error> {
    let mut entries = Vec::new();
    entries
        .try_reserve_exact(info.tensors.len())
        .map_err(|_| Error::Invalid("cannot allocate Qwen GGUF tensor index"))?;
    for tensor in &info.tensors {
        let storage = qwen_storage_type(&tensor.type_name)
            .ok_or(Error::Invalid("unsupported Qwen4-Exp GGUF tensor type"))?;
        let file_offset = info
            .data_offset
            .checked_add(tensor.offset)
            .ok_or(Error::Invalid("Qwen GGUF tensor file offset overflow"))?;
        let end = file_offset
            .checked_add(tensor.byte_size)
            .ok_or(Error::Invalid("Qwen GGUF tensor payload end overflow"))?;
        if end > info.file_size {
            return Err(Error::Invalid(
                "Qwen GGUF tensor payload exceeds file bounds",
            ));
        }
        entries.push(QwenTensorEntry {
            name: tensor.name.clone(),
            storage,
            extents: tensor.extents,
            file_offset,
            byte_size: tensor.byte_size,
        });
    }
    Ok(QwenTensorIndex { entries })
}

/// Validate the GGUF tensor names and extents against the pinned Qwen4-Exp config.
pub fn validate_qwen4exp_gguf(
    info: &GgufInfo,
    config: &agentix_qwen::config::QwenTextConfig,
) -> Result<agentix_qwen::schema::TensorSchemaReport, Error> {
    if info
        .architecture
        .as_deref()
        .is_some_and(|architecture| architecture != "qwen4exp")
    {
        return Err(Error::Invalid("GGUF architecture is not qwen4exp"));
    }
    let expected = agentix_qwen::schema::qwen4exp_tensor_geometries(config)
        .map_err(|_| Error::Invalid("cannot construct Qwen4-Exp tensor schema"))?;
    let actual = info
        .tensors
        .iter()
        .map(|tensor| agentix_qwen::schema::TensorGeometry {
            name: tensor.name.clone(),
            extents: tensor.extents,
        })
        .collect::<Vec<_>>();
    agentix_qwen::schema::validate_tensor_geometries(&expected, &actual)
        .map_err(|_| Error::Invalid("cannot validate Qwen4-Exp tensor schema"))
}

fn metadata_u16(context: *const ffi::gguf_context, key: &str) -> Result<Option<u16>, Error> {
    let key = CString::new(key).map_err(|_| Error::Invalid("invalid GGUF metadata key"))?;
    // SAFETY: context is live and key is a NUL-terminated string.
    let id = unsafe { ffi::gguf_find_key(context, key.as_ptr()) };
    if id < 0 {
        return Ok(None);
    }
    // SAFETY: id is a valid key index returned by gguf_find_key.
    let kind = unsafe { ffi::gguf_get_kv_type(context, id) };
    if kind != ffi::GGUF_TYPE_UINT16 {
        return Err(Error::Invalid("GGUF split metadata must be uint16"));
    }
    // SAFETY: the key type was checked as uint16 and its index is valid.
    Ok(Some(unsafe { ffi::gguf_get_val_u16(context, id) }))
}

fn metadata_value(
    context: *const ffi::gguf_context,
    index: i64,
) -> Result<GgufMetadataValue, Error> {
    // SAFETY: index is within the KV count of the live GGUF context.
    let kind = unsafe { ffi::gguf_get_kv_type(context, index) };
    match kind {
        // SAFETY: this match arm verifies the exact GGUF type expected by the getter.
        ffi::GGUF_TYPE_UINT8 => Ok(GgufMetadataValue::UInt8(unsafe {
            ffi::gguf_get_val_u8(context, index)
        })),
        // SAFETY: this match arm verifies the exact GGUF type expected by the getter.
        ffi::GGUF_TYPE_INT8 => Ok(GgufMetadataValue::Int8(unsafe {
            ffi::gguf_get_val_i8(context, index)
        })),
        // SAFETY: this match arm verifies the exact GGUF type expected by the getter.
        ffi::GGUF_TYPE_UINT16 => Ok(GgufMetadataValue::UInt16(unsafe {
            ffi::gguf_get_val_u16(context, index)
        })),
        // SAFETY: this match arm verifies the exact GGUF type expected by the getter.
        ffi::GGUF_TYPE_INT16 => Ok(GgufMetadataValue::Int16(unsafe {
            ffi::gguf_get_val_i16(context, index)
        })),
        // SAFETY: this match arm verifies the exact GGUF type expected by the getter.
        ffi::GGUF_TYPE_UINT32 => Ok(GgufMetadataValue::UInt32(unsafe {
            ffi::gguf_get_val_u32(context, index)
        })),
        // SAFETY: this match arm verifies the exact GGUF type expected by the getter.
        ffi::GGUF_TYPE_INT32 => Ok(GgufMetadataValue::Int32(unsafe {
            ffi::gguf_get_val_i32(context, index)
        })),
        // SAFETY: this match arm verifies the exact GGUF type expected by the getter.
        ffi::GGUF_TYPE_FLOAT32 => Ok(GgufMetadataValue::Float32(unsafe {
            ffi::gguf_get_val_f32(context, index)
        })),
        // SAFETY: this match arm verifies the exact GGUF type expected by the getter.
        ffi::GGUF_TYPE_BOOL => Ok(GgufMetadataValue::Bool(unsafe {
            ffi::gguf_get_val_bool(context, index)
        })),
        ffi::GGUF_TYPE_STRING => {
            // SAFETY: index is valid and the KV type was checked as a string.
            let value = unsafe { ffi::gguf_get_val_str(context, index) };
            if value.is_null() {
                return Err(Error::Invalid("GGUF string metadata has no value"));
            }
            // SAFETY: the value is a context-owned NUL-terminated string.
            let value = unsafe { CStr::from_ptr(value) }
                .to_str()
                .map_err(|_| Error::Invalid("GGUF string metadata is not UTF-8"))?;
            if value.len() > MAX_METADATA_STRING_BYTES {
                return Err(Error::Invalid("GGUF metadata string exceeds size limit"));
            }
            Ok(GgufMetadataValue::String(value.to_owned()))
        }
        ffi::GGUF_TYPE_ARRAY => {
            // SAFETY: index is valid and its KV type is an array.
            let element_type = unsafe { ffi::gguf_get_arr_type(context, index) } as i32;
            // SAFETY: index is valid and its KV type is an array.
            let length = unsafe { ffi::gguf_get_arr_n(context, index) };
            Ok(GgufMetadataValue::Array {
                element_type,
                length: u64::try_from(length)
                    .map_err(|_| Error::Invalid("GGUF metadata array length overflow"))?,
            })
        }
        // SAFETY: this match arm verifies the exact GGUF type expected by the getter.
        ffi::GGUF_TYPE_UINT64 => Ok(GgufMetadataValue::UInt64(unsafe {
            ffi::gguf_get_val_u64(context, index)
        })),
        // SAFETY: this match arm verifies the exact GGUF type expected by the getter.
        ffi::GGUF_TYPE_INT64 => Ok(GgufMetadataValue::Int64(unsafe {
            ffi::gguf_get_val_i64(context, index)
        })),
        // SAFETY: this match arm verifies the exact GGUF type expected by the getter.
        ffi::GGUF_TYPE_FLOAT64 => Ok(GgufMetadataValue::Float64(unsafe {
            ffi::gguf_get_val_f64(context, index)
        })),
        _ => Err(Error::Invalid("unsupported GGUF metadata value type")),
    }
}

fn parse_metadata(context: *const ffi::gguf_context) -> Result<Vec<GgufMetadata>, Error> {
    // SAFETY: context is live.
    let count = unsafe { ffi::gguf_get_n_kv(context) };
    if !(0..=MAX_METADATA_KEYS).contains(&count) {
        return Err(Error::Invalid(
            "GGUF metadata key count outside supported bounds",
        ));
    }
    let capacity =
        usize::try_from(count).map_err(|_| Error::Invalid("GGUF metadata key count overflow"))?;
    let mut metadata = Vec::new();
    metadata
        .try_reserve_exact(capacity)
        .map_err(|_| Error::Invalid("cannot allocate GGUF metadata index"))?;
    let mut names = HashSet::new();
    names
        .try_reserve(capacity)
        .map_err(|_| Error::Invalid("cannot allocate GGUF metadata name index"))?;
    let mut copied_bytes = 0_usize;
    for index in 0..count {
        // SAFETY: index is within the KV count of the live GGUF context.
        let raw_key = unsafe { ffi::gguf_get_key(context, index) };
        if raw_key.is_null() {
            return Err(Error::Invalid("GGUF metadata key is missing"));
        }
        // SAFETY: the key is a context-owned NUL-terminated string.
        let key = unsafe { CStr::from_ptr(raw_key) }
            .to_str()
            .map_err(|_| Error::Invalid("GGUF metadata key is not UTF-8"))?;
        if key.is_empty() || key.len() > 4096 || !names.insert(key) {
            return Err(Error::Invalid("GGUF metadata key is invalid or duplicated"));
        }
        copied_bytes = copied_bytes
            .checked_add(key.len())
            .ok_or(Error::Invalid("GGUF metadata copy size overflow"))?;
        let value = metadata_value(context, index)?;
        if let GgufMetadataValue::String(string) = &value {
            copied_bytes = copied_bytes
                .checked_add(string.len())
                .ok_or(Error::Invalid("GGUF metadata copy size overflow"))?;
        }
        if copied_bytes > MAX_METADATA_COPY_BYTES {
            return Err(Error::Invalid("GGUF copied metadata exceeds size limit"));
        }
        metadata.push(GgufMetadata {
            key: key.to_owned(),
            value,
        });
    }
    Ok(metadata)
}

struct Context(NonNull<ffi::gguf_context>);

impl Drop for Context {
    fn drop(&mut self) {
        // SAFETY: this is the unique non-null context returned by gguf_init_from_file.
        unsafe { ffi::gguf_free(self.0.as_ptr()) };
    }
}

/// Parses GGUF metadata without allocating tensor payloads and validates every
/// advertised tensor byte range against the file length.
pub fn inspect_gguf(path: &Path) -> Result<GgufInfo, Error> {
    let file_size = std::fs::metadata(path)?.len();
    if !(24..=MAX_GGUF_BYTES).contains(&file_size) {
        return Err(Error::Invalid("file size outside supported bounds"));
    }
    let path = path
        .to_str()
        .ok_or(Error::Invalid("path is not valid UTF-8"))?;
    let path = CString::new(path).map_err(|_| Error::Invalid("path contains a NUL byte"))?;
    let params = ffi::gguf_init_params {
        no_alloc: true,
        ctx: ptr::null_mut(),
    };
    // SAFETY: `path` is a valid NUL-terminated string and params requests
    // metadata-only parsing, so no tensor payload buffer is allocated.
    let context = unsafe { ffi::gguf_init_from_file(path.as_ptr(), params) };
    let context = NonNull::new(context).ok_or(Error::Invalid("native parser rejected file"))?;
    let context = Context(context);
    let raw_context = context.0.as_ptr();

    let metadata = parse_metadata(raw_context)?;
    let architecture = metadata
        .iter()
        .find(|entry| entry.key == "general.architecture")
        .map(|entry| match &entry.value {
            GgufMetadataValue::String(value) => Ok(value.clone()),
            _ => Err(Error::Invalid(
                "GGUF architecture metadata must be a string",
            )),
        })
        .transpose()?;
    let split_count = metadata_u16(raw_context, "split.count")?.unwrap_or(1);
    let split_index = metadata_u16(raw_context, "split.no")?;
    if split_count == 0
        || (split_count == 1 && split_index.is_some_and(|index| index != 0))
        || split_index.is_some_and(|index| index >= split_count)
        || (split_count > 1 && split_index.is_none())
    {
        return Err(Error::Invalid("inconsistent GGUF split metadata"));
    }

    // SAFETY: all pointers below are owned by the live GGUF context and tensor
    // IDs are checked against its non-negative, bounded tensor count.
    let version = unsafe { ffi::gguf_get_version(raw_context) };
    if version != 3 {
        return Err(Error::Invalid("only GGUF version 3 is supported"));
    }
    let count = unsafe { ffi::gguf_get_n_tensors(raw_context) };
    if !(0..=MAX_TENSORS).contains(&count) {
        return Err(Error::Invalid("tensor count outside supported bounds"));
    }
    let data_offset = u64::try_from(unsafe { ffi::gguf_get_data_offset(raw_context) })
        .map_err(|_| Error::Invalid("data offset exceeds supported integer range"))?;
    let alignment = u64::try_from(unsafe { ffi::gguf_get_alignment(raw_context) })
        .map_err(|_| Error::Invalid("alignment exceeds supported integer range"))?;
    if alignment == 0 || !alignment.is_power_of_two() {
        return Err(Error::Invalid("invalid tensor alignment"));
    }
    if data_offset > file_size {
        return Err(Error::Invalid(
            "tensor data section begins past end of file",
        ));
    }

    let capacity = usize::try_from(count).map_err(|_| Error::Invalid("tensor count overflow"))?;
    let mut tensors = Vec::new();
    tensors
        .try_reserve_exact(capacity)
        .map_err(|_| Error::Invalid("cannot allocate tensor metadata"))?;
    let mut names = HashSet::new();
    names
        .try_reserve(capacity)
        .map_err(|_| Error::Invalid("cannot allocate tensor name index"))?;
    let mut ranges = Vec::new();
    ranges
        .try_reserve_exact(capacity)
        .map_err(|_| Error::Invalid("cannot allocate tensor range index"))?;

    for index in 0..count {
        // SAFETY: `index` is in [0,count), and the returned strings and extents
        // remain owned by `context`, which is live until this function returns.
        let raw_name = unsafe { ffi::gguf_get_tensor_name(raw_context, index) };
        if raw_name.is_null() {
            return Err(Error::Invalid("tensor has no name"));
        }
        let name = unsafe { CStr::from_ptr(raw_name) }
            .to_str()
            .map_err(|_| Error::Invalid("tensor name is not UTF-8"))?
            .to_owned();
        if name.is_empty() || !names.insert(name.clone()) {
            return Err(Error::Invalid("empty or duplicate tensor name"));
        }

        let raw_extents = unsafe { ffi::gguf_get_tensor_ne(raw_context, index) };
        if raw_extents.is_null() {
            return Err(Error::Invalid("tensor has no dimensions"));
        }
        let extents = unsafe { std::slice::from_raw_parts(raw_extents, GGML_MAX_DIMS) };
        let mut checked_extents = [1_u64; GGML_MAX_DIMS];
        for (destination, source) in checked_extents.iter_mut().zip(extents) {
            if *source <= 0 {
                return Err(Error::Invalid("tensor has non-positive extent"));
            }
            *destination =
                u64::try_from(*source).map_err(|_| Error::Invalid("tensor extent overflow"))?;
        }

        let ggml_type = unsafe { ffi::gguf_get_tensor_type(raw_context, index) } as i32;
        let expected_size = tensor_byte_size(ggml_type, checked_extents)?;
        // SAFETY: GGUF parsing produced a GGML type enum; ggml_type_name returns
        // a static library-owned C string for valid enum values.
        let raw_type_name = unsafe { ffi::ggml_type_name(ggml_type as ffi::ggml_type) };
        if raw_type_name.is_null() {
            return Err(Error::Invalid("unknown GGML tensor type"));
        }
        let type_name = unsafe { CStr::from_ptr(raw_type_name) }
            .to_str()
            .map_err(|_| Error::Invalid("GGML type name is not UTF-8"))?
            .to_owned();

        let offset = u64::try_from(unsafe { ffi::gguf_get_tensor_offset(raw_context, index) })
            .map_err(|_| Error::Invalid("tensor offset overflow"))?;
        if offset % alignment != 0 {
            return Err(Error::Invalid("tensor offset violates GGUF alignment"));
        }
        let byte_size = u64::try_from(unsafe { ffi::gguf_get_tensor_size(raw_context, index) })
            .map_err(|_| Error::Invalid("tensor byte size overflow"))?;
        if byte_size != expected_size {
            return Err(Error::Invalid(
                "tensor payload size does not match its type and shape",
            ));
        }
        let start = data_offset
            .checked_add(offset)
            .ok_or(Error::Invalid("tensor start offset overflow"))?;
        let end = start
            .checked_add(byte_size)
            .ok_or(Error::Invalid("tensor end offset overflow"))?;
        if byte_size == 0 || end > file_size {
            return Err(Error::Invalid("tensor payload exceeds file bounds"));
        }
        ranges.push((start, end));
        tensors.push(TensorInfo {
            name,
            ggml_type,
            type_name,
            extents: checked_extents,
            offset,
            byte_size,
        });
    }

    ranges.sort_unstable();
    if ranges.windows(2).any(|pair| pair[0].1 > pair[1].0) {
        return Err(Error::Invalid("tensor payload ranges overlap"));
    }

    Ok(GgufInfo {
        version,
        file_size,
        data_offset,
        alignment,
        architecture,
        metadata,
        split_index,
        split_count,
        tensors,
    })
}

fn ple_metadata_key(kind: agentix_qwen::reference::source::PleHashMetadataKind) -> &'static str {
    use agentix_qwen::reference::source::PleHashMetadataKind as Kind;
    match kind {
        Kind::LayerMultipliers => "qwen4exp.ple.layer_multipliers",
        Kind::HeadOffsets => "qwen4exp.ple.head_offsets",
        Kind::HeadVocabSizes => "qwen4exp.ple.head_vocab_sizes",
    }
}

fn create_metadata_temp(output: &Path) -> Result<(PathBuf, File), Error> {
    let parent = output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .canonicalize()?;
    let name = output
        .file_name()
        .ok_or(Error::Invalid("GGUF output path has no filename"))?
        .to_str()
        .ok_or(Error::Invalid("GGUF output filename is not UTF-8"))?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| Error::Invalid("system clock predates Unix epoch"))?
        .as_nanos();
    for attempt in 0..128_u32 {
        let temporary = parent.join(format!(
            ".{name}.agentix-qwen-{}-{timestamp}-{attempt}.tmp",
            std::process::id()
        ));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => return Ok((temporary, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(Error::Invalid(
        "could not reserve a unique GGUF output temporary",
    ))
}

fn metadata_array_matches_u64(
    context: *const ffi::gguf_context,
    key: &str,
    expected: &[u64],
) -> Result<bool, Error> {
    let key = CString::new(key).map_err(|_| Error::Invalid("invalid GGUF metadata key"))?;
    // SAFETY: context is live and key is NUL terminated.
    let key_id = unsafe { ffi::gguf_find_key(context, key.as_ptr()) };
    if key_id < 0 {
        return Ok(false);
    }
    // SAFETY: key_id was returned by gguf_find_key on this live context.
    if unsafe { ffi::gguf_get_kv_type(context, key_id) } != ffi::GGUF_TYPE_ARRAY
        || unsafe { ffi::gguf_get_arr_type(context, key_id) } != ffi::GGUF_TYPE_UINT64
        || unsafe { ffi::gguf_get_arr_n(context, key_id) } != expected.len()
    {
        return Ok(false);
    }
    if expected.is_empty() {
        return Ok(true);
    }
    // SAFETY: GGUF reports a UINT64 array of the checked length; its context owns
    // at least length * 8 bytes for this array until the context is freed.
    let data = unsafe { ffi::gguf_get_arr_data(context, key_id) };
    if data.is_null() {
        return Err(Error::Invalid("GGUF UINT64 array has no data"));
    }
    let byte_len = expected
        .len()
        .checked_mul(std::mem::size_of::<u64>())
        .ok_or(Error::Invalid("GGUF UINT64 array length overflow"))?;
    // SAFETY: data is a context-owned array pointer, and its checked GGUF type
    // and element count establish a readable byte range of byte_len bytes.
    let bytes = unsafe { std::slice::from_raw_parts(data.cast::<u8>(), byte_len) };
    for (index, expected_value) in expected.iter().enumerate() {
        let start = index
            .checked_mul(8)
            .ok_or(Error::Invalid("GGUF UINT64 array index overflow"))?;
        let chunk = bytes
            .get(start..start + 8)
            .ok_or(Error::Invalid("GGUF UINT64 array is truncated"))?;
        let chunk: [u8; 8] = chunk
            .try_into()
            .map_err(|_| Error::Invalid("GGUF UINT64 array element is truncated"))?;
        if u64::from_ne_bytes(chunk) != *expected_value {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Adds the pinned PLE UINT64 arrays to a Qwen4-Exp GGUF while streaming its
/// existing tensor payload unchanged into a new file. The output must not
/// already exist. Tensor schema validation remains a caller gate; this function
/// verifies GGUF structure, architecture, bounds, and the PLE metadata layout.
pub fn rewrite_qwen4exp_gguf_ple_hash_metadata(
    input_path: &Path,
    output_path: &Path,
    config: &agentix_qwen::config::QwenTextConfig,
    arrays: &[agentix_qwen::reference::source::ConvertedPleHashMetadata],
) -> Result<(), Error> {
    use agentix_qwen::reference::source::validate_qwen4exp_ple_hash_metadata;

    validate_qwen4exp_ple_hash_metadata(arrays, config)
        .map_err(|_| Error::Invalid("Qwen PLE metadata does not match config"))?;
    let source_info = inspect_gguf(input_path)?;
    if source_info.architecture.as_deref() != Some("qwen4exp") {
        return Err(Error::Invalid("GGUF architecture is not qwen4exp"));
    }
    let input_canonical = input_path.canonicalize()?;
    let parent = output_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .canonicalize()?;
    let output_name = output_path
        .file_name()
        .ok_or(Error::Invalid("GGUF output path has no filename"))?;
    let output_canonical = parent.join(output_name);
    if output_canonical == input_canonical {
        return Err(Error::Invalid("GGUF output path aliases its input"));
    }
    match fs::symlink_metadata(&output_canonical) {
        Ok(_) => return Err(Error::Invalid("GGUF output must not already exist")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }

    let input_text = input_path
        .to_str()
        .ok_or(Error::Invalid("GGUF input path is not UTF-8"))?;
    let input_c = CString::new(input_text).map_err(|_| Error::Invalid("GGUF path has NUL"))?;
    let params = ffi::gguf_init_params {
        no_alloc: true,
        ctx: ptr::null_mut(),
    };
    // SAFETY: input_c is a valid NUL-terminated path; no_alloc prevents tensor
    // payload allocation while retaining editable metadata in the context.
    let raw_context = unsafe { ffi::gguf_init_from_file(input_c.as_ptr(), params) };
    let context = NonNull::new(raw_context).ok_or(Error::Invalid("native parser rejected file"))?;
    let context = Context(context);
    let native_context = context.0.as_ptr();
    // SAFETY: context is live and gguf_get_data_offset reads its parsed header.
    let source_data_offset = u64::try_from(unsafe { ffi::gguf_get_data_offset(native_context) })
        .map_err(|_| Error::Invalid("GGUF source data offset overflow"))?;
    if source_data_offset != source_info.data_offset {
        return Err(Error::Invalid("GGUF changed after metadata inspection"));
    }

    let mut expected_arrays = Vec::new();
    expected_arrays
        .try_reserve_exact(arrays.len())
        .map_err(|_| Error::Invalid("cannot allocate Qwen PLE metadata"))?;
    for array in arrays {
        let mut values = Vec::new();
        values
            .try_reserve_exact(array.values.len())
            .map_err(|_| Error::Invalid("cannot allocate Qwen PLE metadata values"))?;
        for &value in &array.values {
            values.push(
                u64::try_from(value)
                    .map_err(|_| Error::Invalid("Qwen PLE metadata contains a negative value"))?,
            );
        }
        let key_text = ple_metadata_key(array.kind);
        let key = CString::new(key_text).map_err(|_| Error::Invalid("invalid PLE metadata key"))?;
        // SAFETY: context/key are valid; values is a live UINT64 buffer and the
        // native function copies n elements before returning.
        unsafe {
            ffi::gguf_set_arr_data(
                native_context,
                key.as_ptr(),
                ffi::GGUF_TYPE_UINT64,
                values.as_ptr().cast(),
                values.len(),
            );
        }
        expected_arrays.push((key_text, values));
    }
    // SAFETY: context remains live; meta_size recomputes the serialized header
    // extent after key/value edits and includes GGUF alignment padding.
    let output_data_offset = u64::try_from(unsafe { ffi::gguf_get_meta_size(native_context) })
        .map_err(|_| Error::Invalid("GGUF output metadata size overflow"))?;
    let source_payload_len = source_info
        .file_size
        .checked_sub(source_data_offset)
        .ok_or(Error::Invalid("GGUF source data section exceeds file"))?;
    let output_size = output_data_offset
        .checked_add(source_payload_len)
        .ok_or(Error::Invalid("rewritten GGUF size overflow"))?;
    if output_size > MAX_GGUF_BYTES {
        return Err(Error::Invalid("rewritten GGUF exceeds size limit"));
    }

    let (temporary, reserved_file) = create_metadata_temp(&output_canonical)?;
    drop(reserved_file);
    let result = (|| -> Result<(), Error> {
        let temporary_text = temporary
            .to_str()
            .ok_or(Error::Invalid("GGUF temporary path is not UTF-8"))?;
        let temporary_c = CString::new(temporary_text)
            .map_err(|_| Error::Invalid("GGUF temporary path has NUL"))?;
        // SAFETY: context and path are valid; only_meta writes the updated GGUF
        // header and aligned tensor-info section without tensor payload bytes.
        if !unsafe { ffi::gguf_write_to_file(native_context, temporary_c.as_ptr(), true) } {
            return Err(Error::Invalid("native GGUF metadata write failed"));
        }
        if fs::metadata(&temporary)?.len() != output_data_offset {
            return Err(Error::Invalid("rewritten GGUF metadata extent mismatch"));
        }
        let mut input = File::open(input_path)?;
        if input.metadata()?.len() != source_info.file_size {
            return Err(Error::Invalid("GGUF input changed during rewrite"));
        }
        input.seek(SeekFrom::Start(source_data_offset))?;
        let mut payload = input.take(source_payload_len);
        let mut output = OpenOptions::new().append(true).open(&temporary)?;
        let copied = std::io::copy(&mut payload, &mut output)?;
        if copied != source_payload_len || output.metadata()?.len() != output_size {
            return Err(Error::Invalid("rewritten GGUF payload copy was incomplete"));
        }
        drop(output);
        let rewritten_info = inspect_gguf(&temporary)?;
        if rewritten_info.architecture != source_info.architecture
            || rewritten_info.tensors != source_info.tensors
        {
            return Err(Error::Invalid(
                "rewritten GGUF changed architecture or tensor metadata",
            ));
        }
        let rewritten_text = temporary
            .to_str()
            .ok_or(Error::Invalid("GGUF temporary path is not UTF-8"))?;
        let rewritten_c = CString::new(rewritten_text)
            .map_err(|_| Error::Invalid("GGUF temporary path has NUL"))?;
        // SAFETY: rewritten_c names the complete file just checked by inspect_gguf.
        let rewritten_context = unsafe { ffi::gguf_init_from_file(rewritten_c.as_ptr(), params) };
        let rewritten_context = NonNull::new(rewritten_context)
            .ok_or(Error::Invalid("native parser rejected rewritten GGUF"))?;
        let rewritten_context = Context(rewritten_context);
        for (key, values) in &expected_arrays {
            if !metadata_array_matches_u64(rewritten_context.0.as_ptr(), key, values)? {
                return Err(Error::Invalid("Qwen PLE metadata write did not round-trip"));
            }
            let expected_length = u64::try_from(values.len())
                .map_err(|_| Error::Invalid("Qwen PLE metadata length overflow"))?;
            if !rewritten_info.metadata.iter().any(|entry| {
                entry.key == *key
                    && entry.value
                        == GgufMetadataValue::Array {
                            element_type: ffi::GGUF_TYPE_UINT64 as i32,
                            length: expected_length,
                        }
            }) {
                return Err(Error::Invalid(
                    "rewritten GGUF PLE metadata schema mismatch",
                ));
            }
        }
        Ok(())
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    if let Err(error) = fs::hard_link(&temporary, &output_canonical) {
        let _ = fs::remove_file(&temporary);
        return Err(error.into());
    }
    let _ = fs::remove_file(temporary);
    Ok(())
}

/// Validates the split numbering and tensor-name uniqueness for a complete GGUF set.
pub fn inspect_gguf_set(paths: &[&Path]) -> Result<GgufSetInfo, Error> {
    if paths.is_empty() || paths.len() > usize::from(u16::MAX) {
        return Err(Error::Invalid("GGUF set has an invalid shard count"));
    }
    let mut shards = Vec::new();
    shards
        .try_reserve_exact(paths.len())
        .map_err(|_| Error::Invalid("cannot allocate GGUF shard metadata"))?;
    for path in paths {
        shards.push(inspect_gguf(path)?);
    }
    let split_count = shards[0].split_count;
    if usize::from(split_count) != shards.len()
        || shards.iter().any(|shard| shard.split_count != split_count)
    {
        return Err(Error::Invalid(
            "GGUF shard set is incomplete or inconsistent",
        ));
    }
    for shard in &shards {
        if split_count > 1 && shard.split_index.is_none() {
            return Err(Error::Invalid("GGUF split is missing its index"));
        }
    }
    shards.sort_unstable_by_key(|shard| shard.split_index.unwrap_or(0));
    if shards
        .iter()
        .enumerate()
        .any(|(index, shard)| shard.split_index.unwrap_or(0) as usize != index)
    {
        return Err(Error::Invalid(
            "GGUF split indices are duplicated or missing",
        ));
    }
    let mut total_tensors = 0_usize;
    let mut names = HashSet::new();
    for shard in &shards {
        total_tensors = total_tensors
            .checked_add(shard.tensors.len())
            .ok_or(Error::Invalid("GGUF set tensor count overflow"))?;
        names
            .try_reserve(shard.tensors.len())
            .map_err(|_| Error::Invalid("cannot allocate GGUF tensor-name index"))?;
        for tensor in &shard.tensors {
            if !names.insert(tensor.name.as_str()) {
                return Err(Error::Invalid("duplicate tensor name across GGUF splits"));
            }
        }
    }
    Ok(GgufSetInfo {
        split_count,
        total_tensors,
        shards,
    })
}

/// Reports the GGML version compiled into this native boundary.
pub fn ggml_version() -> Result<String, Error> {
    // SAFETY: ggml_version returns a static NUL-terminated string.
    let version = unsafe { ffi::ggml_version() };
    if version.is_null() {
        return Err(Error::Invalid("native GGML version is unavailable"));
    }
    // SAFETY: checked non-null above; GGML documents this as a static C string.
    unsafe { CStr::from_ptr(version) }
        .to_str()
        .map(str::to_owned)
        .map_err(|_| Error::Invalid("native GGML version is not UTF-8"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn native_dequantize(
        ggml_type: i32,
        bytes: &[u8],
        width: usize,
    ) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
        let mut output = vec![0.0; width];
        let word_count = bytes.len().div_ceil(std::mem::size_of::<u64>());
        let mut aligned = vec![0_u64; word_count];
        // SAFETY: aligned owns enough writable bytes for the source payload.
        unsafe {
            ptr::copy_nonoverlapping(bytes.as_ptr(), aligned.as_mut_ptr().cast(), bytes.len());
        }
        let width = i64::try_from(width)?;
        // SAFETY: the pinned ggml.h declares this exported function and the
        // repr(C) prefix/layout above. The fixture contains one complete row
        // and output has the matching number of f32 elements.
        unsafe {
            let traits = ggml_get_type_traits(ggml_type);
            if traits.is_null() {
                return Err("GGML type traits are unavailable".into());
            }
            let to_float = (*traits).to_float.ok_or("GGML type has no dequantizer")?;
            to_float(aligned.as_ptr().cast(), output.as_mut_ptr(), width);
        }
        Ok(output)
    }

    #[test]
    fn validates_qwen4exp_schema_adapter() -> Result<(), Box<dyn std::error::Error>> {
        let config = agentix_qwen::config::QwenTextConfig::parse(include_bytes!(
            "../../agentix-qwen/tests/fixtures/qwen3_8_flash_next_config.json"
        ))?;
        let expected = agentix_qwen::schema::qwen4exp_tensor_geometries(&config)?;
        let supported_types = [
            "f32", "f16", "bf16", "q4_0", "iq4_nl", "q8_0", "iq2_xs", "iq3_s",
        ];
        let tensors = expected
            .into_iter()
            .enumerate()
            .map(|(index, tensor)| TensorInfo {
                name: tensor.name,
                ggml_type: 0,
                type_name: supported_types[index % supported_types.len()].to_owned(),
                extents: tensor.extents,
                offset: 0,
                byte_size: 0,
            })
            .collect();
        let info = GgufInfo {
            version: 3,
            file_size: 0,
            data_offset: 0,
            alignment: 32,
            architecture: Some("qwen4exp".to_owned()),
            metadata: Vec::new(),
            split_index: Some(0),
            split_count: 2,
            tensors,
        };
        assert!(validate_qwen4exp_gguf(&info, &config)?.is_valid());
        let tensor_index = index_qwen4exp_gguf(&info, &config)?;
        assert_eq!(tensor_index.entries.len(), 1224);
        assert_eq!(
            tensor_index
                .entries
                .iter()
                .map(|entry| entry.storage)
                .collect::<std::collections::HashSet<_>>()
                .len(),
            8
        );

        let mut unsupported = info.clone();
        unsupported.tensors[0].type_name = "iq1_m".to_owned();
        assert!(index_qwen4exp_gguf(&unsupported, &config).is_err());

        let mut out_of_bounds = info.clone();
        out_of_bounds.tensors[0].byte_size = 1;
        assert!(index_qwen4exp_gguf(&out_of_bounds, &config).is_err());

        let mut wrong_architecture = info;
        wrong_architecture.architecture = Some("llama".to_owned());
        assert!(validate_qwen4exp_gguf(&wrong_architecture, &config).is_err());
        Ok(())
    }

    #[test]
    fn decodes_q4_0_values_in_ggml_order() -> Result<(), Box<dyn std::error::Error>> {
        let mut block = vec![0_u8; Q4_0_BLOCK_BYTES];
        block[0..2].copy_from_slice(&0x3c00_u16.to_le_bytes()); // scale = 1
        for quant in &mut block[2..] {
            *quant = 0xf0;
        }
        let row = decode_q4_0_row(&block, Q4_0_BLOCK_VALUES)?;
        assert_eq!(&row[..16], &[-8.0; 16]);
        assert_eq!(&row[16..], &[7.0; 16]);
        Ok(())
    }

    #[test]
    fn decodes_iq4_nl_codebook_in_ggml_order() -> Result<(), Box<dyn std::error::Error>> {
        let mut block = vec![0_u8; 18];
        block[..2].copy_from_slice(&0x3c00_u16.to_le_bytes());
        for j in 0..16 {
            block[j + 2] = (j as u8) | ((j as u8) << 4);
        }
        let row = decode_iq4_nl_row(&block, 32)?;
        let expected = IQ4_NL_VALUES.map(f32::from);
        assert_eq!(&row[..16], &expected);
        assert_eq!(&row[16..], &expected);
        Ok(())
    }

    #[test]
    fn iq4_nl_decoder_matches_pinned_ggml_dequantizer() -> Result<(), Box<dyn std::error::Error>> {
        let mut block = vec![0_u8; 18];
        block[..2].copy_from_slice(&0x3555_u16.to_le_bytes());
        for (index, byte) in block[2..].iter_mut().enumerate() {
            *byte = (index as u8).wrapping_mul(29).wrapping_add(7);
        }
        let decoded = decode_iq4_nl_row(&block, 32)?;
        let native = native_dequantize(ffi::GGML_TYPE_IQ4_NL as i32, &block, 32)?;
        for (actual, expected) in decoded.iter().zip(native) {
            assert!((actual - expected).abs() <= 1.0e-7);
        }
        Ok(())
    }

    #[test]
    fn q8_0_decoder_matches_pinned_ggml_dequantizer() -> Result<(), Box<dyn std::error::Error>> {
        let mut block = vec![0_u8; Q8_0_BLOCK_BYTES];
        block[..2].copy_from_slice(&0x3555_u16.to_le_bytes());
        for (index, byte) in block[2..].iter_mut().enumerate() {
            *byte = (index as u8).wrapping_mul(37);
        }
        let decoded = decode_q8_0_row(&block, 32)?;
        let native = native_dequantize(ffi::GGML_TYPE_Q8_0 as i32, &block, 32)?;
        for (actual, expected) in decoded.iter().zip(native) {
            assert!((actual - expected).abs() <= 1.0e-7);
        }
        Ok(())
    }

    #[test]
    fn rejects_malformed_q8_0_rows() {
        assert!(decode_q8_0_row(&[], 0).is_err());
        assert!(decode_q8_0_row(&[], 31).is_err());
        assert!(decode_q8_0_row(&[0; Q8_0_BLOCK_BYTES - 1], 32).is_err());
        let mut invalid_scale = [0_u8; Q8_0_BLOCK_BYTES];
        invalid_scale[..2].copy_from_slice(&0x7c00_u16.to_le_bytes());
        assert!(decode_q8_0_row(&invalid_scale, 32).is_err());
    }

    #[test]
    fn q4_0_decoder_matches_pinned_ggml_dequantizer() -> Result<(), Box<dyn std::error::Error>> {
        let mut block = vec![0_u8; Q4_0_BLOCK_BYTES];
        block[..2].copy_from_slice(&0x3555_u16.to_le_bytes());
        for (index, byte) in block[2..].iter_mut().enumerate() {
            *byte = (index as u8).wrapping_mul(37).wrapping_add(9);
        }
        let decoded = decode_q4_0_row(&block, 32)?;
        let native = native_dequantize(ffi::GGML_TYPE_Q4_0 as i32, &block, 32)?;
        for (actual, expected) in decoded.iter().zip(native) {
            assert!((actual - expected).abs() <= 1.0e-7);
        }
        Ok(())
    }

    #[test]
    fn sampled_published_q4_0_row_matches_pinned_ggml_dequantizer(
    ) -> Result<(), Box<dyn std::error::Error>> {
        // First PLE row from the pinned ggml-org IQ4_NL artifact; SHA256 is in
        // ggml-qwen-ple-header.json. The PLE matrix itself is stored as Q4_0.
        let bytes = [
            88, 156, 122, 133, 108, 133, 154, 182, 121, 215, 72, 87, 82, 12, 169, 244, 35, 8, 104,
            28, 84, 135, 124, 135, 110, 166, 7, 119, 187, 185, 186, 13, 88, 151, 29, 102, 96, 28,
            249, 141, 191, 101, 25, 189, 136, 155, 171, 73, 18, 187, 90, 101, 39, 112, 32, 157, 90,
            175, 112, 89, 102, 110, 185, 165, 165, 117, 185, 188, 137, 139, 153, 134, 232, 155,
            142, 121, 199, 132, 94, 36, 90, 97, 20, 76, 4, 120, 200, 157, 223, 137,
        ];
        assert_eq!(bytes.len(), 90);
        let decoded = decode_q4_0_row(&bytes, 160)?;
        let native = native_dequantize(ffi::GGML_TYPE_Q4_0 as i32, &bytes, 160)?;
        for (actual, expected) in decoded.iter().zip(native) {
            assert!((actual - expected).abs() <= 1.0e-7);
        }
        Ok(())
    }

    #[test]
    fn source_bf16_ple_row_matches_published_q4_0_row_after_quantization(
    ) -> Result<(), Box<dyn std::error::Error>> {
        const SOURCE_ROW_HEX: &str = "17bc6a3c87bc5a3ce8bbe73ba7bbc13b083b853bdf3c8abcc0bb773ca83cf93abc3b3abad93b96ba89bb39bcc33bb1bc9a3c303c423c0b3d1bbc04bde03c033d90bc9ebb963c5cbbd33cdabbb9bba2bb4c3c873b163cac3c993995bbc03cf4bb44bc3539acbb54b913bc2b3c0dbdafbb373c703c533c05bd6abc9a3bf8bc0fbcc53bb33ce93c3dbc793bb03c96ba473c583c383bdcbc663c033c6ebcb2bb0cbd013d5037723c1abc01bd5c3cb7ba5e3bd33b84bcefbc5f3c33bc10bcc7bc85bb1bbc1abd243dd8bb093cffbcf3bb8f3c583c4e3c2cbb90bc9fbb70bcd3bbfb3b593c3ebcdf3b633c2f3c213c8fbc1dbc0abcb63b58bc69bc203b513a83bb0dbbbabc3abb523b753cc3bc7d3c17bcdf3c5f3c6abc8a3c63ba4d3a97bcdfbca6bbe2ba403b5ebcb8ba553cc13c333ce03bd93c6a3cfd3c9a3b83bc2cbba8bca93a";
        let source_bytes: Vec<u8> = SOURCE_ROW_HEX
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| {
                let hex = std::str::from_utf8(pair)?;
                Ok(u8::from_str_radix(hex, 16)?)
            })
            .collect::<Result<_, Box<dyn std::error::Error>>>()?;
        assert_eq!(source_bytes.len(), 320);

        let tensor_bytes = 2_500_012_u64 * 160 * 2;
        let mut header = format!(
            "{{\"model.language_model.layers.1.ple.ple_embedding.ngram_embedding.shard_0.weight\":{{\"dtype\":\"BF16\",\"shape\":[2500012,160],\"data_offsets\":[0,{tensor_bytes}]}}}}"
        )
        .into_bytes();
        while header.len() % 8 != 0 {
            header.push(b' ');
        }
        let data_offset = 8_u64 + u64::try_from(header.len())?;
        let fixture = TempFile::new();
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&fixture.0)?;
        file.write_all(&u64::try_from(header.len())?.to_le_bytes())?;
        file.write_all(&header)?;
        file.set_len(data_offset + tensor_bytes)?;
        file.write_all_at(&source_bytes, data_offset)?;
        drop(file);

        let safetensors =
            agentix_qwen::rows::SafeTensorFile::open(std::fs::File::open(&fixture.0)?)?;
        let mut source_row = safetensors.bf16_rows(
            "model.language_model.layers.1.ple.ple_embedding.ngram_embedding.shard_0.weight",
            [2_500_012, 160],
            0,
        )?;
        let mut source = [0.0_f32; 160];
        source_row.read_f32_row(0, &mut source)?;

        let target_bytes = [
            88, 156, 122, 133, 108, 133, 154, 182, 121, 215, 72, 87, 82, 12, 169, 244, 35, 8, 104,
            28, 84, 135, 124, 135, 110, 166, 7, 119, 187, 185, 186, 13, 88, 151, 29, 102, 96, 28,
            249, 141, 191, 101, 25, 189, 136, 155, 171, 73, 18, 187, 90, 101, 39, 112, 32, 157, 90,
            175, 112, 89, 102, 110, 185, 165, 165, 117, 185, 188, 137, 139, 153, 134, 232, 155,
            142, 121, 199, 132, 94, 36, 90, 97, 20, 76, 4, 120, 200, 157, 223, 137,
        ];
        let target = decode_q4_0_row(&target_bytes, 160)?;
        let max_abs_error = source
            .iter()
            .zip(target)
            .map(|(source, target)| (source - target).abs())
            .fold(0.0_f32, f32::max);
        assert!(
            max_abs_error <= 0.003,
            "maximum row error was {max_abs_error}"
        );
        Ok(())
    }

    #[test]
    fn sampled_published_iq4_nl_row_matches_pinned_ggml_dequantizer(
    ) -> Result<(), Box<dyn std::error::Error>> {
        // First 90 bytes of per_layer_token_embd.weight at file offset 192 from
        // the pinned IQ3_S artifact; SHA256 is recorded in the evidence JSON.
        let bytes = [
            81, 140, 107, 132, 109, 132, 154, 182, 106, 230, 55, 87, 65, 13, 186, 243, 18, 7, 115,
            12, 67, 134, 109, 135, 94, 182, 6, 102, 188, 201, 203, 13, 72, 150, 30, 86, 81, 12,
            250, 142, 207, 84, 9, 206, 120, 156, 172, 57, 17, 204, 90, 84, 38, 112, 94, 141, 90,
            191, 96, 74, 86, 94, 202, 164, 165, 101, 185, 188, 121, 139, 154, 134, 13, 140, 142,
            121, 199, 131, 78, 19, 75, 97, 20, 60, 3, 104, 216, 157, 239, 122,
        ];
        assert_eq!(bytes.len(), 90);
        let decoded = decode_iq4_nl_row(&bytes, 160)?;
        let native = native_dequantize(ffi::GGML_TYPE_IQ4_NL as i32, &bytes, 160)?;
        for (actual, expected) in decoded.iter().zip(native) {
            assert!((actual - expected).abs() <= 1.0e-7);
        }
        Ok(())
    }

    #[test]
    fn rejects_malformed_iq4_nl_rows() {
        assert!(decode_iq4_nl_row(&[], 0).is_err());
        assert!(decode_iq4_nl_row(&[], 31).is_err());
        assert!(decode_iq4_nl_row(&[0; 17], 32).is_err());
        let mut invalid_scale = [0_u8; 18];
        invalid_scale[..2].copy_from_slice(&0x7c00_u16.to_le_bytes());
        assert!(decode_iq4_nl_row(&invalid_scale, 32).is_err());
    }

    #[test]
    fn rejects_malformed_q4_0_rows() {
        assert!(decode_q4_0_row(&[], 0).is_err());
        assert!(decode_q4_0_row(&[], 31).is_err());
        assert!(decode_q4_0_row(&[0; Q4_0_BLOCK_BYTES - 1], 32).is_err());
        let mut invalid_scale = [0_u8; Q4_0_BLOCK_BYTES];
        invalid_scale[..2].copy_from_slice(&0x7c00_u16.to_le_bytes()); // +Inf
        assert!(decode_q4_0_row(&invalid_scale, 32).is_err());
    }
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TempFile(std::path::PathBuf);

    impl TempFile {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |duration| duration.as_nanos());
            Self(std::env::temp_dir().join(format!("agentix-qwen-{nonce}.gguf")))
        }
    }

    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    fn write_test_shard(
        file: &TempFile,
        tensor_name: &str,
        split_index: u16,
        split_count: u16,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let name = CString::new(tensor_name)?;
        let architecture_key = CString::new("general.architecture")?;
        let architecture_value = CString::new("qwen4exp")?;
        let split_no_key = CString::new("split.no")?;
        let split_count_key = CString::new("split.count")?;
        let path = CString::new(file.0.to_str().ok_or("temp path is not valid UTF-8")?)?;
        // SAFETY: this context requests an owned arena large enough for one F32 tensor.
        let tensor_context = unsafe {
            ffi::ggml_init(ffi::ggml_init_params {
                mem_size: 1024 * 1024,
                mem_buffer: ptr::null_mut(),
                no_alloc: false,
            })
        };
        let tensor_context =
            NonNull::new(tensor_context).ok_or("GGML test context allocation failed")?;
        // SAFETY: the live context and positive F32 extent are valid.
        let tensor =
            unsafe { ffi::ggml_new_tensor_1d(tensor_context.as_ptr(), ffi::GGML_TYPE_F32, 4) };
        let tensor = NonNull::new(tensor).ok_or("GGML test tensor allocation failed")?;
        // SAFETY: tensor is owned by the live context and name is NUL terminated.
        unsafe { ffi::ggml_set_name(tensor.as_ptr(), name.as_ptr()) };

        // SAFETY: the returned GGUF context is uniquely owned and freed below.
        let context = unsafe { ffi::gguf_init_empty() };
        let context = NonNull::new(context).ok_or("GGUF test context allocation failed")?;
        // SAFETY: metadata pointers are NUL terminated and tensor/data remain live through write.
        unsafe {
            ffi::gguf_set_val_str(
                context.as_ptr(),
                architecture_key.as_ptr(),
                architecture_value.as_ptr(),
            );
            ffi::gguf_set_val_u16(context.as_ptr(), split_no_key.as_ptr(), split_index);
            ffi::gguf_set_val_u16(context.as_ptr(), split_count_key.as_ptr(), split_count);
            ffi::gguf_add_tensor(context.as_ptr(), tensor.as_ptr());
            ffi::gguf_set_tensor_data(
                context.as_ptr(),
                name.as_ptr(),
                ffi::ggml_get_data(tensor.as_ptr()),
            );
        }
        // SAFETY: context and path are valid; false requests a full file write.
        let written = unsafe { ffi::gguf_write_to_file(context.as_ptr(), path.as_ptr(), false) };
        // SAFETY: this test uniquely owns both native contexts.
        unsafe {
            ffi::gguf_free(context.as_ptr());
            ffi::ggml_free(tensor_context.as_ptr());
        }
        assert!(written);
        Ok(())
    }

    fn write_test_f32_vector(
        file: &TempFile,
        tensor_name: &str,
        values: &[f32],
    ) -> Result<(), Box<dyn std::error::Error>> {
        let name = CString::new(tensor_name)?;
        let architecture_key = CString::new("general.architecture")?;
        let architecture_value = CString::new("qwen4exp")?;
        let path = CString::new(file.0.to_str().ok_or("temp path is not valid UTF-8")?)?;
        let length = i64::try_from(values.len())?;
        let tensor_context = unsafe {
            ffi::ggml_init(ffi::ggml_init_params {
                mem_size: 1024 * 1024,
                mem_buffer: ptr::null_mut(),
                no_alloc: false,
            })
        };
        let tensor_context =
            NonNull::new(tensor_context).ok_or("GGML test context allocation failed")?;
        let tensor =
            unsafe { ffi::ggml_new_tensor_1d(tensor_context.as_ptr(), ffi::GGML_TYPE_F32, length) };
        let tensor = NonNull::new(tensor).ok_or("GGML test tensor allocation failed")?;
        // SAFETY: tensor is an allocated F32 vector with exactly values.len() elements.
        unsafe {
            ptr::copy_nonoverlapping(
                values.as_ptr(),
                ffi::ggml_get_data(tensor.as_ptr()).cast::<f32>(),
                values.len(),
            );
            ffi::ggml_set_name(tensor.as_ptr(), name.as_ptr());
        }
        let context = unsafe { ffi::gguf_init_empty() };
        let context = NonNull::new(context).ok_or("GGUF test context allocation failed")?;
        // SAFETY: the key/value and tensor/data remain live through serialization.
        unsafe {
            ffi::gguf_set_val_str(
                context.as_ptr(),
                architecture_key.as_ptr(),
                architecture_value.as_ptr(),
            );
            ffi::gguf_add_tensor(context.as_ptr(), tensor.as_ptr());
            ffi::gguf_set_tensor_data(
                context.as_ptr(),
                name.as_ptr(),
                ffi::ggml_get_data(tensor.as_ptr()),
            );
        }
        let written = unsafe { ffi::gguf_write_to_file(context.as_ptr(), path.as_ptr(), false) };
        // SAFETY: this test uniquely owns both native contexts.
        unsafe {
            ffi::gguf_free(context.as_ptr());
            ffi::ggml_free(tensor_context.as_ptr());
        }
        assert!(written);
        Ok(())
    }

    fn decode_hex_bytes(hex: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        if !hex.len().is_multiple_of(2) {
            return Err("sample payload hex has odd length".into());
        }
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(hex.len() / 2)?;
        for pair in hex.as_bytes().chunks_exact(2) {
            let pair = std::str::from_utf8(pair)?;
            bytes.push(u8::from_str_radix(pair, 16)?);
        }
        Ok(bytes)
    }

    #[test]
    fn sampled_published_norm_vector_matches_direct_source_transform(
    ) -> Result<(), Box<dyn std::error::Error>> {
        const SOURCE_HEX: &str = "823f783f7a3f823f763f7e3f673f733f633f6a3f6e3f7e3f6c3f6e3f6a3f783f783f6b3f7e3f713f833f7b3f7f3f803f7b3f693f7f3f7c3f793f6e3f793f6d3f803f7c3f803f763f603f7b3f7b3f803f7d3f753f613f623f7a3f713f7d3f803f803f823f813f7e3f7b3f7a3f6d3f7c3f723f7c3f773f763f7f3f6b3f743f7c3f7d3f733f763f6d3f773f7a3f7e3f7b3f7e3f733f803f7b3f713f723f763f673f803f833f753f773f783f803f793f773f733f803f783f7d3f7e3f7d3f6e3f6f3f7c3f753f743f7d3f713f783f783f7a3f7b3f7a3f7a3f7b3f723f643f7d3f763f733f673f6b3f833f813f823f7f3f773f7e3f723f793f7b3f723f7c3f7d3f7f3f";
        const TARGET_HEX: &str = "0000823f0000783f00007a3f0000823f0000763f00007e3f0000673f0000733f0000633f00006a3f00006e3f00007e3f00006c3f00006e3f00006a3f0000783f0000783f00006b3f00007e3f0000713f0000833f00007b3f00007f3f0000803f00007b3f0000693f00007f3f00007c3f0000793f00006e3f0000793f00006d3f0000803f00007c3f0000803f0000763f0000603f00007b3f00007b3f0000803f00007d3f0000753f0000613f0000623f00007a3f0000713f00007d3f0000803f0000803f0000823f0000813f00007e3f00007b3f00007a3f00006d3f00007c3f0000723f00007c3f0000773f0000763f00007f3f00006b3f0000743f00007c3f00007d3f0000733f0000763f00006d3f0000773f00007a3f00007e3f00007b3f00007e3f0000733f0000803f00007b3f0000713f0000723f0000763f0000673f0000803f0000833f0000753f0000773f0000783f0000803f0000793f0000773f0000733f0000803f0000783f00007d3f00007e3f00007d3f00006e3f00006f3f00007c3f0000753f0000743f00007d3f0000713f0000783f0000783f00007a3f00007b3f00007a3f00007a3f00007b3f0000723f0000643f00007d3f0000763f0000733f0000673f00006b3f0000833f0000813f0000823f00007f3f0000773f00007e3f0000723f0000793f00007b3f0000723f00007c3f00007d3f00007f3f";
        let source_bytes = decode_hex_bytes(SOURCE_HEX)?;
        let target_bytes = decode_hex_bytes(TARGET_HEX)?;
        const SOURCE_NAME: &str = "model.language_model.layers.0.linear_attn.norm.weight";
        const TARGET_NAME: &str = "blk.0.ssm_norm.weight";
        assert_eq!(source_bytes.len(), 128 * 2);
        assert_eq!(target_bytes.len(), 128 * 4);

        let source_file = TempFile::new();
        let mut header = format!(
            "{{\"{SOURCE_NAME}\":{{\"dtype\":\"BF16\",\"shape\":[128],\"data_offsets\":[0,{}]}}}}",
            source_bytes.len()
        )
        .into_bytes();
        while header.len() % 8 != 0 {
            header.push(b' ');
        }
        let mut output = File::create(&source_file.0)?;
        output.write_all(&u64::try_from(header.len())?.to_le_bytes())?;
        output.write_all(&header)?;
        output.write_all(&source_bytes)?;
        drop(output);
        let source = agentix_qwen::rows::SafeTensorFile::open(File::open(&source_file.0)?)?;
        let source_values = source.float_vector(SOURCE_NAME, 128)?;
        let config = agentix_qwen::config::QwenTextConfig::parse(include_bytes!(
            "../../agentix-qwen/tests/fixtures/qwen3_8_flash_next_config.json"
        ))?;
        let converted = agentix_qwen::reference::source::transform_qwen4exp_f32_tensor(
            SOURCE_NAME,
            &[128],
            &source_values,
            &config,
        )?;
        assert_eq!(converted.len(), 1);
        assert_eq!(converted[0].name, TARGET_NAME);
        assert_eq!(converted[0].dimensions, [128]);

        let target_values = target_bytes
            .chunks_exact(4)
            .map(|bytes| f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
            .collect::<Vec<_>>();
        let target_file = TempFile::new();
        write_test_f32_vector(&target_file, TARGET_NAME, &target_values)?;
        let target_rows = DenseRows::open(&target_file.0, TARGET_NAME, 128)?;
        assert_eq!(target_rows.row_count(), 1);
        assert_eq!(target_rows.read_row(0)?, converted[0].values);
        Ok(())
    }

    #[test]
    fn sampled_published_alog_vector_matches_gated_delta_reorder(
    ) -> Result<(), Box<dyn std::error::Error>> {
        const SOURCE_HEX: &str = "0d408340b7bf8b405a3f304053401a409cbd55c04fbf2f4099bfb6bf44401fc09d40744065c01bc097404740a2400e3b2440834024403140bb3f28c0fabf4f40c03f7940294072bf864021408a4048c08a40a0405640f63d6c3f2d40d4bf9b3f";
        const TARGET_HEX: &str = "28da10c1d1fd99c26f39d8c13be212bd9eef9abee7c1aabd4ac9e4bc9c41b3c1bc7d4fc136397ec1ed3b11be0fc443c244b783c237f733bde799e2c124d26ec17bdb6fc2b7f815c0027a31c17e16e4be2f0d77beb82107c335c5b5bd28fc1dc37bdb6fc281eb89c0bd1fcbc1f85960c14cfd45c1ef4095c2265690bf0f6f43be012175be38487ac163396dbfdf6676c12c0cabc1c70d35c2670ee0c2144780bfbc7d4fc14c5b94bdff698fc017f1c6beef4095c2c56914c35fe520c007d356c0";
        let source_bytes = decode_hex_bytes(SOURCE_HEX)?;
        let target_bytes = decode_hex_bytes(TARGET_HEX)?;
        const SOURCE_NAME: &str = "model.language_model.layers.0.linear_attn.A_log";
        const TARGET_NAME: &str = "blk.0.ssm_a";
        assert_eq!(source_bytes.len(), 48 * 2);
        assert_eq!(target_bytes.len(), 48 * 4);

        let source_file = TempFile::new();
        let mut header = format!(
            "{{\"{SOURCE_NAME}\":{{\"dtype\":\"BF16\",\"shape\":[48],\"data_offsets\":[0,{}]}}}}",
            source_bytes.len()
        )
        .into_bytes();
        while header.len() % 8 != 0 {
            header.push(b' ');
        }
        let mut output = File::create(&source_file.0)?;
        output.write_all(&u64::try_from(header.len())?.to_le_bytes())?;
        output.write_all(&header)?;
        output.write_all(&source_bytes)?;
        drop(output);
        let source = agentix_qwen::rows::SafeTensorFile::open(File::open(&source_file.0)?)?;
        let source_values = source.float_vector(SOURCE_NAME, 48)?;
        let config = agentix_qwen::config::QwenTextConfig::parse(include_bytes!(
            "../../agentix-qwen/tests/fixtures/qwen3_8_flash_next_config.json"
        ))?;
        let converted = agentix_qwen::reference::source::transform_qwen4exp_f32_tensor(
            SOURCE_NAME,
            &[48],
            &source_values,
            &config,
        )?;
        assert_eq!(converted.len(), 1);
        assert_eq!(converted[0].name, TARGET_NAME);
        assert_eq!(converted[0].dimensions, [48]);

        let target_values = target_bytes
            .chunks_exact(4)
            .map(|bytes| f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
            .collect::<Vec<_>>();
        let target_file = TempFile::new();
        write_test_f32_vector(&target_file, TARGET_NAME, &target_values)?;
        let target_rows = DenseRows::open(&target_file.0, TARGET_NAME, 48)?;
        assert_eq!(target_rows.row_count(), 1);
        let actual_values = target_rows.read_row(0)?;
        for (actual, expected) in actual_values.iter().zip(&converted[0].values) {
            let tolerance = 1.0e-6_f32 * expected.abs().max(1.0);
            assert!(
                (actual - expected).abs() <= tolerance,
                "A_log parity mismatch: actual={actual}, expected={expected}, tolerance={tolerance}"
            );
        }
        Ok(())
    }

    #[test]
    fn sampled_published_dt_bias_vector_matches_gated_delta_reorder(
    ) -> Result<(), Box<dyn std::error::Error>> {
        const SOURCE_HEX: &str = "26c081c0e4bedfbdc9c0c0c06bc00fc0dbbfb4c0c6bf20c00fbf224012c0e3c0ecbf1fc0e2c0abc09c3e86c07dbfa4c01bc06ec01ec069c0babffac0e7c09ec0c2c06fbf0fbf00c131c024c01ac0e7c09abfc3bf8ebf85bf13bb5fc0f9c0d7c0";
        const TARGET_HEX: &str = "000026c00000dfbd00006bc00000b4c000000fbf0000e3c00000e2c0000086c000001bc0000069c00000e7c000006fbf000031c00000e7c000008ebf00005fc0000081c00000c9c000000fc00000c6bf000022400000ecbf0000abc000007dbf00006ec00000babf00009ec000000fbf000024c000009abf000085bf0000f9c00000e4be0000c0c00000dbbf000020c0000012c000001fc000009c3e0000a4c000001ec00000fac00000c2c0000000c100001ac00000c3bf000013bb0000d7c0";
        const SOURCE_NAME: &str = "model.language_model.layers.0.linear_attn.dt_bias";
        const TARGET_NAME: &str = "blk.0.ssm_dt.bias";
        let source_bytes = decode_hex_bytes(SOURCE_HEX)?;
        let target_bytes = decode_hex_bytes(TARGET_HEX)?;
        assert_eq!(source_bytes.len(), 48 * 2);
        assert_eq!(target_bytes.len(), 48 * 4);

        let source_file = TempFile::new();
        let mut header = format!(
            "{{\"{SOURCE_NAME}\":{{\"dtype\":\"BF16\",\"shape\":[48],\"data_offsets\":[0,{}]}}}}",
            source_bytes.len()
        )
        .into_bytes();
        while header.len() % 8 != 0 {
            header.push(b' ');
        }
        let mut output = File::create(&source_file.0)?;
        output.write_all(&u64::try_from(header.len())?.to_le_bytes())?;
        output.write_all(&header)?;
        output.write_all(&source_bytes)?;
        drop(output);
        let source = agentix_qwen::rows::SafeTensorFile::open(File::open(&source_file.0)?)?;
        let source_values = source.float_vector(SOURCE_NAME, 48)?;
        let config = agentix_qwen::config::QwenTextConfig::parse(include_bytes!(
            "../../agentix-qwen/tests/fixtures/qwen3_8_flash_next_config.json"
        ))?;
        let converted = agentix_qwen::reference::source::transform_qwen4exp_f32_tensor(
            SOURCE_NAME,
            &[48],
            &source_values,
            &config,
        )?;
        assert_eq!(converted.len(), 1);
        assert_eq!(converted[0].name, TARGET_NAME);
        assert_eq!(converted[0].dimensions, [48]);

        let target_values = target_bytes
            .chunks_exact(4)
            .map(|bytes| f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
            .collect::<Vec<_>>();
        let target_file = TempFile::new();
        write_test_f32_vector(&target_file, TARGET_NAME, &target_values)?;
        let target_rows = DenseRows::open(&target_file.0, TARGET_NAME, 48)?;
        assert_eq!(target_rows.row_count(), 1);
        assert_eq!(target_rows.read_row(0)?, converted[0].values);
        Ok(())
    }

    #[test]
    fn rewrites_qwen_ple_uint64_metadata_and_streams_tensor_payload(
    ) -> Result<(), Box<dyn std::error::Error>> {
        use agentix_qwen::reference::source::{
            ConvertedPleHashMetadata, PleHashMetadataKind as Kind,
        };

        let input = TempFile::new();
        let output = TempFile::new();
        write_test_shard(&input, "fixture.weight", 0, 1)?;
        let mut config = agentix_qwen::config::QwenTextConfig::parse(include_bytes!(
            "../../agentix-qwen/tests/fixtures/qwen3_8_flash_next_config.json"
        ))?;
        config.vocab_size = 8;
        config.eos_token_id = 7;
        config.ngram_size = 3;
        config.heads_per_ngram = 1;
        config.ngram_vocab_size_base = 20_000_000;
        let ngram = agentix_qwen::ngram::Ngram::new(agentix_qwen::ngram::Config {
            vocabulary: u32::try_from(config.vocab_size)?,
            eos: config.eos_token_id,
            size: config.ngram_size,
            heads_per_size: config.heads_per_ngram,
            vocabulary_base: u64::try_from(config.ngram_vocab_size_base)?,
            layer_ordinal: 0,
            seed: config.ngram_seed,
        })?;
        let as_i64 = |values: &[u64]| -> Result<Vec<i64>, Error> {
            values
                .iter()
                .map(|value| {
                    i64::try_from(*value).map_err(|_| Error::Invalid("fixture value overflow"))
                })
                .collect()
        };
        let arrays = vec![
            ConvertedPleHashMetadata {
                kind: Kind::LayerMultipliers,
                values: as_i64(ngram.hash_multipliers())?,
            },
            ConvertedPleHashMetadata {
                kind: Kind::HeadOffsets,
                values: as_i64(ngram.head_offsets())?,
            },
            ConvertedPleHashMetadata {
                kind: Kind::HeadVocabSizes,
                values: as_i64(ngram.head_vocab_sizes())?,
            },
        ];

        rewrite_qwen4exp_gguf_ple_hash_metadata(&input.0, &output.0, &config, &arrays)?;
        let original = inspect_gguf(&input.0)?;
        let rewritten = inspect_gguf(&output.0)?;
        assert_eq!(rewritten.architecture.as_deref(), Some("qwen4exp"));
        assert_eq!(rewritten.tensors, original.tensors);
        let multiplier_length = u64::try_from(arrays[0].values.len())?;
        assert!(rewritten.metadata.iter().any(|item| {
            item.key == "qwen4exp.ple.layer_multipliers"
                && item.value
                    == GgufMetadataValue::Array {
                        element_type: ffi::GGUF_TYPE_UINT64 as i32,
                        length: multiplier_length,
                    }
        }));
        let input_bytes = std::fs::read(&input.0)?;
        let output_bytes = std::fs::read(&output.0)?;
        let input_start = usize::try_from(original.data_offset)?;
        let output_start = usize::try_from(rewritten.data_offset)?;
        assert_eq!(&output_bytes[output_start..], &input_bytes[input_start..]);

        let unchanged = output_bytes.clone();
        assert!(
            rewrite_qwen4exp_gguf_ple_hash_metadata(&input.0, &output.0, &config, &arrays).is_err()
        );
        assert_eq!(std::fs::read(&output.0)?, unchanged);
        let invalid_output = TempFile::new();
        let mut invalid_arrays = arrays.clone();
        let offsets = invalid_arrays
            .iter_mut()
            .find(|array| array.kind == Kind::HeadOffsets)
            .ok_or("fixture offsets missing")?;
        offsets.values[1] += 1;
        assert!(rewrite_qwen4exp_gguf_ple_hash_metadata(
            &input.0,
            &invalid_output.0,
            &config,
            &invalid_arrays,
        )
        .is_err());
        assert!(!invalid_output.0.exists());
        Ok(())
    }

    fn write_quant_test_tensor(
        file: &TempFile,
        kind: ffi::ggml_type,
        width: usize,
        rows_per_group: u64,
        groups: u64,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let name = CString::new("per_layer_token_embd.weight")?;
        let path = CString::new(file.0.to_str().ok_or("temp path is not valid UTF-8")?)?;
        // SAFETY: this context requests an owned arena large enough for one quantized block.
        let tensor_context = unsafe {
            ffi::ggml_init(ffi::ggml_init_params {
                mem_size: 1024 * 1024,
                mem_buffer: ptr::null_mut(),
                no_alloc: false,
            })
        };
        let tensor_context =
            NonNull::new(tensor_context).ok_or("GGML test context allocation failed")?;
        // SAFETY: the caller supplies a supported GGML type valid for 32 values.
        let width_i64 = i64::try_from(width)?;
        let rows_i64 = i64::try_from(rows_per_group)?;
        let groups_i64 = i64::try_from(groups)?;
        let tensor = unsafe {
            ffi::ggml_new_tensor_3d(
                tensor_context.as_ptr(),
                kind,
                width_i64,
                rows_i64,
                groups_i64,
            )
        };
        let tensor = NonNull::new(tensor).ok_or("GGML test tensor allocation failed")?;
        let byte_count = usize::try_from(tensor_byte_size(
            kind as i32,
            [u64::try_from(width)?, rows_per_group, groups, 1],
        )?)?;
        let mut block = vec![0_u8; byte_count];
        if kind == ffi::GGML_TYPE_Q8_0 {
            for chunk in block.chunks_exact_mut(Q8_0_BLOCK_BYTES) {
                chunk[..2].copy_from_slice(&0x3c00_u16.to_le_bytes());
                chunk[2..].fill(1);
            }
        } else if matches!(kind, ffi::GGML_TYPE_Q4_0 | ffi::GGML_TYPE_IQ4_NL) {
            for chunk in block.chunks_exact_mut(Q4_0_BLOCK_BYTES) {
                chunk[..2].copy_from_slice(&0x3c00_u16.to_le_bytes());
                chunk[2..].fill(0xf0);
            }
        } else {
            // A unit block scale makes the native codebook path observable.
            // SAFETY: the test uses a type ID from the pinned GGML enum.
            let stride = unsafe { ffi::ggml_type_size(kind) };
            for chunk in block.chunks_exact_mut(stride) {
                chunk[..2].copy_from_slice(&0x3c00_u16.to_le_bytes());
            }
        }
        // SAFETY: the buffer length is the byte size reported by pinned GGML.
        unsafe {
            ptr::copy_nonoverlapping(
                block.as_ptr(),
                ffi::ggml_get_data(tensor.as_ptr()).cast::<u8>(),
                block.len(),
            );
            ffi::ggml_set_name(tensor.as_ptr(), name.as_ptr());
        }
        // SAFETY: the returned GGUF context is uniquely owned and freed below.
        let context = unsafe { ffi::gguf_init_empty() };
        let context = NonNull::new(context).ok_or("GGUF test context allocation failed")?;
        // SAFETY: tensor/data and name remain live through serialization.
        unsafe {
            ffi::gguf_add_tensor(context.as_ptr(), tensor.as_ptr());
            ffi::gguf_set_tensor_data(
                context.as_ptr(),
                name.as_ptr(),
                ffi::ggml_get_data(tensor.as_ptr()),
            );
        }
        // SAFETY: context and path are valid; false requests a complete file write.
        let written = unsafe { ffi::gguf_write_to_file(context.as_ptr(), path.as_ptr(), false) };
        // SAFETY: this test uniquely owns both native contexts.
        unsafe {
            ffi::gguf_free(context.as_ptr());
            ffi::ggml_free(tensor_context.as_ptr());
        }
        assert!(written);
        Ok(())
    }

    fn write_dense_test_tensor(
        file: &TempFile,
        kind: ffi::ggml_type,
        raw_values: &[u8],
        value_count: usize,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let name = CString::new("fixture.weight")?;
        let path = CString::new(file.0.to_str().ok_or("temp path is not UTF-8")?)?;
        let value_count = i64::try_from(value_count)?;
        // SAFETY: kind is a valid dense GGML type supplied by the test cases.
        let type_size = unsafe { ffi::ggml_type_size(kind) };
        let expected_bytes = usize::try_from(value_count)?
            .checked_mul(type_size)
            .ok_or("dense fixture byte size overflow")?;
        if raw_values.len() != expected_bytes {
            return Err("dense fixture byte length does not match tensor type".into());
        }
        let tensor_context = unsafe {
            ffi::ggml_init(ffi::ggml_init_params {
                mem_size: 1024 * 1024,
                mem_buffer: ptr::null_mut(),
                no_alloc: false,
            })
        };
        let tensor_context =
            NonNull::new(tensor_context).ok_or("GGML test context allocation failed")?;
        let tensor = unsafe { ffi::ggml_new_tensor_1d(tensor_context.as_ptr(), kind, value_count) };
        let tensor = NonNull::new(tensor).ok_or("GGML test tensor allocation failed")?;
        // SAFETY: raw_values length is checked against the tensor's encoded byte size by caller.
        unsafe {
            ptr::copy_nonoverlapping(
                raw_values.as_ptr(),
                ffi::ggml_get_data(tensor.as_ptr()).cast::<u8>(),
                raw_values.len(),
            );
            ffi::ggml_set_name(tensor.as_ptr(), name.as_ptr());
        }
        let context = unsafe { ffi::gguf_init_empty() };
        let context = NonNull::new(context).ok_or("GGUF empty context allocation failed")?;
        unsafe {
            ffi::gguf_add_tensor(context.as_ptr(), tensor.as_ptr());
            ffi::gguf_set_tensor_data(
                context.as_ptr(),
                name.as_ptr(),
                ffi::ggml_get_data(tensor.as_ptr()),
            );
        }
        let written = unsafe { ffi::gguf_write_to_file(context.as_ptr(), path.as_ptr(), false) };
        unsafe {
            ffi::gguf_free(context.as_ptr());
            ffi::ggml_free(tensor_context.as_ptr());
        }
        assert!(written);
        Ok(())
    }

    #[test]
    fn reads_only_the_requested_q4_0_gguf_row() -> Result<(), Box<dyn std::error::Error>> {
        let file = TempFile::new();
        write_quant_test_tensor(&file, ffi::GGML_TYPE_Q4_0, 32, 1, 1)?;
        let rows = Q4_0Rows::open(&file.0, "per_layer_token_embd.weight", 32)?;
        assert!(Q4_0Rows::open(&file.0, "per_layer_token_embd.weight", 16).is_err());
        assert!(Q4_0Rows::open(&file.0, "missing.weight", 32).is_err());
        assert_eq!(rows.row_count(), 1);
        assert_eq!(rows.row_width(), 32);
        let decoded = rows.read_row(0)?;
        assert_eq!(&decoded[..16], &[-8.0; 16]);
        assert_eq!(&decoded[16..], &[7.0; 16]);
        let mut output = [0.0; 32];
        rows.read_row_into(0, &mut output)?;
        assert_eq!(output.as_slice(), decoded.as_slice());
        assert!(rows.read_row_into(0, &mut [0.0; 16]).is_err());
        assert!(rows.read_row(1).is_err());
        Ok(())
    }

    #[test]
    fn reads_only_the_requested_q8_0_gguf_row() -> Result<(), Box<dyn std::error::Error>> {
        let file = TempFile::new();
        write_quant_test_tensor(&file, ffi::GGML_TYPE_Q8_0, 32, 1, 1)?;
        let rows = QuantizedRows::open(&file.0, "per_layer_token_embd.weight", 32)?;
        assert_eq!(rows.row_count(), 1);
        assert_eq!(rows.read_row(0)?, vec![1.0; 32]);
        assert!(rows.read_row(1).is_err());

        let info = inspect_gguf(&file.0)?;
        let model_rows = GgufRows::open_from_info(&file.0, &info, &info.tensors[0])?;
        assert_eq!(model_rows.read_row(0)?, vec![1.0; 32]);
        let streamed = agentix_qwen::reference::linear_from_rows(
            &[1.0; 32],
            &mut { GgufRows::open_from_info(&file.0, &info, &info.tensors[0])? },
            None,
        )?;
        assert_eq!(streamed, vec![32.0]);
        Ok(())
    }

    #[test]
    fn expert_group_matrix_view_reads_only_its_contiguous_rows(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let file = TempFile::new();
        write_quant_test_tensor(&file, ffi::GGML_TYPE_Q8_0, 32, 2, 3)?;
        let rows = GgufRows::Quantized(QuantizedRows::open(
            &file.0,
            "per_layer_token_embd.weight",
            32,
        )?);
        let mut group = GgufMatrixRows {
            rows,
            first_row: 2,
            output_features: 2,
            input_features: 32,
        };
        let output = agentix_qwen::reference::linear_from_rows(&[1.0; 32], &mut group, None)?;
        assert_eq!(output, vec![32.0; 2]);
        assert!(agentix_qwen::reference::LinearWeightRows::read_weight_row(
            &mut group,
            2,
            &mut [0.0; 32]
        )
        .is_err());
        Ok(())
    }

    #[test]
    fn qwen_tensor_store_rejects_schema_incomplete_gguf_set(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let file = TempFile::new();
        write_quant_test_tensor(&file, ffi::GGML_TYPE_Q8_0, 32, 1, 1)?;
        let config = agentix_qwen::config::QwenTextConfig::parse(include_bytes!(
            "../../agentix-qwen/tests/fixtures/qwen3_8_flash_next_config.json"
        ))?;
        assert!(QwenGgufTensorStore::open(std::slice::from_ref(&file.0), &config).is_err());
        Ok(())
    }

    #[test]
    fn reads_candidate_quantized_rows_through_pinned_ggml_callbacks(
    ) -> Result<(), Box<dyn std::error::Error>> {
        for (kind, width) in [
            (ffi::GGML_TYPE_IQ2_XS, K_BLOCK_VALUES),
            (ffi::GGML_TYPE_IQ3_S, K_BLOCK_VALUES),
            (ffi::GGML_TYPE_IQ2_S, K_BLOCK_VALUES),
            (ffi::GGML_TYPE_IQ3_XXS, K_BLOCK_VALUES),
            (ffi::GGML_TYPE_IQ4_XS, K_BLOCK_VALUES),
            (ffi::GGML_TYPE_Q2_0, K_BLOCK_VALUES),
            (ffi::GGML_TYPE_Q4_K, K_BLOCK_VALUES),
            (ffi::GGML_TYPE_Q5_K, K_BLOCK_VALUES),
            (ffi::GGML_TYPE_Q6_K, K_BLOCK_VALUES),
        ] {
            let file = TempFile::new();
            write_quant_test_tensor(&file, kind, width, 1, 1)?;
            let rows = QuantizedRows::open(&file.0, "per_layer_token_embd.weight", width)?;
            assert_eq!(rows.row_count(), 1);
            let decoded = rows
                .read_row(0)
                .map_err(|error| format!("GGML type {kind}: {error}"))?;
            assert_eq!(decoded.len(), width);
            assert!(decoded.iter().all(|value| value.is_finite()));
            let block_bytes = usize::try_from(tensor_byte_size(
                kind as i32,
                [u64::try_from(width)?, 1, 1, 1],
            )?)?;
            // SAFETY: the test uses a type ID from the pinned GGML enum.
            let block = unsafe { ffi::ggml_type_size(kind) };
            let mut encoded = vec![0_u8; block_bytes];
            for chunk in encoded.chunks_exact_mut(block) {
                chunk[..2].copy_from_slice(&0x3c00_u16.to_le_bytes());
            }
            assert_eq!(decoded, native_dequantize(kind as i32, &encoded, width)?);
            assert!(rows.read_row(1).is_err());
        }
        assert!(
            QuantizedRows::open(&TempFile::new().0, "per_layer_token_embd.weight", 255,).is_err()
        );
        Ok(())
    }

    #[test]
    fn flattens_rank_three_quantized_expert_rows_with_group_geometry(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let file = TempFile::new();
        write_quant_test_tensor(&file, ffi::GGML_TYPE_IQ3_S, K_BLOCK_VALUES, 2, 3)?;
        let rows = QuantizedRows::open(&file.0, "per_layer_token_embd.weight", K_BLOCK_VALUES)?;
        assert_eq!(rows.row_count(), 6);
        assert_eq!(rows.rows_per_group(), 2);
        assert_eq!(rows.group_count(), 3);
        for row in 0..rows.row_count() {
            assert!(rows.read_row(row)?.iter().all(|value| value.is_finite()));
        }
        assert!(rows.read_row(6).is_err());
        Ok(())
    }

    #[test]
    fn reads_f32_f16_and_bf16_gguf_rows() -> Result<(), Box<dyn std::error::Error>> {
        let mut f32_bytes = Vec::new();
        f32_bytes.extend_from_slice(&1.25_f32.to_le_bytes());
        f32_bytes.extend_from_slice(&(-2.5_f32).to_le_bytes());
        let mut f16_bytes = Vec::new();
        f16_bytes.extend_from_slice(&0x3c00_u16.to_le_bytes());
        f16_bytes.extend_from_slice(&0xc000_u16.to_le_bytes());
        let mut bf16_bytes = Vec::new();
        bf16_bytes.extend_from_slice(&0x3f80_u16.to_le_bytes());
        bf16_bytes.extend_from_slice(&0xc000_u16.to_le_bytes());
        let cases = [
            (ffi::GGML_TYPE_F32, f32_bytes, vec![1.25, -2.5]),
            (ffi::GGML_TYPE_F16, f16_bytes, vec![1.0, -2.0]),
            (ffi::GGML_TYPE_BF16, bf16_bytes, vec![1.0, -2.0]),
        ];
        for (kind, raw_values, expected) in cases {
            let file = TempFile::new();
            write_dense_test_tensor(&file, kind, &raw_values, expected.len())?;
            let rows = DenseRows::open(&file.0, "fixture.weight", expected.len())?;
            assert_eq!(rows.row_count(), 1);
            let decoded = rows.read_row(0)?;
            assert_eq!(decoded, expected);
            if kind != ffi::GGML_TYPE_F32 {
                let native = native_dequantize(kind as i32, &raw_values, decoded.len())?;
                for (actual, reference) in decoded.iter().zip(native) {
                    assert!((actual - reference).abs() <= 1.0e-7);
                }
            }
            assert!(rows.read_row(1).is_err());
        }
        Ok(())
    }

    #[test]
    fn dense_rows_reject_non_finite_values_and_wrong_geometry(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let invalid_file = TempFile::new();
        write_dense_test_tensor(
            &invalid_file,
            ffi::GGML_TYPE_F32,
            &f32::NAN.to_le_bytes(),
            1,
        )?;
        let invalid = DenseRows::open(&invalid_file.0, "fixture.weight", 1)?;
        assert!(invalid.read_row(0).is_err());
        assert!(DenseRows::open(&invalid_file.0, "fixture.weight", 2).is_err());
        Ok(())
    }

    #[test]
    fn opens_iq4_nl_tensor_and_decodes_requested_row() -> Result<(), Box<dyn std::error::Error>> {
        let file = TempFile::new();
        write_quant_test_tensor(&file, ffi::GGML_TYPE_IQ4_NL, 32, 1, 1)?;
        let mut rows = QuantizedRows::open(&file.0, "per_layer_token_embd.weight", 32)?;
        assert_eq!(rows.row_count(), 1);
        assert_eq!(
            rows.read_row(0)?,
            vec![-127.0; 16]
                .into_iter()
                .chain(vec![113.0; 16])
                .collect::<Vec<_>>()
        );
        let mut output = [0.0; 32];
        agentix_qwen::ple::EmbeddingRowReader::read_row(&mut rows, 0, &mut output)?;
        assert_eq!(output[0], -127.0);
        assert_eq!(output[31], 113.0);
        Ok(())
    }

    #[test]
    fn inspects_native_gguf_tensor() -> Result<(), Box<dyn std::error::Error>> {
        let file = TempFile::new();
        // SAFETY: this context requests an owned arena large enough for one F32 tensor.
        let tensor_context = unsafe {
            ffi::ggml_init(ffi::ggml_init_params {
                mem_size: 1024 * 1024,
                mem_buffer: ptr::null_mut(),
                no_alloc: false,
            })
        };
        let tensor_context =
            NonNull::new(tensor_context).ok_or("GGML test context allocation failed")?;
        // SAFETY: the context is live; F32 and extent 4 are valid tensor parameters.
        let tensor =
            unsafe { ffi::ggml_new_tensor_1d(tensor_context.as_ptr(), ffi::GGML_TYPE_F32, 4) };
        let tensor = NonNull::new(tensor).ok_or("GGML test tensor allocation failed")?;
        let name = CString::new("fixture.weight")?;
        // SAFETY: tensor is owned by the live context and name is NUL terminated.
        unsafe { ffi::ggml_set_name(tensor.as_ptr(), name.as_ptr()) };

        // SAFETY: the returned GGUF context is uniquely owned and freed after writing.
        let raw = unsafe { ffi::gguf_init_empty() };
        let context = NonNull::new(raw).ok_or("GGUF empty context allocation failed")?;
        let architecture_key = CString::new("general.architecture")?;
        let architecture_value = CString::new("qwen4exp")?;
        let block_count_key = CString::new("qwen4exp.block_count")?;
        let array_key = CString::new("qwen4exp.test_array")?;
        let array_values = [11_u32, 13_u32, 17_u32];
        // SAFETY: tensor metadata and backing data remain live until the write finishes.
        unsafe {
            ffi::gguf_set_val_str(
                context.as_ptr(),
                architecture_key.as_ptr(),
                architecture_value.as_ptr(),
            );
            ffi::gguf_set_val_u32(context.as_ptr(), block_count_key.as_ptr(), 48);
            ffi::gguf_set_arr_data(
                context.as_ptr(),
                array_key.as_ptr(),
                ffi::GGUF_TYPE_UINT32,
                array_values.as_ptr().cast(),
                array_values.len(),
            );
            ffi::gguf_add_tensor(context.as_ptr(), tensor.as_ptr());
            ffi::gguf_set_tensor_data(
                context.as_ptr(),
                name.as_ptr(),
                ffi::ggml_get_data(tensor.as_ptr()),
            );
        }
        let path = CString::new(file.0.to_str().ok_or("temp path is not UTF-8")?)?;
        // SAFETY: context and path are valid; false requests a complete file write.
        let written = unsafe { ffi::gguf_write_to_file(context.as_ptr(), path.as_ptr(), false) };
        // SAFETY: this test uniquely owns the context returned by gguf_init_empty.
        unsafe { ffi::gguf_free(context.as_ptr()) };
        // SAFETY: this test uniquely owns the context returned by ggml_init.
        unsafe { ffi::ggml_free(tensor_context.as_ptr()) };
        assert!(written);

        let info = inspect_gguf(&file.0)?;
        assert_eq!(info.version, 3);
        assert_eq!(info.tensors.len(), 1);
        assert_eq!(info.tensors[0].name, "fixture.weight");
        assert_eq!(info.tensors[0].type_name, "f32");
        assert_eq!(info.tensors[0].extents, [4, 1, 1, 1]);
        assert_eq!(info.tensors[0].byte_size, 16);
        assert_eq!(info.architecture.as_deref(), Some("qwen4exp"));
        assert!(info.metadata.iter().any(|entry| {
            entry.key == "qwen4exp.block_count" && entry.value == GgufMetadataValue::UInt32(48)
        }));
        assert!(info.metadata.iter().any(|entry| {
            entry.key == "qwen4exp.test_array"
                && entry.value
                    == GgufMetadataValue::Array {
                        element_type: ffi::GGUF_TYPE_UINT32 as i32,
                        length: 3,
                    }
        }));
        assert!(info.alignment.is_power_of_two());
        assert!(QuantizedRows::open(&file.0, "fixture.weight", 32).is_err());
        Ok(())
    }

    #[test]
    fn rejects_truncated_gguf() {
        let file = TempFile::new();
        assert!(std::fs::write(&file.0, b"GGUF").is_ok());
        assert!(matches!(inspect_gguf(&file.0), Err(Error::Invalid(_))));
    }

    #[test]
    fn matches_pinned_ggml_quant_block_geometry() -> Result<(), Error> {
        assert_eq!(
            tensor_byte_size(ffi::GGML_TYPE_IQ2_XS as i32, [256, 1, 1, 1])?,
            74
        );
        assert_eq!(
            tensor_byte_size(ffi::GGML_TYPE_IQ3_S as i32, [256, 1, 1, 1])?,
            110
        );
        assert!(tensor_byte_size(ffi::GGML_TYPE_IQ2_XS as i32, [255, 1, 1, 1]).is_err());
        Ok(())
    }

    #[test]
    fn validates_and_orders_split_gguf_set() -> Result<(), Box<dyn std::error::Error>> {
        let first = TempFile::new();
        let second = TempFile::new();
        write_test_shard(&first, "first.weight", 0, 2)?;
        write_test_shard(&second, "second.weight", 1, 2)?;
        let set = inspect_gguf_set(&[&second.0, &first.0])?;
        assert_eq!(set.split_count, 2);
        assert_eq!(set.total_tensors, 2);
        assert_eq!(set.shards[0].tensors[0].name, "first.weight");
        assert_eq!(set.shards[1].tensors[0].name, "second.weight");
        assert!(inspect_gguf_set(&[&first.0]).is_err());
        Ok(())
    }
}
