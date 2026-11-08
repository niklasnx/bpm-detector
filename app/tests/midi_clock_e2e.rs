#![cfg(unix)]

//! End-to-end MIDI Beat Clock test: create a virtual MIDI input, send the
//! clock to it through the real midir path, and count 0xF8 bytes.
//!
//! Ignored by default: virtual MIDI ports need CoreMIDI (macOS) or ALSA
//! (Linux) and are unavailable in some CI sandboxes.
//!
//! Run with: cargo test --release --test midi_clock_e2e -- --ignored --nocapture

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bpmdetector::midi::MidiClock;
use midir::os::unix::VirtualInput;

#[test]
#[ignore = "needs a virtual MIDI port"]
fn sends_24_ppq_clock() {
    let ticks = Arc::new(AtomicUsize::new(0));
    let counter = ticks.clone();

    let mut input = midir::MidiInput::new("bpmdetector-test-in").expect("midi input");
    input.ignore(midir::Ignore::None);
    let _conn = input
        .create_virtual(
            "bpmdetector-test-in",
            move |_stamp, msg, _| {
                if msg.contains(&0xF8) {
                    counter.fetch_add(1, Ordering::Relaxed);
                }
            },
            (),
        )
        .expect("virtual input");

    let bpm = Arc::new(std::sync::atomic::AtomicU32::new(12000)); // 120 BPM x100
    let clock = MidiClock::start(Some("bpmdetector-test-in".into()), bpm).expect("clock");

    let start = Instant::now();
    std::thread::sleep(Duration::from_millis(1500));
    let elapsed = start.elapsed().as_secs_f64();
    drop(clock);

    let got = ticks.load(Ordering::Relaxed);
    let expected = 120.0 / 60.0 * 24.0 * elapsed;
    assert!(
        got as f64 >= expected * 0.85 && got as f64 <= expected * 1.15,
        "expected ~{expected:.0} clock ticks in {elapsed:.2}s, got {got}"
    );
}
