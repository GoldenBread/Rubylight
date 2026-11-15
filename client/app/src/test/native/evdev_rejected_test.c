// The root evdev reader remembers the input nodes it could not use, so its enumeration every
// second neither reopens nor logs them, until a node disappears or is recreated.
#include "../../main/jni/evdev_reader/evdev_rejected.h"
#include <assert.h>
#include <stdio.h>

static struct stat node(dev_t rdev, ino_t ino) {
    struct stat st;
    memset(&st, 0, sizeof(st));
    st.st_rdev = rdev;
    st.st_ino = ino;
    return st;
}

int main(void) {
    static struct RejectedNodes rejected;
    struct stat sensor = node(0xd40, 100), mouse = node(0xd41, 101);

    // First pass: neither is known, so both are opened; the sensor hub is no mouse or keyboard.
    rejectedNodesBeginPass(&rejected);
    assert(!rejectedNodesSkip(&rejected, "event0", &sensor));
    assert(!rejectedNodesSkip(&rejected, "event1", &mouse));
    rejectedNodesAdd(&rejected, "event0", &sensor);
    rejectedNodesEndPass(&rejected);

    // Every later pass skips it while it stays as it was.
    for (int pass = 0; pass < 3; pass++) {
        rejectedNodesBeginPass(&rejected);
        assert(rejectedNodesSkip(&rejected, "event0", &sensor));
        rejectedNodesEndPass(&rejected);
    }
    assert(rejected.count == 1);

    // Recreated under the same name (another device): tried again.
    struct stat replaced = node(0xd40, 200);
    rejectedNodesBeginPass(&rejected);
    assert(!rejectedNodesSkip(&rejected, "event0", &replaced));
    rejectedNodesAdd(&rejected, "event0", &replaced);
    rejectedNodesEndPass(&rejected);
    rejectedNodesBeginPass(&rejected);
    assert(rejectedNodesSkip(&rejected, "event0", &replaced));
    rejectedNodesEndPass(&rejected);

    // Gone from /dev/input: forgotten, so a node that comes back as it was is tried again.
    rejectedNodesBeginPass(&rejected);
    rejectedNodesEndPass(&rejected);
    assert(rejected.count == 0);
    rejectedNodesBeginPass(&rejected);
    assert(!rejectedNodesSkip(&rejected, "event0", &replaced));
    rejectedNodesEndPass(&rejected);

    // A node that cannot be opened is tried again until that failed EVDEV_OPEN_FAILURES passes in a
    // row: a hot-plugged node may refuse an open until ueventd sets its mode and label.
    struct stat plugged = node(0xd42, 102);
    for (int pass = 0; pass < EVDEV_OPEN_FAILURES - 1; pass++) {
        rejectedNodesBeginPass(&rejected);
        assert(!rejectedNodesSkip(&rejected, "event2", &plugged));
        rejectedNodesOpenFailed(&rejected, "event2", &plugged);
        rejectedNodesEndPass(&rejected);
    }
    // It opened (the mode was set): the failures start over.
    rejectedNodesBeginPass(&rejected);
    assert(!rejectedNodesSkip(&rejected, "event2", &plugged));
    rejectedNodesOpened(&rejected, "event2");
    rejectedNodesEndPass(&rejected);
    for (int pass = 0; pass < EVDEV_OPEN_FAILURES; pass++) {
        rejectedNodesBeginPass(&rejected);
        assert(!rejectedNodesSkip(&rejected, "event2", &plugged));
        rejectedNodesOpenFailed(&rejected, "event2", &plugged);
        rejectedNodesEndPass(&rejected);
    }
    // Failing that many passes in a row, it is skipped.
    rejectedNodesBeginPass(&rejected);
    assert(rejectedNodesSkip(&rejected, "event2", &plugged));
    rejectedNodesEndPass(&rejected);
    rejectedNodesBeginPass(&rejected);
    rejectedNodesEndPass(&rejected);
    assert(rejected.count == 0);

    // A full list remembers no more; those nodes are tried each pass as before.
    rejectedNodesBeginPass(&rejected);
    char name[16];
    for (int i = 0; i < EVDEV_MAX_REJECTED + 1; i++) {
        snprintf(name, sizeof(name), "event%d", i);
        struct stat st = node(0xd00 + i, 300 + i);
        rejectedNodesAdd(&rejected, name, &st);
    }
    rejectedNodesEndPass(&rejected);
    assert(rejected.count == EVDEV_MAX_REJECTED);
    struct stat last = node(0xd00 + EVDEV_MAX_REJECTED, 300 + EVDEV_MAX_REJECTED);
    snprintf(name, sizeof(name), "event%d", EVDEV_MAX_REJECTED);
    rejectedNodesBeginPass(&rejected);
    assert(!rejectedNodesSkip(&rejected, name, &last));
    puts("evdev rejected nodes: all tests passed");
    return 0;
}
