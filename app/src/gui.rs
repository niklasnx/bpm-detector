//! egui GUI: BPM display, device/channel selectors, MIDI clock + Link toggles.

use eframe::egui;
use std::sync::{
    atomic::{AtomicBool, AtomicU32, Ordering},
    Arc, Mutex,
};

use crate::audio;
use crate::bpm::{BpmDetector, BpmUpdate};
use crate::midi::{self, SharedBpm};
use crate::link;
pub struct App {
    devices: Vec<audio::DeviceInfo>,
    selected_device: Option<String>,
    selected_channel: usize,
    midi_ports: Vec<String>,
    selected_midi_port: Option<String>,
    midi_enabled: bool,
    link_enabled: bool,

    detector: BpmDetector,
    updates: Arc<Mutex<rtrb::Consumer<BpmUpdate>>>,
    stop_flag: Arc<AtomicBool>,
    current_stream: Option<audio::AudioStream>,
    shared_bpm: SharedBpm,
    link: Option<link::Link>,
    clock_holder: Option<Box<midi::MidiClock>>,

    displayed_bpm: Option<f64>,
    status: String,
    link_peers: u64,
    /// egui time of the most recent beat — drives the signal LED.
    last_beat_at: Option<f64>,
    /// Automatic gain control: keeps the input level near the target by
    /// slowly adjusting the manual gain.
    auto_gain: bool,
    /// Wall-clock time of the last auto-gain step (rate limiting).
    last_agc_step: Option<f64>,
    /// Current position in the aubio sample clock (seconds since stream
    /// start). Shared by the detector and the staleness check.
    detector_time: f64,
    /// Wall-clock time when detector_time was last advanced — used to keep
    /// the LED glow fading in real time.
    detector_time_wall: f64,
    /// Input gain, linear, scaled x100 in the atomic.
    gain: Arc<AtomicU32>,
    /// Peak input level, linear, scaled x100 in the atomic.
    level: Arc<AtomicU32>,
}

impl App {
    pub fn new(_producer: rtrb::Producer<BpmUpdate>, consumer: rtrb::Consumer<BpmUpdate>) -> Self {
        let devices = audio::list_input_devices().unwrap_or_default();
        let selected_device = devices
            .iter()
            .find(|d| d.is_default)
            .or_else(|| devices.first())
            .map(|d| d.name.clone());
        let midi_ports = midi::list_outputs();
        let shared_bpm: SharedBpm = Arc::new(AtomicU32::new(0));
        let link = link::Link::new(shared_bpm.clone());
        let mut app = Self {
            devices,
            selected_device,
            selected_channel: 1,
            midi_ports,
            selected_midi_port: None,
            midi_enabled: false,
            link_enabled: false,
            detector: BpmDetector::new(),
            updates: Arc::new(Mutex::new(consumer)),
            stop_flag: Arc::new(AtomicBool::new(false)),
            current_stream: None,
            shared_bpm,
            link,
            clock_holder: None,
            displayed_bpm: None,
            status: String::new(),
            link_peers: 0,
            last_beat_at: None,
            auto_gain: true,
            last_agc_step: None,
            detector_time: 0.0,
            detector_time_wall: 0.0,
            gain: Arc::new(AtomicU32::new(100)), // 1.0x
            level: Arc::new(AtomicU32::new(0)),
        };
        app.restart_capture();
        app
    }

    fn restart_capture(&mut self) {
        // Stop previous stream
        self.current_stream = None;
        self.detector.reset();
        self.displayed_bpm = None;
        self.detector_time = 0.0;
        self.last_beat_at = None;

        let (prod, cons) = rtrb::RingBuffer::<BpmUpdate>::new(256);
        // Replace the UI-side consumer
        *self.updates.lock().unwrap() = cons;

        match audio::spawn_capture(
            self.selected_device.as_deref(),
            self.selected_channel as u16,
            prod,
            self.gain.clone(),
            self.level.clone(),
            self.stop_flag.clone(),
        ) {
            Ok(stream) => {
                self.current_stream = Some(stream);
                self.status = format!(
                    "listening: {} / ch {}",
                    self.selected_device.as_deref().unwrap_or("?"),
                    self.selected_channel
                );
            }
            Err(e) => {
                self.status = format!("error: {e}");
                log::error!("capture failed: {e}");
            }
        }
    }

