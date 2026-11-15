// The C engine's frame array grows half as large again, as the Rust engine's does: a stream
// whose frames creep up in size makes a new Java array a few times, not at every new largest frame.
#include "frame_capacity.h"
#include <assert.h>
#include <stdio.h>

int main(void) {
    // Frames from 32 KB up to about 1 MB (400 Mbps at 60 fps after the start), 1 KB larger each time.
    int32_t capacity = 32768;
    int arrays = 0;
    for (int32_t size = 32768; size <= 1000000; size += 1024) {
        if (capacity < size) {
            int32_t grown = grownFrameCapacity(size, capacity);
            assert(grown >= size && grown >= capacity + capacity / 2);
            capacity = grown;
            arrays++;
        }
    }
    // Growing to exactly each new size made about 950 arrays on this ramp.
    assert(arrays > 0 && arrays <= 10);

    // A frame larger than half again takes its own size; the size stays within a Java array's.
    assert(grownFrameCapacity(1000000, 32768) == 1000000);
    assert(grownFrameCapacity(INT32_MAX, 2000000000) == INT32_MAX);
    puts("frame capacity: all tests passed");
    return 0;
}
