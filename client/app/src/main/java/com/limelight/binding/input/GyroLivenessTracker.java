package com.limelight.binding.input;

import com.limelight.nvstream.jni.NightjarInputJni;

/**
 * Spots a controller gyroscope that delivers samples but no data: some controller clones list an
 * Android gyroscope whose values stay exactly zero while its timestamps advance. A real gyroscope
 * is noisy even at rest, so a full second of bit-for-bit zeros means the sensor is a phantom. A
 * gyroscope that has sent one nonzero sample since its listener was registered is real: a later
 * run of zeros (a driver that rounds a controller at rest to zero) never trips, since nothing
 * would switch it back.
 * Ported from Moonlight V+ (qiin2333/moonlight-vplus, GPL-3.0), ControllerGyroLivenessTracker.
 */
final class GyroLivenessTracker {
    static final long DEFAULT_ZERO_NANOS = 1_000_000_000L;

    private final long zeroNanos;
    // [0] = zeroRunStartNanos, [1] = sawMotion (0/1), [2] = tripped (0/1)
    private final long[] state = new long[3];

    GyroLivenessTracker() {
        this(DEFAULT_ZERO_NANOS);
    }

    GyroLivenessTracker(long zeroNanos) {
        this.zeroNanos = zeroNanos;
    }

    /** True once, for the sample that completes the zero run, if no sample before moved. */
    synchronized boolean onSample(float x, float y, float z, long timestampNanos) {
        return NightjarInputJni.gyroLivenessStep(zeroNanos, x, y, z, timestampNanos, state);
    }
}
