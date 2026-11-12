//! Physical GPU acceptance: NVRTC compilation, CUDA launch and result checking.
use std::ffi::{c_char, c_int, c_uint, c_void, CStr, CString};
use std::ptr;

#[link(name = "dl")]
extern "C" {
    fn dlopen(name: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
    fn dlerror() -> *const c_char;
}

fn check(code: c_int, operation: &str) -> Result<(), String> {
    if code == 0 {
        Ok(())
    } else {
        Err(format!("{operation} failed with status {code}"))
    }
}

fn main() -> Result<(), String> {
    // SAFETY: Each library stays loaded for the process lifetime. Symbol types
    // below match CUDA Driver/NVRTC C APIs. All input/output buffers remain live
    // and sized for their API calls; synchronization precedes host result reads.
    unsafe {
        let driver = dlopen(c"libcuda.so.1".as_ptr(), 2);
        let nvrtc = dlopen(c"libnvrtc.so.13".as_ptr(), 2);
        if driver.is_null() || nvrtc.is_null() {
            let error = dlerror();
            return Err(if error.is_null() {
                "missing CUDA driver/NVRTC 13 library".into()
            } else {
                CStr::from_ptr(error).to_string_lossy().into_owned()
            });
        }
        macro_rules! symbol {
            ($library:expr, $name:literal, $ty:ty) => {{
                let name = CString::new($name).map_err(|e| e.to_string())?;
                let pointer = dlsym($library, name.as_ptr());
                if pointer.is_null() {
                    return Err(format!("missing symbol {}", $name));
                }
                std::mem::transmute::<*mut c_void, $ty>(pointer)
            }};
        }
        let version = symbol!(
            nvrtc,
            "nvrtcVersion",
            unsafe extern "C" fn(*mut c_int, *mut c_int) -> c_int
        );
        let (mut major, mut minor) = (0, 0);
        check(version(&mut major, &mut minor), "nvrtcVersion")?;
        if major != 13 || minor != 1 {
            return Err(format!("expected NVRTC 13.1, found {major}.{minor}"));
        }
        let init = symbol!(driver, "cuInit", unsafe extern "C" fn(c_uint) -> c_int);
        check(init(0), "cuInit")?;
        let get_device = symbol!(
            driver,
            "cuDeviceGet",
            unsafe extern "C" fn(*mut c_int, c_int) -> c_int
        );
        let mut device = 0;
        check(get_device(&mut device, 0), "cuDeviceGet")?;
        let attribute = symbol!(
            driver,
            "cuDeviceGetAttribute",
            unsafe extern "C" fn(*mut c_int, c_int, c_int) -> c_int
        );
        let (mut cc_major, mut cc_minor) = (0, 0);
        check(
            attribute(&mut cc_major, 75, device),
            "compute capability major",
        )?;
        check(
            attribute(&mut cc_minor, 76, device),
            "compute capability minor",
        )?;
        let retain = symbol!(
            driver,
            "cuDevicePrimaryCtxRetain",
            unsafe extern "C" fn(*mut *mut c_void, c_int) -> c_int
        );
        let mut context = ptr::null_mut();
        check(retain(&mut context, device), "cuDevicePrimaryCtxRetain")?;
        let set_current = symbol!(
            driver,
            "cuCtxSetCurrent",
            unsafe extern "C" fn(*mut c_void) -> c_int
        );
        check(set_current(context), "cuCtxSetCurrent")?;
        let create = symbol!(
            nvrtc,
            "nvrtcCreateProgram",
            unsafe extern "C" fn(
                *mut *mut c_void,
                *const c_char,
                *const c_char,
                c_int,
                *const *const c_char,
                *const *const c_char,
            ) -> c_int
        );
        let source = c"extern \"C\" __global__ void answer(int* out) { out[0] = 6 * 7; }";
        let mut program = ptr::null_mut();
        check(
            create(
                &mut program,
                source.as_ptr(),
                c"smoke.cu".as_ptr(),
                0,
                ptr::null(),
                ptr::null(),
            ),
            "nvrtcCreateProgram",
        )?;
        let compile = symbol!(
            nvrtc,
            "nvrtcCompileProgram",
            unsafe extern "C" fn(*mut c_void, c_int, *const *const c_char) -> c_int
        );
        let arch = CString::new(format!("--gpu-architecture=compute_{cc_major}{cc_minor}"))
            .map_err(|e| e.to_string())?;
        let status = compile(program, 1, [arch.as_ptr()].as_ptr());
        if status != 0 {
            let log_size = symbol!(
                nvrtc,
                "nvrtcGetProgramLogSize",
                unsafe extern "C" fn(*mut c_void, *mut usize) -> c_int
            );
            let log = symbol!(
                nvrtc,
                "nvrtcGetProgramLog",
                unsafe extern "C" fn(*mut c_void, *mut c_char) -> c_int
            );
            let mut size = 0;
            check(log_size(program, &mut size), "NVRTC log size")?;
            let mut buffer = vec![0_u8; size];
            check(log(program, buffer.as_mut_ptr().cast()), "NVRTC log")?;
            return Err(format!(
                "nvrtcCompileProgram failed ({status}): {}",
                String::from_utf8_lossy(&buffer)
            ));
        }
        let ptx_size = symbol!(
            nvrtc,
            "nvrtcGetPTXSize",
            unsafe extern "C" fn(*mut c_void, *mut usize) -> c_int
        );
        let get_ptx = symbol!(
            nvrtc,
            "nvrtcGetPTX",
            unsafe extern "C" fn(*mut c_void, *mut c_char) -> c_int
        );
        let mut size = 0;
        check(ptx_size(program, &mut size), "nvrtcGetPTXSize")?;
        let mut ptx = vec![0_u8; size];
        check(get_ptx(program, ptx.as_mut_ptr().cast()), "nvrtcGetPTX")?;
        let load = symbol!(
            driver,
            "cuModuleLoadData",
            unsafe extern "C" fn(*mut *mut c_void, *const c_void) -> c_int
        );
        let mut module = ptr::null_mut();
        check(
            load(&mut module, ptx.as_ptr().cast()),
            "cuModuleLoadData (driver/PTX compatibility)",
        )?;
        let get_function = symbol!(
            driver,
            "cuModuleGetFunction",
            unsafe extern "C" fn(*mut *mut c_void, *mut c_void, *const c_char) -> c_int
        );
        let mut function = ptr::null_mut();
        check(
            get_function(&mut function, module, c"answer".as_ptr()),
            "cuModuleGetFunction",
        )?;
        let alloc = symbol!(
            driver,
            "cuMemAlloc_v2",
            unsafe extern "C" fn(*mut u64, usize) -> c_int
        );
        let mut allocation = 0_u64;
        check(alloc(&mut allocation, size_of::<i32>()), "cuMemAlloc")?;
        let launch = symbol!(
            driver,
            "cuLaunchKernel",
            unsafe extern "C" fn(
                *mut c_void,
                c_uint,
                c_uint,
                c_uint,
                c_uint,
                c_uint,
                c_uint,
                c_uint,
                *mut c_void,
                *mut *mut c_void,
                *mut *mut c_void,
            ) -> c_int
        );
        let mut parameters = [(&mut allocation as *mut u64).cast::<c_void>()];
        check(
            launch(
                function,
                1,
                1,
                1,
                1,
                1,
                1,
                0,
                ptr::null_mut(),
                parameters.as_mut_ptr(),
                ptr::null_mut(),
            ),
            "cuLaunchKernel",
        )?;
        let sync = symbol!(driver, "cuCtxSynchronize", unsafe extern "C" fn() -> c_int);
        check(sync(), "cuCtxSynchronize")?;
        let copy = symbol!(
            driver,
            "cuMemcpyDtoH_v2",
            unsafe extern "C" fn(*mut c_void, u64, usize) -> c_int
        );
        let mut answer = 0_i32;
        check(
            copy(
                (&mut answer as *mut i32).cast(),
                allocation,
                size_of::<i32>(),
            ),
            "cuMemcpyDtoH",
        )?;
        if answer != 42 {
            return Err(format!(
                "incorrect compute result: expected 42, got {answer}"
            ));
        }
        let free = symbol!(driver, "cuMemFree_v2", unsafe extern "C" fn(u64) -> c_int);
        check(free(allocation), "cuMemFree")?;
        let unload = symbol!(
            driver,
            "cuModuleUnload",
            unsafe extern "C" fn(*mut c_void) -> c_int
        );
        check(unload(module), "cuModuleUnload")?;
        let destroy = symbol!(
            nvrtc,
            "nvrtcDestroyProgram",
            unsafe extern "C" fn(*mut *mut c_void) -> c_int
        );
        check(destroy(&mut program), "nvrtcDestroyProgram")?;
        let release = symbol!(
            driver,
            "cuDevicePrimaryCtxRelease_v2",
            unsafe extern "C" fn(c_int) -> c_int
        );
        check(release(device), "cuDevicePrimaryCtxRelease")?;
        println!(
            "PASS: NVRTC {major}.{minor}, compute_{cc_major}{cc_minor}, kernel result {answer}"
        );
    }
    Ok(())
}
