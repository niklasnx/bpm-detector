//! Ableton Link integration: publishes the detected BPM into a live Link
//! session so Resolume / Ableton / Traktor / QLC+ (via MIDI clock from peers)
//! can sync to it.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use ableton_link::Bridge;

pub struct Link {
    bridge: Bridge,
    enabled: AtomicBool,
    peers_cache: AtomicU64,
    /// Shared BPM scaled x100 — updated by the detector, read by publish().
    shared_bpm: Arc<AtomicU32>,
}

impl Link {
    pub fn new(shared_bpm: Arc<AtomicU32>) -> Option<Self> {
        if !Bridge::available() {
            log::warn!("ableton link: bridge unavailable in this build");
            return None;
        }
        let bridge = Bridge::new(120.0)?;
        Some(Self {
            bridge,
            enabled: AtomicBool::new(false),
            peers_cache: AtomicU64::new(0),
            shared_bpm,
        })
    }

    pub fn available(&self) -> bool {
        true
    }

    pub fn set_enabled(&self, on: bool) {
        self.bridge.set_enabled(on);
        self.enabled.store(on, Ordering::Relaxed);
        log::info!("ableton link: {}", if on { "enabled" } else { "disabled" });
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub fn peers(&self) -> u64 {
        let p = self.bridge.peers();
        self.peers_cache.store(p, Ordering::Relaxed);
        p
    }

    /// Push the current BPM into the Link session graph.
    pub fn publish_bpm(&self) {
        if !self.is_enabled() {
            return;
        }
        let bpm = self.shared_bpm.load(Ordering::Relaxed) as f64 / 100.0;
        if bpm > 1.0 {
            self.bridge.set_tempo(bpm);
        }
    }

    pub fn session_tempo(&self) -> f64 {
        self.bridge.tempo()
    }
}