    fn set_midi(&mut self, on: bool) {
        if on {
            match midi::MidiClock::start(self.selected_midi_port.clone(), self.shared_bpm.clone()) {
                Ok(clock) => {
                    self.status = format!("MIDI clock → {}", self.selected_midi_port.as_deref().unwrap_or("first port"));
                    self.clock_holder = Some(Box::new(clock));
                }
                Err(e) => {
                    self.status = format!("MIDI error: {e}");
                    self.midi_enabled = false;
                }
            }
        } else {
            self.clock_holder = None;
            self.status = "MIDI clock off".into();
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // 1. Drain beat updates from the audio thread. All times (beat,
        //    onset, staleness) live in the aubio sample clock, which runs
        //    monotonic from stream start — independent of UI repaint jitter.
        let mut latest_event_time = self.detector_time;
        {
            let mut cons = self.updates.lock().unwrap();
            while let Ok(u) = cons.pop() {
                match u {
                    BpmUpdate::Onset { onset_time } => {
                        latest_event_time = latest_event_time.max(onset_time);
                        // LED: flash at the sample-accurate hit position
                        self.last_beat_at = Some(onset_time);
                    }
                    BpmUpdate::Beat { beat_time, .. } => {
                        latest_event_time = latest_event_time.max(beat_time);
                        // Fallback flash when the onset detector misses a beat
                        if self.last_beat_at.map(|t| beat_time - t > 0.4).unwrap_or(true) {
                            self.last_beat_at = Some(beat_time);
                        }
                    }
                }
                if let Some(bpm) = self.detector.on_beat(latest_event_time, u) {
                    self.displayed_bpm = Some(bpm);
                    self.shared_bpm.store((bpm * 100.0) as u32, Ordering::Relaxed);
                    if let Some(l) = &self.link {
                        l.publish_bpm();
                    }
                }
            }
        }
        self.detector_time = latest_event_time;
        self.detector_time_wall = ctx.input(|i| i.time);

        // 1b. Automatic gain control: nudge the gain so the input peak sits
        //     near ~0.5. Slow, rate-limited steps so the reaction does not
        //     pump on individual transients. Disabled while the user drags
        //     the slider (slider change takes precedence).
        if self.auto_gain {
            let wall_now = ctx.input(|i| i.time);
            let dt_ok = self.last_agc_step.map(|t| wall_now - t > 0.2).unwrap_or(true);
            if dt_ok {
                let peak = self.level.load(Ordering::Relaxed) as f64 / 100.0;
                // Only react when there is real signal
                if peak > 0.02 {
                    const TARGET: f64 = 0.5;
                    if (peak - TARGET).abs() / TARGET > 0.15 {
                        let linear = self.gain.load(Ordering::Relaxed) as f64 / 100.0;
                        // Proportional step, capped at ±1 dB per tick
                        let factor = (TARGET / peak).clamp(0.875, 1.125);
                        let new = (linear * factor).clamp(0.0631, 15.849); // -24…+24 dB
                        self.gain.store((new * 100.0) as u32, Ordering::Relaxed);
                    }
                }
                self.last_agc_step = Some(wall_now);
            }
        }

        // 2. Staleness: show "---" (in the audio clock)
        if self.detector.is_stale(self.detector_time) {
            self.displayed_bpm = None;
        }

        // 3. Draw. The LED glow must fade in real time even while no new
        //    events arrive, so "now" is the audio clock position
        //    extrapolated by the wall time since the last audio event.
        let now = self.detector_time
            + (ctx.input(|i| i.time) - self.detector_time_wall).max(0.0);

        egui::CentralPanel::default()
            .frame(egui::Frame::default().fill(egui::Color32::from_rgb(0x14, 0x14, 0x14)))
            .show(ctx, |ui| {
                // Signal LED (top right): bright on beat, fades over ~300 ms,
                // dark when no signal.
                let led_center =
                    egui::pos2(ui.clip_rect().right() - 24.0, ui.clip_rect().top() + 12.0);
                let age = self.last_beat_at.map(|t| now - t).unwrap_or(f64::INFINITY);
                let glow = if age < 0.3 { (1.0 - age / 0.3) as f32 } else { 0.0 };
                let base = egui::Color32::from_rgb(0x33, 0x33, 0x33);
                let hot = egui::Color32::from_rgb(0x30, 0xe0, 0x60);
                let color = egui::Color32::from_rgba_unmultiplied(
                    base.r() + ((hot.r() - base.r()) as f32 * glow) as u8,
                    base.g() + ((hot.g() - base.g()) as f32 * glow) as u8,
                    base.b() + ((hot.b() - base.b()) as f32 * glow) as u8,
                    255,
                );
                let radius = 5.0 + 2.0 * glow;
                ui.painter().circle_filled(led_center, radius, color);

                ui.vertical_centered(|ui| {
                    ui.add_space(18.0);

                    // Big BPM number
                    let text = self
                        .displayed_bpm
                        .map(|b| format!("{:.0}", b))
                        .unwrap_or_else(|| "---".into());
                    ui.colored_label(
                        egui::Color32::from_rgb(0xe6, 0xe6, 0xe6),
                        egui::RichText::new(&text).size(96.0),
                    );

                    ui.add_space(10.0);

                    // Device / channel row
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Audio Device:").color(egui::Color32::GRAY));
                        let before = self.selected_device.clone();
                        egui::ComboBox::from_id_salt("device")
                            .selected_text(
                                self.selected_device
                                    .as_deref()
                                    .unwrap_or("<select>")
                                    .to_string(),
                            )
                            .width(220.0)
                            .show_ui(ui, |ui| {
                                for d in &self.devices {
                                    let label = if d.is_default {
                                        format!("{}  (default)", d.name)
                                    } else {
                                        d.name.clone()
                                    };
                                    ui.selectable_value(&mut self.selected_device, Some(d.name.clone()), label);
                                }
                            });
                        if before != self.selected_device {
                            self.restart_capture();
                        }

                        ui.label(egui::RichText::new("Channel:").color(egui::Color32::GRAY));
                        let max_ch = self
                            .devices
                            .iter()
                            .find(|d| Some(&d.name) == self.selected_device.as_ref())
                            .map(|d| d.channels as usize)
                            .unwrap_or(2);
                        let before_ch = self.selected_channel;
                        egui::ComboBox::from_id_salt("channel")
                            .selected_text(self.selected_channel.to_string())
                            .width(50.0)
                            .show_ui(ui, |ui| {
                                for c in 1..=max_ch.max(1) {
                                    ui.selectable_value(&mut self.selected_channel, c, c.to_string());
                                }
                            });
                        if before_ch != self.selected_channel {
                            self.restart_capture();
                        }
                    });

                    ui.add_space(6.0);

                    // Gain + input level meter row
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Gain:").color(egui::Color32::GRAY));
                        let mut gain_db: f64 = 20.0 * (self.gain.load(Ordering::Relaxed) as f64 / 100.0).log10();
                        let slider = ui.add(
                            egui::Slider::new(&mut gain_db, -24.0..=24.0)
                                .text("")
                                .show_value(true)
                                .suffix(" dB"),
                        );
                        if slider.changed() {
                            self.gain
                                .store((10.0f64.powf(gain_db / 20.0) * 100.0) as u32, Ordering::Relaxed);
                        }

                        ui.checkbox(&mut self.auto_gain, "Auto");

                        ui.label(egui::RichText::new("In:").color(egui::Color32::GRAY));
                        let peak = self.level.load(Ordering::Relaxed) as f32 / 100.0;
                        let bar_w = 80.0;
                        let (rect, _) = ui.allocate_exact_size(
                            egui::vec2(bar_w, 10.0),
                            egui::Sense::hover(),
                        );
                        ui.painter().rect_filled(rect, 3.0, egui::Color32::from_rgb(0x22, 0x22, 0x22));
                        let level_w = rect.width() * peak.clamp(0.0, 1.0);
                        let meter_color = if peak > 0.95 {
                            egui::Color32::from_rgb(0xe0, 0x40, 0x40) // clip
                        } else if peak > 0.7 {
                            egui::Color32::from_rgb(0xe0, 0xc0, 0x40) // hot
                        } else {
                            egui::Color32::from_rgb(0x30, 0xc0, 0x60) // ok
                        };
                        if level_w > 1.0 {
                            ui.painter().rect_filled(
                                egui::Rect::from_min_size(rect.left_top(), egui::vec2(level_w, rect.height())),
                                3.0,
                                meter_color,
                            );
                        }
                    });

                    ui.add_space(10.0);

                    // MIDI / Link row
                    ui.horizontal(|ui| {
                        if ui.checkbox(&mut self.midi_enabled, "MIDI Clock").changed() {
                            self.set_midi(self.midi_enabled);
                        }
                        ui.add_space(12.0);
                        egui::ComboBox::from_id_salt("midiport")
                            .selected_text(
                                self.selected_midi_port
                                    .as_deref()
                                    .unwrap_or("<first port>")
                                    .to_string(),
                            )
                            .width(140.0)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut self.selected_midi_port, None, "<first port>");
                                for p in &self.midi_ports {
                                    ui.selectable_value(&mut self.selected_midi_port, Some(p.clone()), p);
                                }
                            });
                        if ui.checkbox(&mut self.link_enabled, "Ableton Link").changed() {
                            if let Some(l) = &self.link {
                                l.set_enabled(self.link_enabled);
                            }
                            self.status = if self.link_enabled {
                                "Ableton Link on".into()
                            } else {
                                "Ableton Link off".into()
                            };
                        }
                        if let Some(l) = &self.link {
                            self.link_peers = l.peers();
                            if self.link_enabled {
                                ui.label(
                                    egui::RichText::new(format!("{} peers", self.link_peers))
                                        .size(12.0)
                                        .color(egui::Color32::from_rgb(0x88, 0x88, 0x88)),
                                );
                            }
                        }
                    });

                    ui.add_space(6.0);
                    ui.label(egui::RichText::new(&self.status).size(12.0).color(egui::Color32::from_rgb(0x88, 0x88, 0x88)));
                });
            });

        // Repaint fast while a beat glow is fading so the LED animates, and
        // fast enough in general that the level meter looks live.
        let glow_active = self.last_beat_at.map(|t| now - t).unwrap_or(f64::INFINITY) < 0.35;
        ctx.request_repaint_after(if glow_active {
            std::time::Duration::from_millis(16)
        } else {
            std::time::Duration::from_millis(33) // ~30 fps: meter + BPM stay snappy
        });
    }
}
