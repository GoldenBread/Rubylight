#pragma once

#include <stdint.h>

// The size of a new frame array for needed bytes when the capacity one is too small: half as
// large again, so frames that creep up in size (as a stream's do after its start) make a new
// ~1 MB Java array, and garbage of the old one, a few times rather than at every new largest
// frame. Java reads only the length each call passes. The Rust engine grows its array the same
// way (grown_capacity in protocol/android/src/renderers.rs).
static inline int32_t grownFrameCapacity(int32_t needed, int32_t capacity) {
    int64_t grown = (int64_t)capacity + capacity / 2;
    if (grown < needed) grown = needed;
    return grown > INT32_MAX ? INT32_MAX : (int32_t)grown;
}
