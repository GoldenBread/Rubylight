#include <algorithm>
#include <cassert>
#include <cstdint>
#include "../../main/jni/vulkan/present_scheduler.cpp"

using namespace vkr;

constexpr int64_t kPeriodNs = 16'666'667;

// What Android's loader (AOSP frameworks/native vulkan/libvulkan/swapchain.cpp, release branches
// 11 to 16) reports and returns for a FIFO swapchain, given the compositor's minimum undequeued
// buffers (`kept`). The renderer asks for max(minImageCount, wanted) images.
struct Loader {
    int apiLevel;
    uint32_t minImageCount;
    uint32_t requested;
    uint32_t images;
};

static Loader android(int release, uint32_t kept, uint32_t wanted) {
    Loader loader {};
    loader.apiLevel = release == 11 ? 30 : release == 12 ? 31 : release == 13 ? 33 : release == 14 ? 34 : 35;
    loader.minImageCount = release <= 12 ? 2 : release == 13 ? 3 : release == 14 ? kept + 1 : kept + 2;
    loader.requested = std::max(loader.minImageCount, wanted);
    loader.images = release == 12 ? std::max(kept + 2, loader.requested)
                    : release <= 14 ? loader.requested - 1 + kept
                                    : loader.requested;
    return loader;
}

// The longest delay that swapchain holds by the loader's own accounting: the images the renderer
// can have besides what the compositor keeps, less one to draw into, plus one (a delay of d has
// d - 1 frames waiting), less one more for a frame presented ahead waiting for its vsync
static int holds(const Loader& loader, uint32_t kept, bool presentAhead) {
    return static_cast<int>(loader.images) - static_cast<int>(kept) - (presentAhead ? 1 : 0);
}

static int maxDelayFor(const Loader& loader, bool presentAhead) {
    return PresentScheduler::maxDelayFor(loader.requested, loader.images, loader.minImageCount, loader.apiLevel,
                                         presentAhead);
}

// Late presents at the scheduler's own delay, the way the compositor reports a delay that is too
// short: each reaches the screen a vsync after the one asked for, and could not have been sooner
static void presentLate(PresentScheduler& scheduler, int count) {
    for (int i = 0; i < count; i++) {
        const int64_t vsyncNs = (i + 1) * kPeriodNs;
        const int delay = scheduler.delayVsyncs();
        const int64_t actualNs = vsyncNs + (delay + 1) * kPeriodNs;
        scheduler.onPresented(vsyncNs, delay, kPeriodNs, actualNs, actualNs, 0);
    }
}

int main() {
    for (int release : {11, 12, 13, 14, 15, 16}) {
        for (uint32_t kept : {1u, 2u}) {
            // Frames shown at their vsync ask for 3 images: the delay of 2 they start at, on every
            // release and compositor (a cap of images - 2 was 1 with 3 images)
            const Loader atVsync = android(release, kept, 3);
            assert(maxDelayFor(atVsync, false) == 2);
            assert(holds(atVsync, kept, false) >= 2);

            // Frames presented ahead ask for 6, and so do the ones capped by maxImageCount
            const Loader ahead = android(release, kept, 6);
            const int cap = maxDelayFor(ahead, true);
            assert(cap >= 3);
            assert(cap <= holds(ahead, kept, true));
        }
    }
    // A driver that caps the swapchain at 4 images while frames are presented ahead (release 11,
    // the compositor keeping one): 4 images, a delay of 2, as the cap that first handled it had
    Loader capped {30, 2, 4, 4};
    assert(maxDelayFor(capped, true) == 2);
    // Release 15 with the compositor keeping two: 6 images hold a delay of 3 for frames presented ahead
    assert(maxDelayFor(android(15, 2, 6), true) == 3);
    // A front buffer, an unknown release and nonsense still give a delay the scheduler takes
    assert(PresentScheduler::maxDelayFor(1, 1, 1, 34, false) == PresentScheduler::kMinDelay);
    assert(PresentScheduler::maxDelayFor(3, 3, 2, 0, false) == 2);
    assert(PresentScheduler::maxDelayFor(0, 0, 0, 35, true) == PresentScheduler::kMinDelay);

    // The 3-image swapchain keeps the delay of 2 frames start at, and misses don't take it past
    // what the swapchain holds
    {
        PresentScheduler scheduler;
        assert(scheduler.delayVsyncs() == 2);
        scheduler.setMaxDelay(maxDelayFor(android(14, 1, 3), false));
        assert(scheduler.delayVsyncs() == 2);
        assert(scheduler.maxDelay() == 2);
        presentLate(scheduler, 200);
        assert(scheduler.delayVsyncs() == 2);
    }

    // With room for it, the same misses raise the delay as far as it goes
    {
        PresentScheduler scheduler;
        scheduler.setMaxDelay(maxDelayFor(android(14, 1, 6), true));
        presentLate(scheduler, 200);
        assert(scheduler.delayVsyncs() == PresentScheduler::kMaxDelay);
    }

    // A swapchain that holds less than the current delay clamps it
    {
        PresentScheduler scheduler;
        scheduler.setMaxDelay(PresentScheduler::kMinDelay);
        assert(scheduler.delayVsyncs() == 1);
    }
    return 0;
}
