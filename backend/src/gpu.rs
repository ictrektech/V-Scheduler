//! Topology identification must not depend on allocating a CUDA context.
#[derive(Debug)]
pub struct Telemetry {
    pub integrated: Option<bool>,
    pub name: Option<String>,
    pub memory: Option<(u64, u64)>,
}
#[cfg(target_os = "linux")]
pub fn telemetry(query_memory: bool) -> Option<Telemetry> {
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
        let attribute: unsafe extern "C" fn(*mut i32, i32, i32) -> i32 =
            symbol(&library, c"cuDeviceGetAttribute")?;
        let mut device = 0;
        if init(0) != 0 || device_get(&mut device, 0) != 0 {
            return None;
        }
        let mut value = 0;
        let integrated = (attribute(&mut value, 18, device) == 0).then_some(value != 0);
        let mut name = [0 as libc::c_char; 256];
        let device_name = symbol::<unsafe extern "C" fn(*mut libc::c_char, i32, i32) -> i32>(
            &library,
            c"cuDeviceGetName",
        )
        .and_then(|get| {
            (get(name.as_mut_ptr(), name.len() as i32, device) == 0).then(|| {
                std::ffi::CStr::from_ptr(name.as_ptr())
                    .to_string_lossy()
                    .into_owned()
            })
        });
        let mut telemetry = Telemetry {
            integrated,
            name: device_name,
            memory: None,
        };
        // Unified memory uses the host RAM pool. No context or separate CUDA
        // free-memory counter is needed to identify or budget that pool.
        if integrated == Some(true) || !query_memory {
            return Some(telemetry);
        }
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
        let mut previous = std::ptr::null_mut();
        if get_current(&mut previous) != 0 {
            return Some(telemetry);
        }
        let mut context = std::ptr::null_mut();
        if retain(&mut context, device) != 0 {
            return Some(telemetry);
        }
        let (mut free, mut total) = (0, 0);
        let result = set_current(context) == 0 && info(&mut free, &mut total) == 0;
        set_current(previous);
        release(device);
        if result && total > 0 && free <= total {
            telemetry.memory = Some((total as u64 / 1_048_576, free as u64 / 1_048_576));
        }
        Some(telemetry)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn actual_driver_memory_when_gpu_validation_enabled() {
        if std::env::var("SCHEDULER_TEST_GPU").as_deref() != Ok("1") {
            return;
        }
        let info = super::telemetry(true).expect("CUDA topology query failed");
        assert!(info.integrated.is_some());
        if std::env::var("SCHEDULER_EXPECT_UNIFIED").as_deref() == Ok("1") {
            assert_eq!(info.integrated, Some(true));
        }
        if info.integrated == Some(false) {
            let (total, free) = info.memory.expect("dedicated memory unavailable");
            assert!(total > 0 && free <= total);
        }
        println!(
            "CUDA topology: unified={:?}, separate_memory={:?}",
            info.integrated, info.memory
        );
    }
}
#[cfg(not(target_os = "linux"))]
pub fn telemetry(query_memory: bool) -> Option<Telemetry> {
    None
}
