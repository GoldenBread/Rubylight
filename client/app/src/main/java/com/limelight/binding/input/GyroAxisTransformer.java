package com.limelight.binding.input;

import com.limelight.nvstream.jni.NightjarInputJni;

/**
 * Maps a controller's gyroscope axes onto the axes Moonlight sends (X pitch, Y yaw, Z roll)
 * and scales pitch and yaw, without changing units.
 *
 * Ported from jvanderv42's Nightjar fork (jvanderv42/nightjar, GPL-3.0, commit d0e14cf25,
 * app/src/main/java/com/limelight/binding/input/GyroAxisTransformer.java), with an
 * allocation-free transform and an identity check for the motion send paths.
 */
public final class GyroAxisTransformer {
    public static final float MIN_SENSITIVITY = 0.1f;
    public static final float MAX_SENSITIVITY = 3f;

    private static final GyroAxisTransformer IDENTITY = new GyroAxisTransformer(0, 1, 2, 1, 1, 1);

    final int[] sourceAxes;
    final int[] signs;
    final float horizontalSensitivity;
    final float verticalSensitivity;
    private final boolean identity;

    GyroAxisTransformer(int xAxis, int yAxis, int zAxis,
                        int xSign, int ySign, int zSign) {
        this(xAxis, yAxis, zAxis, xSign, ySign, zSign, 1f, 1f);
    }

    GyroAxisTransformer(int xAxis, int yAxis, int zAxis,
                        int xSign, int ySign, int zSign,
                        float horizontalSensitivity, float verticalSensitivity) {
        int res = NightjarInputJni.validateGyroMapping(
                xAxis, yAxis, zAxis, xSign, ySign, zSign,
                horizontalSensitivity, verticalSensitivity);
        int err = res & 0xFF;
        if (err == 1) {
            throw new IllegalArgumentException("Sensitivity must be between 0.10 and 3.00");
        }
        if (err == 2) {
            throw new IllegalArgumentException("Axes must be a permutation of 0, 1, 2");
        }
        if (err == 3) {
            throw new IllegalArgumentException("Axis signs must be +1 or -1");
        }
        sourceAxes = new int[] {xAxis, yAxis, zAxis};
        signs = new int[] {xSign, ySign, zSign};
        this.horizontalSensitivity = horizontalSensitivity;
        this.verticalSensitivity = verticalSensitivity;
        identity = (res & 0x100) != 0;
    }

    static boolean validSensitivity(float value) {
        return NightjarInputJni.isValidGyroSensitivity(value);
    }

    public static GyroAxisTransformer identity() {
        return IDENTITY;
    }

    /** True when {@link #transform} returns its input unchanged, so a caller can skip it. */
    public boolean isIdentity() {
        return identity;
    }

    /** Writes the mapped values into {@code out} (length 3), which may be reused across calls. */
    public void transform(float x, float y, float z, float[] out) {
        NightjarInputJni.transformGyro(
                x, y, z,
                sourceAxes[0], sourceAxes[1], sourceAxes[2],
                signs[0], signs[1], signs[2],
                horizontalSensitivity, verticalSensitivity, out);
    }

    float[] transform(float x, float y, float z) {
        float[] out = new float[3];
        transform(x, y, z, out);
        return out;
    }
}
