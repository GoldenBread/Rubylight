#include <cassert>
#include <cstdint>
#include <string>
#include "../../main/jni/vulkan/frame_pacer.cpp"

using namespace vkr;

namespace {

constexpr int64_t kStartNs = 1'000'000'000;

// The vsyncs at which a pacer for `fps` on a `refreshHz` screen shows a frame, with `queued` frames
// waiting at each: '#' where a frame went up, '.' where the one on screen stayed.
std::string shown(PacingMode mode, int fps, double refreshHz, int vsyncs, size_t queued = 3) {
    const int64_t periodNs = static_cast<int64_t>(1e9 / refreshHz);
    FramePacer pacer(mode, fps, periodNs, JitterBuffer::Balanced);
    FrameTiming frames[3] = {};
    std::string out;
    for (int i = 0; i < vsyncs; i++) {
        out += pacer.onVsync(kStartNs + i * periodNs, frames, queued) == 0 ? '#' : '.';
    }
    return out;
}

// Smoothest and FPS-capped pacing on a refresh 2 to 4 times the stream's rate: a frame each turn,
// even with frames waiting at every vsync, so every frame stays the same number of vsyncs.
void testWholeMultipleShowsAFrameEachTurn() {
    for (PacingMode mode : {PacingMode::Smoothness, PacingMode::CapFps}) {
        assert(shown(mode, 60, 120.0, 8) == "#.#.#.#.");
        assert(shown(mode, 60, 119.88, 6) == "#.#.#.");
        assert(shown(mode, 40, 120.0, 9) == "#..#..#..");
        assert(shown(mode, 30, 120.0, 8) == "#...#...");
    }
}

// Elsewhere each vsync with a frame waiting shows the oldest, as before: at the stream's rate, at a
// refresh that is no whole multiple of it, and for a stream faster than the screen.
void testOtherRefreshesShowAFrameEachVsync() {
    for (PacingMode mode : {PacingMode::Smoothness, PacingMode::CapFps}) {
        assert(shown(mode, 60, 60.0, 6) == "######");
        assert(shown(mode, 60, 90.0, 6) == "######");
        assert(shown(mode, 50, 120.0, 6) == "######");
        assert(shown(mode, 120, 60.0, 6) == "######");
    }
}

// A frame that comes late goes up at the first vsync it is there for, and the turns count from it.
void testLateFrameStartsTheTurnsAgain() {
    const int64_t periodNs = 8'333'333;
    FramePacer pacer(PacingMode::Smoothness, 60, periodNs, JitterBuffer::Balanced);
    FrameTiming frames[1] = {};
    const size_t queued[] = {1, 1, 0, 1, 1, 1, 1};
    std::string out;
    for (size_t i = 0; i < sizeof(queued) / sizeof(queued[0]); i++) {
        out += pacer.onVsync(kStartNs + static_cast<int64_t>(i) * periodNs, frames, queued[i]) == 0 ? '#' : '.';
    }
    assert(out == "#..#.#.");
}

}  // namespace

int main() {
    testWholeMultipleShowsAFrameEachTurn();
    testOtherRefreshesShowAFrameEachVsync();
    testLateFrameStartsTheTurnsAgain();
    return 0;
}
