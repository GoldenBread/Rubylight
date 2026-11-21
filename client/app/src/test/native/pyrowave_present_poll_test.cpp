#include <cassert>
#include <cstdint>
#include "../../main/jni/pyrowave-renderer/present_poll.h"

static constexpr int64_t ms = 1'000'000;

// The idle loop of PyroWaveRenderer::presentLoop(): sleep waitNs() (or until a frame comes), ask,
// repeat while times are pending. Returns the driver asks over `spanNs` of a still stream.
static int idleAsks(int64_t refreshNs, int64_t spanNs, int64_t legacyStepNs) {
    int asks = 0;
    PyroWavePresentPoll poll;
    for (int64_t now = 0; now < spanNs;) {
        const int64_t wait = legacyStepNs > 0 ? legacyStepNs : poll.waitNs(now, refreshNs);
        now += wait;
        poll.polled(now);
        asks++;
        if (wait == 0) now += 1;  // an ask takes time; never a zero-length iteration
    }
    return asks;
}

static void testIdlePollingIsOncePerRefresh() {
    // Pending times expire after 500 ms. The 2 ms wait asked 250 times over that; one ask per
    // refresh is 30 at 60 Hz, 60 at 120 Hz.
    assert(idleAsks(16'666'667, 500 * ms, 2 * ms) >= 249);
    assert(idleAsks(16'666'667, 500 * ms, 0) <= 31);
    assert(idleAsks(8'333'333, 500 * ms, 0) <= 62);
}

static void testFirstAskIsImmediateThenOneRefreshApart() {
    PyroWavePresentPoll poll;
    assert(poll.waitNs(100 * ms, 16 * ms) == 0);
    poll.polled(100 * ms);
    assert(poll.waitNs(100 * ms, 16 * ms) == 16 * ms);
    assert(poll.waitNs(105 * ms, 16 * ms) == 11 * ms);
    assert(poll.waitNs(116 * ms, 16 * ms) == 0);
    assert(poll.waitNs(200 * ms, 16 * ms) == 0);
}

static void testPeriodFallbackAndBounds() {
    PyroWavePresentPoll poll;
    poll.polled(0);
    assert(poll.waitNs(0, 0) == PyroWavePresentPoll::DEFAULT_PERIOD_NS);
    assert(poll.waitNs(0, 1) == PyroWavePresentPoll::MIN_PERIOD_NS);
    assert(poll.waitNs(0, 10'000 * ms) == PyroWavePresentPoll::MAX_PERIOD_NS);
}

// Frames that come every refresh ask once each, at the start of their present: the idle wait is
// never reached, and an ask just made leaves the next one a refresh away.
static void testAnAskJustMadeIsNotRepeated() {
    PyroWavePresentPoll poll;
    poll.polled(50 * ms);
    assert(poll.waitNs(50 * ms + 100'000, 16 * ms) > 15 * ms);
}

int main() {
    testIdlePollingIsOncePerRefresh();
    testFirstAskIsImmediateThenOneRefreshApart();
    testPeriodFallbackAndBounds();
    testAnAskJustMadeIsNotRepeated();
    return 0;
}
