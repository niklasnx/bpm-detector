use std::os::raw::{c_double, c_int, c_void};

unsafe extern "C" {
    fn bpm_bridge_new(initial_bpm: c_double) -> *mut c_void;
    fn bpm_bridge_destroy(handle: *mut c_void);
    fn bpm_bridge_enable(handle: *mut c_void, enable: c_int);
    fn bpm_bridge_is_enabled(handle: *mut c_void) -> c_int;
    fn bpm_bridge_set_tempo(handle: *mut c_void, bpm: c_double);
    fn bpm_bridge_get_tempo(handle: *mut c_void) -> c_double;
    fn bpm_bridge_num_peers(handle: *mut c_void) -> u64;
    fn bpm_bridge_available() -> c_int;
}

/// Handle to a live Ableton Link session. Thread-safe (bridge locks internally).
pub struct Bridge(*mut c_void);

unsafe impl Send for Bridge {}
unsafe impl Sync for Bridge {}

impl Bridge {
    pub fn new(initial_bpm: f64) -> Option<Self> {
        let ptr = unsafe { bpm_bridge_new(initial_bpm) };
        if ptr.is_null() { None } else { Some(Bridge(ptr)) }
    }
    pub fn set_enabled(&self, on: bool) {
        unsafe { bpm_bridge_enable(self.0, on as c_int) }
    }
    pub fn is_enabled(&self) -> bool {
        unsafe { bpm_bridge_is_enabled(self.0) != 0 }
    }
    pub fn set_tempo(&self, bpm: f64) {
        unsafe { bpm_bridge_set_tempo(self.0, bpm) }
    }
    pub fn tempo(&self) -> f64 {
        unsafe { bpm_bridge_get_tempo(self.0) }
    }
    pub fn peers(&self) -> u64 {
        unsafe { bpm_bridge_num_peers(self.0) }
    }
    pub fn available() -> bool {
        unsafe { bpm_bridge_available() == 1 }
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        unsafe { bpm_bridge_destroy(self.0) }
    }
}
