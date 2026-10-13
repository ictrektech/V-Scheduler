//! Driver telemetry fallback for devices whose NVML memory fields are unavailable.
#[cfg(target_os = "linux")]
pub fn memory() -> Option<(u64, u64, bool)> {
    use std::ffi::c_void;
    type Context = *mut c_void;
    struct Library(*mut c_void);
    impl Drop for Library {
        fn drop(&mut self) {
            unsafe {
                libc::dlclose(self.0);
            }
        }
    }
    unsafe fn symbol<T: Copy>(library: &Library, name: &std::ffi::CStr) -> Option<T> {
        let pointer = unsafe { libc::dlsym(library.0, name.as_ptr()) };
        if pointer.is_null() {
            None
        } else {
            Some(unsafe { std::mem::transmute_copy(&pointer) })
        }
    }
    unsafe {
        let handle = libc::dlopen(c"libcuda.so.1".as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL);
        if handle.is_null() {
            return None;
        }
        let library = Library(handle);
        let init: unsafe extern "C" fn(u32) -> i32 = symbol(&library, c"cuInit")?;
        let device_get: unsafe extern "C" fn(*mut i32, i32) -> i32 =
            symbol(&library, c"cuDeviceGet")?;
        let get_current: unsafe extern "C" fn(*mut Context) -> i32 =
            symbol(&library, c"cuCtxGetCurrent")?;
        let set_current: unsafe extern "C" fn(Context) -> i32 =
            symbol(&library, c"cuCtxSetCurrent")?;
        let retain: unsafe extern "C" fn(*mut Context, i32) -> i32 =
            symbol(&library, c"cuDevicePrimaryCtxRetain")?;
        let release: unsafe extern "C" fn(i32) -> i32 =
            symbol(&library, c"cuDevicePrimaryCtxRelease_v2")?;
        let info: unsafe extern "C" fn(*mut usize, *mut usize) -> i32 =
            symbol(&library, c"cuMemGetInfo_v2")?;
        let attribute: unsafe extern "C" fn(*mut i32, i32, i32) -> i32 =
            symbol(&library, c"cuDeviceGetAttribute")?;
        let mut device = 0;
        if init(0) != 0 || device_get(&mut device, 0) != 0 {
            return None;
        }
        let mut previous = std::ptr::null_mut();
        if get_current(&mut previous) != 0 {
            return None;
        }
        let mut context = std::ptr::null_mut();
        if retain(&mut context, device) != 0 {
            return None;
        }
        let (mut free, mut total, mut integrated) = (0, 0, 0);
        let result = set_current(context) == 0 && info(&mut free, &mut total) == 0;
        let is_integrated = attribute(&mut integrated, 18, device) == 0 && integrated != 0;
        set_current(previous);
        release(device);
        if result && total > 0 && free <= total {
            Some((
                total as u64 / 1_048_576,
                free as u64 / 1_048_576,
                is_integrated,
            ))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn actual_driver_memory_when_gpu_validation_enabled() {
        if std::env::var("SCHEDULER_TEST_GPU").as_deref() != Ok("1") {
            return;
        }
        let (total, free, _) = super::memory().expect("CUDA driver memory query failed");
        assert!(total > 0 && free <= total);
        println!("CUDA telemetry: total={total} MiB, free={free} MiB");
    }
}
#[cfg(not(target_os = "linux"))]
pub fn memory() -> Option<(u64, u64, bool)> {
    None
}
