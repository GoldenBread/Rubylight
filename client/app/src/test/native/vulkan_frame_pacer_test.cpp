#include <algorithm>
#include <cassert>
#include <cstdint>
#include <deque>
#include <string>
#include <utility>
#include <vector>
#include "../../main/jni/vulkan/frame_pacer.cpp"

using namespace vkr;

namespace {

constexpr int64_t kStartNs = 1'000'000'000;
constexpr int64_t kMs = 1'000'000;

// What the renderer does with the pacer: a frame joins the queue when it arrives (the oldest is
// dropped past maxQueued()), and at each vsync onVsync picks the frame to show and drops those
// before it.
struct Run {
    std::string shown;              // per vsync: '#' where a frame went up, '.' where none did
    std::vector<int64_t> waitedNs;  // per frame shown: its vsync less its arrival
    size_t mostWaiting = 0;         // frames left waiting after a vsync, from the second second on
    int skipped = 0;
    int overflowed = 0;
};

Run run(PacingMode mode, int fps, int64_t periodNs, const std::vector<int64_t>& arrivalsNs, int vsyncs) {
    FramePacer pacer(mode, fps, periodNs, JitterBuffer::Balanced);
    std::deque<FrameTiming> queue;
    Run out;
    size_t next = 0;
    for (int v = 0; v < vsyncs; v++) {
        const int64_t vsyncNs = kStartNs + v * periodNs;
        while (next < arrivalsNs.size() && arrivalsNs[next] <= vsyncNs) {
            FrameTiming timing;
            timing.arrivalNs = arrivalsNs[next++];
            pacer.onFrameArrived(timing);
            queue.push_back(timing);
            if (queue.size() > pacer.maxQueued()) {
                queue.pop_front();
                out.overflowed++;
            }
        }
        const std::vector<FrameTiming> waiting(queue.begin(), queue.end());
        const int choice = pacer.onVsync(vsyncNs, waiting.data(), waiting.size());
        if (choice >= 0) {
            out.skipped += choice;
            out.waitedNs.push_back(vsyncNs - queue[static_cast<size_t>(choice)].arrivalNs);
            queue.erase(queue.begin(), queue.begin() + choice + 1);
        }
        out.shown += choice >= 0 ? '#' : '.';
        if (vsyncNs - kStartNs >= 1'000'000'000) {
            out.mostWaiting = std::max(out.mostWaiting, queue.size());
        }
    }
    return out;
}

// Frames a whole multiple of vsyncs apart, each 1 ms before its vsync, every other one a vsync early
// (bunched up behind the one before it).
std::vector<int64_t> bunched(int vsyncsPerFrame, int64_t periodNs, int frames) {
    std::vector<int64_t> arrivals;
    for (int i = 0; i < frames; i++) {
        arrivals.push_back(kStartNs + i * vsyncsPerFrame * periodNs - kMs - (i % 2 == 1 ? periodNs : 0));
    }
    return arrivals;
}

// Smoothest and FPS-capped pacing on a refresh 2 to 4 times the stream's rate: a frame each turn, so
// a frame that comes a vsync early waits for its turn and every frame stays the same number of vsyncs.
void testWholeMultipleShowsAFrameEachTurn() {
    for (PacingMode mode : {PacingMode::Smoothness, PacingMode::CapFps}) {
        assert(run(mode, 60, 8'333'333, bunched(2, 8'333'333, 4), 8).shown == "#.#.#.#.");
        assert(run(mode, 60, 8'341'675, bunched(2, 8'341'675, 4), 8).shown == "#.#.#.#.");
        assert(run(mode, 40, 8'333'333, bunched(3, 8'333'333, 4), 12).shown == "#..#..#..#..");
        assert(run(mode, 30, 8'333'333, bunched(4, 8'333'333, 3), 12).shown == "#...#...#...");
    }
}

// Elsewhere each vsync with a frame waiting shows the oldest, as before: at the stream's rate, at a
// refresh that is no whole multiple of it, and for a stream faster than the screen.
void testOtherRefreshesShowAFrameEachVsync() {
    const std::vector<int64_t> burst = {kStartNs - 3 * kMs, kStartNs - 2 * kMs, kStartNs - kMs};
    for (PacingMode mode : {PacingMode::Smoothness, PacingMode::CapFps}) {
        assert(run(mode, 60, 16'666'667, burst, 5).shown == "###..");
        assert(run(mode, 60, 11'111'111, burst, 5).shown == "###..");
        assert(run(mode, 50, 8'333'333, burst, 5).shown == "###..");
        assert(run(mode, 120, 16'666'667, burst, 5).shown == "###..");
        // At a whole multiple they go up a turn apart, and the second, more than a stream frame old at
        // its turn with the third behind it, is skipped.
        assert(run(mode, 60, 8'333'333, burst, 5).shown == "#.#..");
    }
}

// A frame that comes late goes up at the first vsync it is there for, and the turns count from it.
void testLateFrameStartsTheTurnsAgain() {
    const int64_t periodNs = 8'333'333;
    const std::vector<int64_t> arrivals = {kStartNs - kMs, kStartNs + 2 * periodNs - kMs, kStartNs + 5 * periodNs - kMs,
                                           kStartNs + 6 * periodNs - kMs, kStartNs + 8 * periodNs - kMs};
    assert(run(PacingMode::Smoothness, 60, periodNs, arrivals, 10).shown == "#.#..#.#.#");
}

// A burst settles back: frames 10 and 11 come late, with frame 12. Frame 10 goes up at once; at the
// next turn 11 and 12 are more than a stream frame old with 13 behind them, so both are skipped and
// the frames after them wait no longer than before the burst.
void testBurstSettlesBack() {
    const int64_t periodNs = 8'333'333;
    std::vector<int64_t> arrivals;
    for (int i = 0; i < 40; i++) {
        arrivals.push_back(kStartNs + 2 * i * periodNs - kMs);
    }
    arrivals[10] = arrivals[11] = arrivals[12];
    const Run out = run(PacingMode::Smoothness, 60, periodNs, arrivals, 80);
    assert(out.skipped == 2);
    assert(out.overflowed == 0);
    assert(out.waitedNs.size() == 38);
    for (size_t i = 0; i < out.waitedNs.size(); i++) {
        assert(out.waitedNs[i] == kMs);
    }
}

// A host a little faster than the screen settles too, the case that overflowed: 60.000 fps on
// 119.88 Hz brings 0.06 frames a second more than turns of two vsyncs show. A frame more than a stream
// frame old with a newer one behind it is skipped, about 7 in two minutes, so no backlog builds up and
// no frame waits longer than a stream frame.
void testFasterHostSettles() {
    const int64_t periodNs = 8'341'675;
    std::vector<int64_t> arrivals;
    for (int i = 0; i < 7200; i++) {
        arrivals.push_back(kStartNs - kMs + i * 16'666'667LL);
    }
    const Run out = run(PacingMode::Smoothness, 60, periodNs, arrivals, 14'400);
    assert(out.overflowed == 0);
    assert(out.mostWaiting <= 1);
    assert(out.skipped >= 6 && out.skipped <= 8);
    for (int64_t waited : out.waitedNs) {
        assert(waited <= 16'666'667);
    }
}

}  // namespace

int main() {
    testWholeMultipleShowsAFrameEachTurn();
    testOtherRefreshesShowAFrameEachVsync();
    testLateFrameStartsTheTurnsAgain();
    testBurstSettlesBack();
    testFasterHostSettles();
    return 0;
}
