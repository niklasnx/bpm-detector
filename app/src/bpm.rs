//! Core BPM detection logic. No GUI, no audio I/O — pure state machine,
//! fully unit-testable.

use std::collections::VecDeque;

/// Number of inter-beat intervals used for the median estimate.
pub const HISTORY_LEN: usize = 6;
/// Detected intervals outside this range (30-200 BPM) are rejected.
const MIN_INTERVAL: f64 = 0.30;
const MAX_INTERVAL: f64 = 2.00;
/// Interval must be within this factor of the previous one, else it is an
/// outlier (but the beat clock still advances - see regression test).
const OUTLIER_FACTOR: f64 = 1.9;
/// If an interval is ~2x the previous one, a beat was probably missed.
const DOUBLE_TOLERANCE: f64 = 0.12;
/// Seconds without a detected beat before the display resets to "---".
pub const STALE_AFTER: f64 = 2.0;
/// A confidence above this counts as a locked, reliable estimate.
const LOCK_CONFIDENCE: f64 = 0.45;
/// While unlocked, accept faster: shorter history reacts quicker to changes.
const WARMUP_LEN: usize = 3;
/// How many consecutive consistent intervals force an immediate re-lock when
/// they differ from the current estimate (pitch-fader responsiveness).
const SHIFT_CONFIRM: usize = 2;

/// One detected beat or onset, sent from the audio thread to the UI.
/// Times are sample-accurate positions from aubio, in seconds.
#[derive(Debug, Clone, Copy)]
pub enum BpmUpdate {
    /// aubio tempo tracker fired: its smoothed estimate + confidence.
    Beat {
        bpm_hint: f64,
        confidence: f64,
        /// Sample-accurate time of the detected beat (from aubio).
        beat_time: f64,
    },
    /// aubio onset detector fired: an audible transient (kick/snare).
    Onset {
        /// Sample-accurate time of the transient (from aubio).
        onset_time: f64,
    },
}

#[derive(Debug)]
pub struct BpmDetector {
    history: VecDeque<f64>,
    /// aubio's own smoothed estimate, tracked separately from interval math.
    hint_history: VecDeque<f64>,
    last_confidence: f64,
    /// Beat time of the last beat, in seconds (monotonic audio/UI clock).
    last_beat: Option<f64>,
    prev_interval: Option<f64>,
    last_estimate: Option<f64>,
    /// Consecutive intervals that agree with each other but not with the
    /// current estimate — pitch-shift confirmation counter.
    shift_count: usize,
    shift_candidate: Option<f64>,
    /// True once we have a stable lock (confidence + consistent history).
    locked: bool,
}

impl BpmDetector {
    pub fn new() -> Self {
        Self {
            history: VecDeque::with_capacity(HISTORY_LEN),
            hint_history: VecDeque::with_capacity(HISTORY_LEN),
            last_confidence: 0.0,
            last_beat: None,
            prev_interval: None,
            last_estimate: None,
            shift_count: 0,
            shift_candidate: None,
            locked: false,
        }
    }

    pub fn last_confidence(&self) -> f64 {
        self.last_confidence
    }

    pub fn is_locked(&self) -> bool {
        self.locked
    }

    /// Feed one event from the audio thread. Times are sample-accurate beat
    /// positions in seconds (monotonic). Returns a BPM estimate.
    pub fn on_beat(&mut self, _fallback_time: f64, update: BpmUpdate) -> Option<f64> {
        // Only tracked beats advance interval math; onsets are LED-only
        // (they double-fire on snares/hi-hats and would poison the median).
        let BpmUpdate::Beat { bpm_hint, confidence, beat_time } = update else {
            return self.last_estimate;
        };
        self.on_tracked_beat(beat_time, bpm_hint, confidence)
    }

