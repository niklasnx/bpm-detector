//! Live Ableton Link test: two Link instances on the same machine must
//! discover each other and propagate tempo. Ignored by default because it
//! needs multicast networking, which some CI sandboxes block.
//!
//! Run with: cargo test --release --test link -- --ignored --nocapture

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bpmdetector::link::Link;

#[test]
#[ignore = "needs multicast networking"]
fn two_instances_discover_and_sync() {
    let bpm_a: Arc<AtomicU32> = Arc::new(AtomicU32::new(0));
    let bpm_b: Arc<AtomicU32> = Arc::new(AtomicU32::new(0));

    let a = Link::new(bpm_a.clone()).expect("link A");
    let b = Link::new(bpm_b.clone()).expect("link B");
    a.set_enabled(true);
    b.set_enabled(true);

    // Wait for discovery, then A publishes a tempo.
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut peers = 0;
    while Instant::now() < deadline {
        peers = a.peers().max(b.peers());
        if peers > 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(peers > 0, "no Link peer discovered within 8s");

    bpm_a.store(12800, Ordering::Relaxed);
    a.publish_bpm();
    // Session tempo should converge to the published value.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut tempo = 0.0;
    while Instant::now() < deadline {
        tempo = b.session_tempo();
        if (tempo - 128.0).abs() < 0.05 {
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    assert!(
        (tempo - 128.0).abs() < 0.5,
        "peer tempo {tempo:.3} did not converge to 128"
    );

    a.set_enabled(false);
    b.set_enabled(false);
}

#[test]
fn bridge_tempo_roundtrip() {
    use ableton_link::Bridge;
    let b = Bridge::new(120.0).expect("bridge");
    b.set_enabled(true);
    b.set_tempo(128.0);
    std::thread::sleep(Duration::from_millis(200));
    let t = b.tempo();
    assert!((t - 128.0).abs() < 0.5, "tempo after set: {t}");
}
