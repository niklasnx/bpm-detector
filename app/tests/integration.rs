//! Integration test: run synthetic click tracks (pure PCM) through the real
//! aubio tempo detector and verify the BpmDetector produces sane estimates.
//! This exercises the same code path as the live audio capture.

use bpmdetector::bpm::{BpmDetector, BpmUpdate};

// NOTE: We re-implement the small glue here instead of importing audio.rs
// (which needs cpal device access). The aubio hop/beat logic is identical.

use aubio_rs::Tempo;

const SAMPLE_RATE: u32 = 44100;
const WIN: usize = 1024;
const HOP: usize = 512;

/// Generate a click track: filtered kick-like bursts every beat.
fn click_track(bpm: f64, seconds: f64) -> Vec<f32> {
    let interval = (60.0 / bpm * SAMPLE_RATE as f64) as usize;
    let total = (seconds * SAMPLE_RATE as f64) as usize;
    let mut buf = vec![0.0f32; total];
    let mut pos = 0;
    while pos < total {
        // 60ms decaying kick: sine sweep with exponential decay
        let len = (0.06 * SAMPLE_RATE as f32) as usize;
        for i in 0..len.min(total - pos) {
            let t = i as f32 / SAMPLE_RATE as f32;
            let freq = 90.0 - 50.0 * (t / 0.06); // downward pitch sweep
            let env = (-t * 40.0).exp();
            buf[pos + i] = (2.0 * std::f32::consts::PI * freq * t).sin() * env * 0.95;
        }
        pos += interval;
    }
    buf
}

#[test]
fn aubio_detects_synth_clicks_120bpm() {
    let mut tempo = Tempo::new(aubio_rs::OnsetMode::SpecFlux, WIN, HOP, SAMPLE_RATE)
        .expect("aubio tempo");
    let samples = click_track(120.0, 8.0);

    let mut det = BpmDetector::new();
    let mut beat_times: Vec<f64> = Vec::new();
    let mut clock = 0.0f64;

    for chunk in samples.chunks(HOP) {
        clock += chunk.len() as f64 / SAMPLE_RATE as f64;
        if let Ok(r) = tempo.do_result(chunk) {
            if r > 0.5 {
                if let Some(bpm) = det.on_beat(clock, BpmUpdate::Beat { bpm_hint: tempo.get_bpm() as f64, confidence: tempo.get_confidence() as f64, beat_time: clock }) {
                    beat_times.push(bpm);
                }
            }
        }
    }

    assert!(
        !beat_times.is_empty(),
        "aubio found no beats in a 120 BPM click track"
    );
    let final_bpm = *beat_times.last().unwrap();
    assert!(
        (final_bpm - 120.0).abs() < 3.0 || (final_bpm - 60.0).abs() < 3.0 || (final_bpm - 240.0).abs() < 3.0,
        "final BPM {} should be within a half/double window of 120",
        final_bpm
    );
}

#[test]
fn aubio_detects_synth_clicks_98bpm() {
    let mut tempo = Tempo::new(aubio_rs::OnsetMode::SpecFlux, WIN, HOP, SAMPLE_RATE)
        .expect("aubio tempo");
    let samples = click_track(98.0, 8.0);

    let mut det = BpmDetector::new();
    let mut final_bpm = None;
    let mut clock = 0.0f64;

    for chunk in samples.chunks(HOP) {
        clock += chunk.len() as f64 / SAMPLE_RATE as f64;
        if let Ok(r) = tempo.do_result(chunk) {
            if r > 0.5 {
                if let Some(bpm) = det.on_beat(clock, BpmUpdate::Beat { bpm_hint: tempo.get_bpm() as f64, confidence: tempo.get_confidence() as f64, beat_time: clock }) {
                    final_bpm = Some(bpm);
                }
            }
        }
    }

    let fb = final_bpm.expect("no beats detected for 98 BPM track");
    assert!(
        (fb - 98.0).abs() < 4.0 || (fb - 49.0).abs() < 4.0 || (fb - 196.0).abs() < 4.0,
        "final BPM {} should be within a half/double window of 98",
        fb
    );
}

#[test]
fn silence_produces_no_estimates() {
    let mut tempo = Tempo::new(aubio_rs::OnsetMode::SpecFlux, WIN, HOP, SAMPLE_RATE)
        .expect("aubio tempo");
    let silence = vec![0.0f32; SAMPLE_RATE as usize * 4];
    let mut det = BpmDetector::new();
    let mut clock = 0.0f64;
    let mut got_beat = false;
    for chunk in silence.chunks(HOP) {
        clock += chunk.len() as f64 / SAMPLE_RATE as f64;
        if let Ok(r) = tempo.do_result(chunk) {
            if r > 0.5 {
                got_beat = true;
                let _ = det.on_beat(clock, BpmUpdate::Beat { bpm_hint: 0.0, confidence: 0.0, beat_time: clock });
            }
        }
    }
    assert!(!got_beat, "silence should not produce beats");
    assert!(det.is_stale(clock), "detector should be stale during silence");
}