    fn on_tracked_beat(&mut self, beat_time: f64, bpm_hint: f64, confidence: f64) -> Option<f64> {
        self.last_confidence = confidence;

        // Track aubio's own estimate; it handles genres where inter-onset
        // intervals are noisy (hip-hop, breakbeats, complex jazz) far better
        // than raw interval medians.
        if bpm_hint > 30.0 && bpm_hint < 250.0 {
            self.hint_history.push_back(bpm_hint);
            while self.hint_history.len() > HISTORY_LEN {
                self.hint_history.pop_front();
            }
        }

        let Some(last) = self.last_beat else {
            self.last_beat = Some(beat_time);
            return self.estimate();
        };

        let mut interval = beat_time - last;

        // ALWAYS advance the beat clock, even for outliers. Skipping the
        // update would let the next interval grow and get rejected forever
        // (the classic "stuck BPM" bug).
        self.last_beat = Some(beat_time);

        if interval < MIN_INTERVAL || interval > MAX_INTERVAL {
            return self.estimate();
        }

        // Half/double + outlier handling relative to the previous interval.
        if let Some(prev) = self.prev_interval {
            if (interval - 2.0 * prev).abs() < DOUBLE_TOLERANCE {
                interval /= 2.0; // missed beat in between
            } else if interval > OUTLIER_FACTOR * prev || interval < prev / OUTLIER_FACTOR {
                self.prev_interval = Some(interval);
                return self.estimate();
            }
        }
        self.prev_interval = Some(interval);

        let bpm = 60.0 / interval;
        self.history.push_back(bpm);
        while self.history.len() > HISTORY_LEN {
            self.history.pop_front();
        }

        self.estimate()
    }

    /// Combine interval median and aubio hint into a display estimate.
    /// While the signal is "easy" (steady electronic music), both agree.
    /// On complex material the hint dominates because it uses beat tracking
    /// over a longer window.
    fn estimate(&mut self) -> Option<f64> {
        let interval_median = self.median(&self.history).map(fold_to_range);
        let hint_median = self.median(&self.hint_history).map(fold_to_range);

        let conf = self.last_confidence;
        self.locked = conf >= LOCK_CONFIDENCE && self.history.len() >= WARMUP_LEN;

        // Pitch-fader responsiveness: when the last few intervals agree with
        // each other but not with the current estimate, adopt them
        // immediately instead of waiting for the whole history to drift over.
        let shift = self.detect_shift();

        let est = match (interval_median, hint_median) {
            (Some(a), Some(b)) => {
                if let Some(s) = shift {
                    s
                } else if (a - b).abs() / a.max(b) < 0.02 {
                    (a + b) / 2.0
                } else if conf >= LOCK_CONFIDENCE {
                    b
                } else {
                    a
                }
            }
            (Some(a), None) => shift.unwrap_or(a),
            (None, Some(b)) => shift.unwrap_or(b),
            (None, None) => return self.last_estimate,
        };

        if !(30.0..=250.0).contains(&est) {
            return self.last_estimate;
        }

        self.last_estimate = Some(est);
        self.last_estimate
    }

    /// Detect a sudden tempo shift: the most recent intervals form a new
    /// stable value different from the current estimate.
    fn detect_shift(&mut self) -> Option<f64> {
        let newest = self.history.back().copied()?;
        let est = self.last_estimate?;

        // Does the newest interval agree with the previous one?
        let consistent = match self.history.len().checked_sub(2).and_then(|i| self.history.get(i)) {
            Some(prev) => (newest - prev).abs() / newest.max(*prev) < 0.06,
            None => false,
        };

        if !consistent {
            self.shift_count = 0;
            self.shift_candidate = None;
            return None;
        }

        self.shift_candidate = Some(newest);
        self.shift_count += 1;

        // New stable tempo, different from the displayed one: adopt now.
        if self.shift_count >= SHIFT_CONFIRM {
            let folded = fold_to_range(newest);
            if (folded - est).abs() / est > 0.03 {
                self.shift_count = 0;
                self.shift_candidate = None;
                self.last_estimate = Some(folded);
                return Some(folded);
            }
        }
        None
    }

    fn median(&self, q: &VecDeque<f64>) -> Option<f64> {
        if q.is_empty() {
            return None;
        }
        let mut v: Vec<f64> = q.iter().copied().collect();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        Some(v[v.len() / 2])
    }

    /// True when no beat has arrived within `STALE_AFTER` seconds.
    pub fn is_stale(&self, now: f64) -> bool {
        match self.last_beat {
            Some(t) => now - t > STALE_AFTER,
            None => true,
        }
    }

