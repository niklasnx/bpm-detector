// C bridge for Ableton Link (header-only SDK v4.x)
// Exposes exactly what the BPM detector needs: enable, tempo, peers.

#include <chrono>
#include <condition_variable>
#include <cstdint>
#include <mutex>

#include <ableton/Link.hpp>

namespace {

struct Bridge {
    ableton::Link link{120.0};
    std::mutex mutex;
};

} // namespace

extern "C" {

void* bpm_bridge_new(double initial_bpm) {
    (void)initial_bpm; // default tempo is set by the session itself
    auto* b = new Bridge();
    b->link.enable(true);
    b->link.enableStartStopSync(false);
    return b;
}

void bpm_bridge_destroy(void* handle) {
    delete static_cast<Bridge*>(handle);
}

void bpm_bridge_enable(void* handle, int enable) {
    auto* b = static_cast<Bridge*>(handle);
    std::lock_guard<std::mutex> lock(b->mutex);
    b->link.enable(enable != 0);
}

int bpm_bridge_is_enabled(void* handle) {
    auto* b = static_cast<Bridge*>(handle);
    return b->link.isEnabled() ? 1 : 0;
}

// Publish a tempo change: set it on the Link timeline at "now".
void bpm_bridge_set_tempo(void* handle, double bpm) {
    auto* b = static_cast<Bridge*>(handle);
    std::lock_guard<std::mutex> lock(b->mutex);
    const auto micros = b->link.clock().micros();
    auto state = b->link.captureAppSessionState();
    state.setTempo(bpm, micros);
    b->link.commitAppSessionState(state);
}

double bpm_bridge_get_tempo(void* handle) {
    auto* b = static_cast<Bridge*>(handle);
    auto state = b->link.captureAppSessionState();
    return state.tempo();
}

std::uint64_t bpm_bridge_num_peers(void* handle) {
    auto* b = static_cast<Bridge*>(handle);
    return b->link.numPeers();
}

int bpm_bridge_available(void) {
    return 1;
}

} // extern "C"
