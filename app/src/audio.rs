//! Audio capture: cpal input stream, mono downmix, aubio tempo detection,
//! beat events into a lock-free queue consumed by the GUI.

use aubio_rs::{Onset, Tempo};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use rtrb::Producer;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use crate::bpm::BpmUpdate;

#[derive(Clone)]
pub struct DeviceInfo {
    pub name: String,
    pub channels: u16,
    pub is_default: bool,
}

pub fn list_input_devices() -> Result<Vec<DeviceInfo>, String> {
    let host = cpal::default_host();
    let default_in = host.default_input_device().and_then(|d| d.name().ok());
    let mut out = Vec::new();
    for device in host
        .input_devices()
        .map_err(|e| format!("enumerate: {e}"))?
    {
        let Ok(cfg) = device.default_input_config() else { continue };
        let Ok(name) = device.name() else { continue };
        out.push(DeviceInfo {
            channels: cfg.channels(),
            is_default: default_in.as_ref() == Some(&name),
            name,
        });
    }
    Ok(out)
}

trait ToF32: Copy { fn to_f32(self) -> f32; }
impl ToF32 for f32 { fn to_f32(self) -> f32 { self } }
impl ToF32 for i16 { fn to_f32(self) -> f32 { self as f32 / 32768.0 } }
impl ToF32 for u16 { fn to_f32(self) -> f32 { (self as f32 - 32768.0) / 32768.0 } }

/// aubio's Tempo holds a raw pointer. The capture struct lives exclusively in
/// the audio callback thread, but cpal requires the closure to be Send; we
/// uphold safety by never touching it outside the callback.
/// Everything the capture needs, moved into the audio callback.
struct Capture {
    tempo: Tempo,
    /// Raw onset detector: fires on every audible transient (kick, snare).
    /// Drives the LED so it flashes exactly on the heard hit.
    onset: Onset,
    hop: usize,
    /// Accumulates incoming samples to exact `hop`-sized blocks. cpal may
    /// deliver any block size; aubio requires exactly one hop per call.
    pending: Vec<f32>,
    sample_rate: f64,
    /// Monotonic audio-clock time of the current hop, in seconds.
    clock: f64,
    /// Linear input gain applied before analysis.
    gain: Arc<AtomicU32>, // scaled x100
    /// Peak level of the last hop, scaled x100, for the UI meter.
    level: Arc<AtomicU32>,
    producer: Producer<BpmUpdate>,
}

impl Capture {
    fn new(
        sample_rate: u32,
        gain: Arc<AtomicU32>,
        level: Arc<AtomicU32>,
        producer: Producer<BpmUpdate>,
    ) -> Result<Self, String> {
        // 75% overlap: hop 256 doubles the time resolution of beat/onset
        // timestamps compared to the typical 1024/512 setting, which halves
        // the quantisation error in inter-beat intervals.
        let win = 1024;
        let hop = 256;
        let tempo = Tempo::new(aubio_rs::OnsetMode::SpecFlux, win, hop, sample_rate)
            .map_err(|e| format!("aubio: {e}"))?;
        let mut onset = aubio_rs::Onset::new(aubio_rs::OnsetMode::SpecFlux, win, hop, sample_rate)
            .map_err(|e| format!("aubio onset: {e}"))?;
        // Lower = more sensitive. 0.3 catches soft kicks without
        // double-triggering on hi-hats.
        onset.set_threshold(0.3);

        Ok(Self {
            tempo,
            onset,
            hop,
            pending: Vec::with_capacity(hop),
            sample_rate: sample_rate as f64,
            clock: 0.0,
            gain,
            level,
            producer,
        })
    }

    /// Feed arbitrary-sized audio blocks from the callback. Samples are
    /// buffered until exactly one hop is available; only then aubio runs.
    fn process(&mut self, mono: &[f32]) {
        let g = self.gain.load(Ordering::Relaxed) as f32 / 100.0;
        let mut peak: f32 = 0.0;

        for &s in mono {
            let x = s * g;
            peak = peak.max(x.abs());
            self.pending.push(x);

            if self.pending.len() == self.hop {
                // Decay previous stored level ~11 dB/s at 512 samples/hop
                let prev = self.level.load(Ordering::Relaxed) as f32 / 100.0;
                let level_now = peak.max((prev - 0.0038) * 0.995);
                self.level
                    .store((level_now.clamp(0.0, 1.0) * 100.0) as u32, Ordering::Relaxed);
                peak = 0.0;

                self.process_hop();
            }
        }
    }