    pub fn reset(&mut self) {
        self.history.clear();
        self.hint_history.clear();
        self.last_beat = None;
        self.prev_interval = None;
        self.last_estimate = None;
        self.last_confidence = 0.0;
        self.shift_count = 0;
        self.shift_candidate = None;
        self.locked = false;
    }
}

/// Fold a BPM into the musically meaningful 80-180 window.
fn fold_to_range(mut bpm: f64) -> f64 {
    while bpm < 80.0 {
        bpm *= 2.0;
    }
    while bpm > 180.0 {
        bpm /= 2.0;
    }
    bpm
}

impl Default for BpmDetector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn update(t: f64, bpm: f64, conf: f64) -> BpmUpdate {
        BpmUpdate::Beat { bpm_hint: bpm, confidence: conf, beat_time: t }
    }

    /// Simulate steady beats at a given BPM.
    fn feed_steady(det: &mut BpmDetector, bpm: f64, beats: usize, start: f64) -> Vec<Option<f64>> {
        feed_steady_conf(det, bpm, beats, start, 0.9)
    }

    fn feed_steady_conf(det: &mut BpmDetector, bpm: f64, beats: usize, start: f64, conf: f64) -> Vec<Option<f64>> {
        let interval = 60.0 / bpm;
        (0..beats)
            .scan(start, |t, _| {
                let cur = *t;
                *t += interval;
                Some(det.on_beat(cur, update(cur, bpm, conf)))
            })
            .collect()
    }

    fn assert_bpm(actual: Option<f64>, expected: f64) {
        let a = actual.unwrap_or_else(|| panic!("expected {expected}, got None"));
        assert!(
            (a - expected).abs() < 0.01 || (a - expected * 2.0).abs() < 0.01 || (a - expected / 2.0).abs() < 0.01,
            "expected {expected} (±half/double), got {a}"
        );
    }

    #[test]
    fn steady_120bpm() {
        let mut det = BpmDetector::new();
        let out = feed_steady(&mut det, 120.0, 10, 0.0);
        assert_bpm(out.last().copied().flatten(), 120.0);
    }

    #[test]
    fn steady_90bpm() {
        let mut det = BpmDetector::new();
        let out = feed_steady(&mut det, 90.0, 10, 0.0);
        assert_bpm(out.last().copied().flatten(), 90.0);
    }

    #[test]
    fn steady_174bpm() {
        let mut det = BpmDetector::new();
        let out = feed_steady(&mut det, 174.0, 12, 0.0);
        assert_bpm(out.last().copied().flatten(), 174.0);
    }

    /// Genres with syncopation: occasional off-grid onsets between beats.
    #[test]
    fn syncopated_beats_dont_derail_estimate() {
        let mut det = BpmDetector::new();
        let interval = 60.0 / 128.0;
        let mut t = 0.0;
        let mut last = None;
        for i in 0..24 {
            // Every 4th beat has a ghost onset 30% early (syncopation)
            let jitter = if i % 4 == 0 { -0.3 * interval } else { 0.0 };
            last = det.on_beat(t + jitter, update(t + jitter, 128.0, 0.9));
            t += interval;
        }
        assert_bpm(last, 128.0);
    }

    /// Hip-hop style: aubio hint gives ~90, intervals swing around it.
    #[test]
    fn hint_rescues_swing_groove() {
        let mut det = BpmDetector::new();
        let mut t = 0.0;
        let mut last = None;
        for i in 0..20 {
            let swing = if i % 2 == 0 { 0.02 } else { -0.02 };
            let interval = 60.0 / 92.0;
            last = det.on_beat(t + swing * interval, update(t + swing * interval, 92.0, 0.8));
            t += interval;
        }
        assert_bpm(last, 92.0);
    }

    #[test]
    fn tempo_change_120_to_130() {
        let mut det = BpmDetector::new();
        feed_steady(&mut det, 120.0, 10, 0.0);
        let mut t = 10.0 * (60.0 / 120.0);
        let interval = 60.0 / 130.0;
        let mut last = None;
        for _ in 0..8 {
            last = det.on_beat(t, update(t, 130.0, 0.9));
            t += interval;
        }
        assert_bpm(last, 130.0);
    }

    /// THE regression test for the "stuck at 156" bug.
    #[test]
    fn outlier_does_not_stick_detection() {
        let mut det = BpmDetector::new();
        feed_steady(&mut det, 120.0, 8, 0.0);
        let t = 7.0 * (60.0 / 120.0) + 4.0; // 4s gap
        let _ = det.on_beat(t, update(t, 120.0, 0.9));
        let mut t2 = t + 60.0 / 120.0;
        let mut recovered = None;
        for _ in 0..6 {
            recovered = det.on_beat(t2, update(t2, 120.0, 0.9));
            t2 += 60.0 / 120.0;
        }
        assert_bpm(recovered, 120.0);
    }

    #[test]
    fn missed_beat_is_corrected() {
        let mut det = BpmDetector::new();
        feed_steady(&mut det, 120.0, 8, 0.0);
        let t = 7.0 * (60.0 / 120.0) + 2.0 * (60.0 / 120.0);
        let est = det.on_beat(t, update(t, 120.0, 0.9));
        assert_bpm(est, 120.0);
    }

    /// Pitch-fader responsiveness: after a tempo change, only 2 consistent
    /// intervals at the new tempo are needed to adopt it (SHIFT_CONFIRM).
    #[test]
    fn pitch_shift_adopts_quickly() {
        let mut det = BpmDetector::new();
        feed_steady(&mut det, 120.0, 10, 0.0);
        let mut t = 10.0 * (60.0 / 120.0);
        let interval = 60.0 / 135.0;
        let mut adopted_at = None;
        for i in 0..6 {
            let est = det.on_beat(t, update(t, 135.0, 0.9));
            if adopted_at.is_none() && i >= 1 {
                if let Some(e) = est {
                    if (e - 135.0).abs() < 1.0 {
                        adopted_at = Some(i);
                    }
                }
            }
            t += interval;
        }
        // Must adopt by the 2nd interval at the new tempo, not wait for the
        // whole history to drain.
        assert!(adopted_at.unwrap_or(99) <= 2, "adopted at {:?}", adopted_at);
        assert_bpm(det.last_estimate, 135.0);
    }

    /// Onsets never poison interval math: they are LED-only and leave the
    /// existing estimate untouched.
    #[test]
    fn onsets_do_not_disturb_estimate() {
        let mut det = BpmDetector::new();
        feed_steady(&mut det, 120.0, 8, 0.0);
        let before = det.last_estimate;
        let interval = 60.0 / 128.0; // onsets at a different rate
        let mut t = 8.0 * (60.0 / 120.0);
        for _ in 0..10 {
            let est = det.on_beat(t, BpmUpdate::Onset { onset_time: t });
            assert_eq!(est, before);
            t += interval;
        }
    }

    #[test]
    fn staleness_after_silence() {
        let mut det = BpmDetector::new();
        feed_steady(&mut det, 120.0, 8, 0.0);
        let last_t = 7.0 * (60.0 / 120.0);
        assert!(!det.is_stale(last_t + 1.0));
        assert!(det.is_stale(last_t + 3.0));
    }

    #[test]
    fn half_bpm_folded_up() {
        let mut det = BpmDetector::new();
        let out = feed_steady(&mut det, 70.0, 10, 0.0);
        assert_bpm(out.last().copied().flatten(), 140.0);
    }

    #[test]
    fn double_bpm_folded_down() {
        let mut det = BpmDetector::new();
        let out = feed_steady(&mut det, 190.0, 10, 0.0);
        assert_bpm(out.last().copied().flatten(), 95.0);
    }

    /// Low confidence (quiet, ambiguous music) still yields the interval
    /// estimate, but the detector reports unlocked.
    #[test]
    fn low_confidence_unlocked() {
        let mut det = BpmDetector::new();
        let out = feed_steady_conf(&mut det, 120.0, 10, 0.0, 0.0);
        assert_bpm(out.last().copied().flatten(), 120.0);
        assert!(!det.is_locked(), "0.0 confidence should not lock");
        let mut det2 = BpmDetector::new();
        let _ = feed_steady(&mut det2, 120.0, 5, 0.0);
        assert!(det2.is_locked(), "0.9 confidence should lock");
    }
}
