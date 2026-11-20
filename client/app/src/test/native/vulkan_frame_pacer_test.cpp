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
constexpr int64_t k60FpsNs = 16'666'667;

// What the renderer does with the pacer: a frame joins the queue when it arrives (the oldest is
// dropped past maxQueued()), and at each vsync onVsync picks the frame to show and drops those
// before it.
struct Run {
    std::string shown;              // per vsync: '#' where a frame went up, '.' where none did
    std::vector<int64_t> waitedNs;  // per frame shown: its vsync less its arrival
    std::vector<int> gaps;          // per frame shown after the first: vsyncs since the one before
    size_t mostWaiting = 0;         // frames left waiting after a vsync, from the second second on
    int skipped = 0;
    int overflowed = 0;

    size_t count(int gap) const { return static_cast<size_t>(std::count(gaps.begin(), gaps.end(), gap)); }
    int64_t mostWaitedNs() const { return *std::max_element(waitedNs.begin(), waitedNs.end()); }
};

Run run(PacingMode mode, int fps, int64_t periodNs, const std::vector<int64_t>& arrivalsNs, int vsyncs,
        bool adaptive = false) {
    FramePacer pacer(mode, fps, periodNs, JitterBuffer::Balanced);
    pacer.setAdaptiveRefresh(adaptive);
    std::deque<FrameTiming> queue;
    Run out;
    size_t next = 0;
    int lastShown = -1;
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
            if (lastShown >= 0) {
                out.gaps.push_back(v - lastShown);
            }
            lastShown = v;
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

// The same numbers on every run
struct Random {
    uint64_t state;
    int64_t below(int64_t limit) {
        state = state * 6364136223846793005ULL + 1442695040888963407ULL;
        return static_cast<int64_t>((state >> 33) % static_cast<uint64_t>(limit));
    }
};

// 60 fps by the host's clock, the first frame aheadNs before the first vsync, each off by up to
// jitterNs either way (evenly spread).
std::vector<int64_t> at60Fps(int frames, int64_t aheadNs, int64_t jitterNs = 0, uint64_t seed = 1) {
    std::vector<int64_t> arrivals;
    Random random{seed};
    for (int i = 0; i < frames; i++) {
        const int64_t offsetNs = jitterNs > 0 ? random.below(2 * jitterNs + 1) - jitterNs : 0;
        arrivals.push_back(kStartNs - aheadNs + i * k60FpsNs + offsetNs);
    }
    return arrivals;
}

// One frame in a hundred held 20 to 60 ms, the frames behind it coming with it
std::vector<int64_t> withStalls(std::vector<int64_t> arrivals, uint64_t seed) {
    Random random{seed * 977};
    for (size_t i = 1; i < arrivals.size(); i++) {
        if (random.below(100) == 0) {
            arrivals[i] += 20 * kMs + random.below(40 * kMs);
        }
        arrivals[i] = std::max(arrivals[i], arrivals[i - 1]);
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
        // At a whole multiple they go up a turn apart: the first at once, and at the next turn the
        // second and third are both more than a stream frame old, so the second gives way to the third.
        const Run multiple = run(mode, 60, 8'333'333, burst, 5);
        assert(multiple.shown == "#.#..");
        assert(multiple.skipped == 1);
        // With VRR chosen or a screen that sets its own refresh nothing waits for a turn, as in MediaCodec.
        assert(run(mode, 60, 8'333'333, burst, 5, true).shown == "###..");
        assert(run(mode, 60, 8'333'333, bunched(2, 8'333'333, 4), 8, true).shown == "##..##..");
    }
}

// A frame that comes late goes up at the first vsync it is there for, and the turns count from it.
void testLateFrameStartsTheTurnsAgain() {
    const int64_t periodNs = 8'333'333;
    const std::vector<int64_t> arrivals = {kStartNs - kMs, kStartNs + 2 * periodNs - kMs, kStartNs + 5 * periodNs - kMs,
                                           kStartNs + 6 * periodNs - kMs, kStartNs + 8 * periodNs - kMs};
    assert(run(PacingMode::Smoothness, 60, periodNs, arrivals, 10).shown == "#.#..#.#.#");
}

// A burst settles back: frames 10 and 11 come late, with frame 12, 4 ms before a turn. Frame 10, the
// burst's oldest, goes up at once; at the next turn 11 and 12 are both more than a stream frame old, so
// 11 gives way to 12. The frames after it then wait a stream frame longer than before the burst, each
// more than a vsync at its turn, until after two seconds of that one goes up at the vsync before its
// turn, and after two more the next: two frames a vsync short, and the frames after them wait 4 ms
// again, as before the burst.
void testBurstSettlesBack() {
    const int64_t periodNs = 8'333'333;
    std::vector<int64_t> arrivals;
    for (int i = 0; i < 450; i++) {
        arrivals.push_back(kStartNs + 2 * i * periodNs - 4 * kMs);
    }
    arrivals[10] = arrivals[11] = arrivals[12];
    const Run out = run(PacingMode::Smoothness, 60, periodNs, arrivals, 900);
    assert(out.skipped == 1);
    assert(out.overflowed == 0);
    assert(out.waitedNs.size() == 449);
    assert(out.waitedNs[10] == 4 * kMs);
    assert(out.waitedNs[11] == 2 * periodNs + 4 * kMs);
    assert(out.mostWaitedNs() == 2 * periodNs + 4 * kMs);
    assert(out.count(6) == 1 && out.count(1) == 2 && out.count(2) == out.gaps.size() - 3);
    for (size_t i = out.waitedNs.size() - 120; i < out.waitedNs.size(); i++) {
        assert(out.waitedNs[i] == 4 * kMs);
    }
}

// A host a little faster than the screen, the case that overflowed: 60.000 fps on 119.88 Hz brings
// 0.06 frames a second more than turns of two vsyncs show, each a little earlier for its turn. Once
// frames have waited more than a vsync at their turn for two seconds, one goes up at the vsync before
// its turn instead, a single frame up a vsync short (13 in two minutes), so no frame is skipped, no
// backlog builds up and no frame waits as long as a stream frame. At 119.6 Hz, 3.3 ms a second, the
// turns move earlier every 2.5 s and still skip none; the moves take back about 4 ms a second at most.
void testFasterHostMovesTheTurnsEarlier() {
    const Run out = run(PacingMode::Smoothness, 60, 8'341'675, at60Fps(7200, kMs), 14'400);
    assert(out.skipped == 0);
    assert(out.overflowed == 0);
    assert(out.mostWaiting <= 1);
    assert(out.count(1) == 13 && out.count(2) == out.gaps.size() - 13);
    assert(out.mostWaitedNs() < k60FpsNs);

    const Run faster = run(PacingMode::Smoothness, 60, 8'361'204, at60Fps(7200, kMs), 14'400);
    assert(faster.skipped == 0);
    assert(faster.overflowed == 0);
    assert(faster.mostWaiting <= 1);
    assert(faster.count(1) >= 40 && faster.count(1) + faster.count(2) == faster.gaps.size());
    assert(faster.mostWaitedNs() < k60FpsNs + 2 * kMs);
}

// A host further ahead than those moves take back, within the half a hertz per multiple that holds
// frames for their turn (60.000 fps on 119.4 and 119.1 Hz): a frame waits at most two vsyncs past the
// first one it is there for, so once one has it goes up a vsync early, and none is skipped (7 and 24 in
// two minutes with the moves alone).
void testMuchFasterHostSkipsNothing() {
    const std::vector<int64_t> periods = {8'375'209, 8'396'306};
    for (int64_t periodNs : periods) {
        const Run out = run(PacingMode::Smoothness, 60, periodNs, at60Fps(7200, kMs), 14'400);
        assert(out.skipped == 0);
        assert(out.overflowed == 0);
        assert(out.count(1) + out.count(2) == out.gaps.size());
        assert(out.mostWaitedNs() <= 3 * periodNs);
    }
}

// After a stall the frames that come together wait no more than that either: of several that have
// waited two vsyncs past the first one they are there for, the newest goes up and the others give way.
// The turns alone left them waiting up to 33 ms.
void testStalledFramesWaitNoLongerThanTwoVsyncs() {
    const std::vector<int64_t> periods = {8'333'333, 8'341'675};
    for (int64_t periodNs : periods) {
        for (uint64_t seed : {7ULL, 8ULL, 9ULL}) {
            const std::vector<int64_t> arrivals = withStalls(at60Fps(7200, 4 * kMs, 2 * kMs, seed), seed);
            const Run out = run(PacingMode::Smoothness, 60, periodNs, arrivals, 14'400);
            assert(out.mostWaitedNs() <= 3 * periodNs);
        }
    }
}

// Frames up to 8 ms either side of their time skip none: at their turn the oldest goes up even when it
// is more than a stream frame old with the next frame already there, which jitter brings often. Only
// the first frame that comes late moves the turns, once, and on 119.88 Hz the moves earlier above.
void testJitterSkipsNothing() {
    for (uint64_t seed : {1ULL, 2ULL, 3ULL}) {
        const Run out = run(PacingMode::Smoothness, 60, 8'333'333, at60Fps(7200, 4 * kMs, 8 * kMs, seed), 14'400);
        assert(out.skipped == 0);
        assert(out.overflowed == 0);
        assert(out.count(2) + 1 >= out.gaps.size());

        const Run drifting =
                run(PacingMode::Smoothness, 60, 8'341'675, at60Fps(7200, 4 * kMs, 8 * kMs, seed), 14'400);
        assert(drifting.skipped == 0);
        assert(drifting.overflowed == 0);
        assert(drifting.count(1) <= 15 && drifting.count(1) + drifting.count(2) + 1 >= drifting.gaps.size());
    }
}

}  // namespace

int main() {
    testWholeMultipleShowsAFrameEachTurn();
    testOtherRefreshesShowAFrameEachVsync();
    testLateFrameStartsTheTurnsAgain();
    testBurstSettlesBack();
    testFasterHostMovesTheTurnsEarlier();
    testMuchFasterHostSkipsNothing();
    testStalledFramesWaitNoLongerThanTwoVsyncs();
    testJitterSkipsNothing();
    return 0;
}