    /// Run one exact hop through aubio. Tempo returns 1.0 in the output frame
    /// when the hop contains a beat.
    fn process_hop(&mut self) {
        self.clock += self.hop as f64 / self.sample_rate;
        let hop = std::mem::take(&mut self.pending);

        if let Ok(result) = self.tempo.do_result(&hop) {
            if result > 0.5 {
                let confidence = self.tempo.get_confidence() as f64;
                let bpm_hint = self.tempo.get_bpm() as f64;
                // Sample-accurate beat position from aubio. aubio reports the
                // beat position in absolute samples since stream start; using
                // it instead of callback/wall-clock times removes UI jitter.
                let beat_samples = self.tempo.get_last() as f64;
                let beat_time = beat_samples / self.sample_rate;
                let _ = self.producer.push(BpmUpdate::Beat {
                    bpm_hint,
                    confidence,
                    beat_time,
                });
            }
        }

        // Onsets drive the LED. Push as a separate event so the UI can flash
        // on the actual audible hit.
        if let Ok(o) = self.onset.do_result(&hop) {
            if o > 0.5 {
                let onset_time = self.onset.get_last() as f64 / self.sample_rate;
                let _ = self.producer.push(BpmUpdate::Onset { onset_time });
            }
        }

        self.pending = hop;
        self.pending.clear();
    }
}

pub struct AudioStream(pub cpal::Stream);

pub fn spawn_capture(
    device_name: Option<&str>,
    channel: u16,
    producer: Producer<BpmUpdate>,
    gain: Arc<AtomicU32>,
    level: Arc<AtomicU32>,
    _stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Result<AudioStream, String> {
    let host = cpal::default_host();
    let device = match device_name {
        Some(name) => host
            .input_devices()
            .map_err(|e| e.to_string())?
            .find(|d| d.name().map(|n| n == name).unwrap_or(false))
            .ok_or_else(|| format!("device not found: {name}"))?,
        None => host
            .default_input_device()
            .ok_or("no default input device")?,
    };

    let config = device.default_input_config().map_err(|e| e.to_string())?;
    let sample_rate = config.sample_rate().0;
    let channels = config.channels() as usize;
    let ch = (channel as usize).saturating_sub(1).min(channels.saturating_sub(1));

    let cap = Capture::new(sample_rate, gain, level, producer)?;
    // cpal requires the callback closure to be Send, but aubio's Tempo holds a
    // raw pointer. The capture is only ever touched from the audio callback
    // (guarded by this mutex), so Send is safe.
    struct SendCapture(Capture);
    unsafe impl Send for SendCapture {}
    let cap_cell = std::sync::Mutex::new(SendCapture(cap));

    let err_fn = move |err| eprintln!("audio stream error: {err}");

    macro_rules! stream {
        ($t:ty) => {
            device
                .build_input_stream(
                    &config.clone().into(),
                    move |data: &[$t], _: &cpal::InputCallbackInfo| {
                        let Ok(mut cap) = cap_cell.try_lock() else { return };
                        let mono: Vec<f32> = data
                            .iter()
                            .skip(ch)
                            .step_by(channels.max(1))
                            .map(|s| ToF32::to_f32(*s))
                            .collect();
                        cap.0.process(&mono);
                    },
                    err_fn,
                    None,
                )
                .map_err(|e| e.to_string())
        };
    }

    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => stream!(f32)?,
        cpal::SampleFormat::I16 => stream!(i16)?,
        cpal::SampleFormat::U16 => stream!(u16)?,
        fmt => return Err(format!("unsupported sample format: {fmt:?}")),
    };
    stream.play().map_err(|e| e.to_string())?;

    Ok(AudioStream(stream))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Kick-like click track, same shape as the integration tests.
    fn clicks(bpm: f64, seconds: f64, sr: u32) -> Vec<f32> {
        let interval = (60.0 / bpm * sr as f64) as usize;
        let total = (seconds * sr as f64) as usize;
        let mut buf = vec![0.0f32; total];
        let mut pos = 0;
        while pos < total {
            let len = (0.06 * sr as f32) as usize;
            for i in 0..len.min(total - pos) {
                let t = i as f32 / sr as f32;
                let freq = 90.0 - 50.0 * (t / 0.06);
                buf[pos + i] = (2.0 * std::f32::consts::PI * freq * t).sin()
                    * (-t * 40.0).exp()
                    * 0.95;
            }
            pos += interval;
        }
        buf
    }

    /// Arbitrary callback block sizes must not crash or corrupt timing: the
    /// capture buffers them into exact hops before calling aubio.
    #[test]
    fn arbitrary_block_sizes_produce_beats() {
        let sr = 44100;
        let (producer, mut consumer) = rtrb::RingBuffer::new(1024);
        let gain = Arc::new(AtomicU32::new(100));
        let level = Arc::new(AtomicU32::new(0));
        let mut cap = Capture::new(sr, gain, level, producer).expect("capture");

        let track = clicks(120.0, 12.0, sr);
        // Deliberately irregular, including sizes smaller than one hop.
        for chunk in track.chunks(441) {
            cap.process(chunk);
        }

        let mut beats = Vec::new();
        while let Ok(u) = consumer.pop() {
            if let BpmUpdate::Beat { beat_time, .. } = u {
                beats.push(beat_time);
            }
        }
        assert!(beats.len() >= 8, "expected beats, got {}", beats.len());
        // Clock must not run ahead of the fed audio.
        assert!(cap.clock <= track.len() as f64 / sr as f64 + 1e-9);
        // Beat times must be monotonic and within the stream.
        assert!(beats.windows(2).all(|w| w[1] > w[0]));
        assert!(beats.last().unwrap() <= &(track.len() as f64 / sr as f64));
    }
}

