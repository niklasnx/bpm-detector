//! Ableton Link wrapper.
//!
//! With the vendored SDK (default): real Link sessions over the network.
//! With feature `stub`: no-ops, `Bridge::available()` returns false. Used for CI
//! cross-compilation targets without a C++ toolchain.

#[cfg(not(feature = "stub"))]
mod ffi;
#[cfg(not(feature = "stub"))]
pub use ffi::Bridge;

#[cfg(feature = "stub")]
mod stub_impl {
    pub struct Bridge;
    impl Bridge {
        pub fn new(_initial_bpm: f64) -> Option<Self> {
            None
        }
        pub fn set_enabled(&self, _on: bool) {}
        pub fn is_enabled(&self) -> bool {
            false
        }
        pub fn set_tempo(&self, _bpm: f64) {}
        pub fn tempo(&self) -> f64 {
            0.0
        }
        pub fn peers(&self) -> u64 {
            0
        }
        pub fn available() -> bool {
            false
        }
    }
}
#[cfg(feature = "stub")]
pub use stub_impl::Bridge;
