# BPM Detector

Minimal real-time BPM detector. One small binary for **macOS, Windows and Linux** (including Raspberry Pi).


![Screenshot](docs/screenshot.png)

## Features

- **Real-time BPM detection** via [aubio](https://aubio.org) (SpecFlux onset + beat tracking), sample-accurate beat timing, half/double tempo folding, median smoothing and outlier rejection
- **Fast tempo tracking** — adopts pitch-fader tempo changes within ~2 beats
- **Beat LED** — flashes on the actual audible transient (kick/snare), sample-accurate, not on the smoothed number
- **Auto gain** — keeps the input level in the sweet spot automatically (on by default)
- **Audio device & channel selection**, input gain (-24…+24 dB) and level meter
- **MIDI Beat Clock** output (24 PPQ, drift-compensated)
- **Ableton Link** — share the detected tempo with Ableton, Resolume, Traktor, …
- Native GUI ([egui](https://www.egui.rs)), ~5 MB binary

### Languages

- **Rust** — everything the user interacts with: GUI, audio capture, BPM
  logic, MIDI clock, and the Link wrapper
- **C/C++** — only dependencies: the vendored Ableton Link SDK (header-only
  C++), a small C bridge to it, and [aubio](https://aubio.org) (C), which is
  compiled from source for onset/beat detection
- **JavaScript** — none

## Download

Grab a prebuilt binary from [Releases](../../releases) (macOS arm64/x64, Linux x64/arm64, Windows x64) or build it yourself:

```sh
cd app
cargo build --release
# binary: app/target/release/bpmdetector
```

On Linux you need ALSA/UDev headers, a C++ toolchain and libclang (used by
bindgen): `sudo apt install libasound2-dev libudev-dev g++ clang libclang-dev`.
On macOS the Xcode Command Line Tools are sufficient. On Windows, LLVM must be
installed (the GitHub Actions runners include it).

## Usage

1. Start the app, pick your audio device and channel
2. Play music — the BPM appears within a few beats
3. Enable **MIDI Clock** and/or **Ableton Link** as needed

The beat LED in the top-right corner flashes on the detected beat of the incoming signal.

### MIDI Beat Clock

The MIDI output sends standard MIDI Beat Clock (24 PPQ, `0xF8`), which is
consumed by DAWs, lighting consoles, video software and hardware sequencers
alike. Select an output port in the app and enable **MIDI Clock** — the
receiving application will follow the detected tempo.

### Ableton Link

Enable the Link checkbox to join the Link session on your network. Detected tempo changes are pushed into the session; peers follow automatically.

### System audio (loopback)

The detector listens to microphone/line inputs. To analyze system audio directly:

- **macOS**: install [BlackHole](https://existential.audio) (free), create a Multi-Output Device and select BlackHole as input
- **Windows**: WASAPI loopback support is planned
- **Linux**: select a monitor source of your output device (PipeWire/PulseAudio)

## Development

```sh
cd app
cargo test          # detector logic, aubio integration, MIDI clock timing
cargo run --release # GUI
```

Two end-to-end tests need real multicast networking / a virtual MIDI port and
are ignored by default:

```sh
cargo test --release --test link -- --ignored
cargo test --release --test midi_clock_e2e -- --ignored
```

CI (`.github/workflows/`):

- `ci.yml` — test + clippy + build on macOS, Windows, Linux; native aarch64 Linux build (Raspberry Pi)
- `release.yml` — on tag `v*`: artifacts for macOS (arm64/x64), Linux (x64/arm64), Windows (x64)

## Layout

```
app/
├── src/         # bpm, audio capture, midi clock, link, gui
├── link/        # ableton-link crate: C++ bridge + vendored SDK
└── tests/       # integration tests (synthetic click tracks, MIDI timing)
```

## License

[GPL-3.0](LICENSE). The app builds against aubio (GPL-3.0) and the
Ableton Link SDK (GPL-2.0-or-later), so the combined work is GPL-3.0.

