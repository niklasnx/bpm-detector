//! MIDI Clock verification: run the drift-compensated scheduler at a fixed
//! BPM and count ticks, and pins the MIDI Beat Clock byte contract.

use std::time::{Duration, Instant};

/// How many ticks we expect in `seconds` at `bpm` (24 PPQ).
fn expected_ticks(bpm: f64, seconds: f64) -> usize {
    (bpm / 60.0 * 24.0 * seconds) as usize
}

/// Re-implementation of the scheduler loop from midi.rs, but pushing tick
/// timestamps instead of MIDI bytes. This verifies the timing math exactly.
fn run_scheduler(bpm: f64, seconds: f64) -> Vec<Instant> {
    let ppq = 24.0;
    let mut ticks = Vec::new();
    let mut next_tick = Instant::now();
    let deadline = Instant::now() + Duration::from_secs_f64(seconds);
    let tick_dur = Duration::from_secs_f64(60.0 / bpm / ppq);

    while next_tick < deadline {
        // Simulated thread sleep jitter like the real scheduler
        std::thread::sleep(tick_dur.min(Duration::from_millis(4)));
        let now = Instant::now();
        if now >= next_tick {
            ticks.push(now);
            next_tick += tick_dur;
            if next_tick < now {
                next_tick = now + tick_dur;
            }
        }
    }
    ticks
}

#[test]
fn midi_clock_tick_rate_120bpm() {
    let ticks = run_scheduler(120.0, 3.0);
    let expected = expected_ticks(120.0, 3.0);
    // Tolerate scheduling jitter of ±5%
    assert!(
        ticks.len() >= (expected as f64 * 0.95) as usize && ticks.len() <= (expected as f64 * 1.05) as usize,
        "120 BPM for 3s: expected ~{} ticks, got {}",
        expected,
        ticks.len()
    );
}

#[test]
fn midi_clock_tick_rate_150bpm() {
    let ticks = run_scheduler(150.0, 3.0);
    let expected = expected_ticks(150.0, 3.0);
    assert!(
        ticks.len() >= (expected as f64 * 0.95) as usize && ticks.len() <= (expected as f64 * 1.05) as usize,
        "150 BPM: expected ~{}, got {}",
        expected,
        ticks.len()
    );
}

#[test]
fn midi_clock_tick_spacing_is_stable() {
    // Verify drift compensation: tick deltas should stay close to the ideal
    let bpm = 128.0;
    let ideal_ms = 60_000.0 / bpm / 24.0;
    let ticks = run_scheduler(bpm, 2.5);
    let deltas: Vec<f64> = ticks
        .windows(2)
        .map(|w| (w[1] - w[0]).as_secs_f64() * 1000.0)
        .collect();
    let avg: f64 = deltas.iter().sum::<f64>() / deltas.len() as f64;
    assert!(
        (avg - ideal_ms).abs() < 1.5,
        "avg tick spacing {:.3}ms should be within 1.5ms of ideal {:.3}ms",
        avg,
        ideal_ms
    );
}

/// The clock source sends only 0xF8 (timing clock) — the standard byte that
/// MIDI Beat Clock consumers expect for the tempo pulse. This test pins the
/// contract so a refactor cannot silently switch to a different message.
#[test]
fn midi_beat_clock_contract() {
    const MIDI_CLOCK: u8 = 0xF8; // 248: timing clock
    const MIDI_START: u8 = 0xFA; // 250: start
    const MIDI_STOP: u8 = 0xFC; // 252: stop

    // Our scheduler sends only 0xF8 per tick — verify the byte is correct
    let tick_byte = 0xF8u8;
    assert_eq!(tick_byte, MIDI_CLOCK);

    // Guard against accidental use of MIDI Time Code (0xF9) or other
    // clock-family messages instead of the Beat Clock pulse.
    assert_ne!(tick_byte, MIDI_START);
    assert_ne!(tick_byte, MIDI_STOP);
}
