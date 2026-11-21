#pragma once

#include <algorithm>
#include <cstdint>

// When the present thread asks the driver for the display times of frames it presented
// (vkGetPastPresentationTimingGOOGLE, which has no event or fence to wait on). A time exists only
// once the display showed the frame, at the earliest one refresh after the present, so asking more
// than once per refresh finds nothing new: while the stream is still and times are pending, the
// thread used to wake every 2 ms (500 Hz) and ask each time. Present thread only.
class PyroWavePresentPoll {
public:
    static constexpr int64_t DEFAULT_PERIOD_NS = 16'666'667;
    static constexpr int64_t MIN_PERIOD_NS = 4'000'000;
    static constexpr int64_t MAX_PERIOD_NS = 50'000'000;

    // The driver was (or is about to be) asked at `nowNs`.
    void polled(int64_t nowNs) {
        lastNs_ = nowNs;
        asked_ = true;
    }

    // How long the idle thread may sleep before it asks again: 0 when it is due, else the rest of
    // one refresh since the last ask. `refreshPeriodNs` is the display's, 0 when unknown.
    int64_t waitNs(int64_t nowNs, int64_t refreshPeriodNs) const {
        if (!asked_) return 0;
        const int64_t period = refreshPeriodNs > 0
                ? std::clamp(refreshPeriodNs, MIN_PERIOD_NS, MAX_PERIOD_NS) : DEFAULT_PERIOD_NS;
        return std::max<int64_t>(0, lastNs_ + period - nowNs);
    }

private:
    int64_t lastNs_ = 0;
    bool asked_ = false;
};
