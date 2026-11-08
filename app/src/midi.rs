//! MIDI Clock output: 24 PPQ, drift-compensated scheduler running on its own
//! thread. BPM updates flow in via an atomic (scaled x100).

use midir::{MidiOutput, MidiOutputConnection};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

const PPQ: f64 = 24.0;

pub fn list_outputs() -> Vec<String> {
    let Ok(out) = MidiOutput::new("bpmdetector-scan") else { return vec![] };
    out.ports()
        .iter()
        .filter_map(|p| out.port_name(p).ok())
        .collect()
}

/// Scaled BPM (x100) shared between detector and clock thread.
pub type SharedBpm = std::sync::Arc<AtomicU32>;

pub struct MidiClock {
    stop: std::sync::Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    _conn: Option<MidiOutputConnection>,
}

impl MidiClock {
    pub fn start(port: Option<String>, bpm: SharedBpm) -> Result<Self, String> {
        let out = MidiOutput::new("bpmdetector").map_err(|e| e.to_string())?;
        let conn = match port {
            Some(name) => {
                let p = out
                    .ports()
                    .into_iter()
                    .find(|p| out.port_name(p).map(|n| n == name).unwrap_or(false))
                    .ok_or_else(|| format!("MIDI port not found: {name}"))?;
                Some(out.connect(&p, "bpmdetector-clock").map_err(|e| e.to_string())?)
            }
            None => {
                // Use the first available port if any, else run without output
                let ports = out.ports();
                ports
                    .first()
                    .and_then(|p| out.connect(p, "bpmdetector-clock").ok())
            }
        };

        if conn.is_none() {
            log::warn!("midi clock: no output port connected");
        }

        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let stop_cb = stop.clone();
        let conn_cell = std::cell::RefCell::new(conn);

        let thread = std::thread::Builder::new()
            .name("midi-clock".into())
            .spawn(move || {
                // Real-time-ish scheduling: sleep in short slices and compare
                // against a monotonic deadline, which absorbs sleep jitter.
                let mut next_tick = Instant::now();
                while !stop_cb.load(Ordering::Relaxed) {
                    let now = Instant::now();
                    if now >= next_tick {
                        if let Some(c) = conn_cell.borrow_mut().as_mut() {
                            let _ = c.send(&[0xF8]); // MIDI Clock
                        }
                        let bpm = bpm.load(Ordering::Relaxed) as f64 / 100.0;
                        let tick = Duration::from_secs_f64(60.0 / bpm.max(1.0) / PPQ);
                        next_tick += tick;
                        if next_tick < now {
                            next_tick = now + tick; // fell behind: resync
                        }
                        let until_tick = next_tick.saturating_duration_since(Instant::now());
                        std::thread::sleep(until_tick.min(Duration::from_millis(5)));
                    } else {
                        std::thread::sleep(next_tick - now);
                    }
                }
            })
            .map_err(|e| e.to_string())?;

        Ok(Self { stop, thread: Some(thread), _conn: None })
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for MidiClock {
    fn drop(&mut self) {
        self.stop();
    }
}
