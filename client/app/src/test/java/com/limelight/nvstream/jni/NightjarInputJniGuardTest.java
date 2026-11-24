package com.limelight.nvstream.jni;

import com.limelight.nvstream.input.ControllerPacket;

import org.junit.Test;

import static org.junit.Assert.*;

/**
 * Verifies upfront JNI array-length guards in {@code policy_input/{slots,arrival,haptics_sensor}.rs}
 * never throw {@link ArrayIndexOutOfBoundsException} on short or empty Java arrays.
 */
public class NightjarInputJniGuardTest {
    @Test
    public void fuseControllerInputsShortPackedArrayReturnsDefaultWithoutException() {
        int[] out = new int[] {-1, -1, -1, -1, -1, -1, -1};
        // Requests count=2 (needs 12 ints), passes only 3 ints.
        NightjarInputJni.fuseControllerInputs(
                new int[] {0x10, 0, 100}, 2, false, (short) 0, 0.0f, false, out);
        assertEquals(0, out[0]);
        assertEquals(0, out[1]);
        assertEquals(0, out[2]);
        assertEquals(0, out[3]);
        assertEquals(0, out[4]);
        assertEquals(0, out[5]);
        assertEquals(0, out[6]);
    }

    @Test
    public void fuseControllerInputsOverflowCountDoesNotAllocateOrThrow() {
        int[] out = new int[] {-1, -1, -1, -1, -1, -1, -1};
        NightjarInputJni.fuseControllerInputs(
                new int[] {1, 2, 3, 4, 5, 6}, Integer.MAX_VALUE, false, (short) 0, 0.0f, false, out);
        assertEquals(0, out[0]);
    }

    @Test
    public void computeBaseSupportedButtonsShortKeyArraySafelyAppliesPrefixAndFlags() {
        int empty = NightjarInputJni.computeBaseSupportedButtons(
                new boolean[0], false, false, 0, false, false, false);
        assertEquals(0, empty);

        // First entry in ANDROID_TO_LI_BUTTON_MAP is A (0x1000).
        int shortPrefix = NightjarInputJni.computeBaseSupportedButtons(
                new boolean[] {true}, false, false, 0, false, false, true);
        assertTrue((shortPrefix & ControllerPacket.A_FLAG) != 0);
        assertTrue((shortPrefix & ControllerPacket.TOUCHPAD_FLAG) != 0);
    }

    @Test
    public void gyroLivenessStepShortStateArrayReturnsFalseWithoutException() {
        assertFalse(NightjarInputJni.gyroLivenessStep(1_000_000L, 0.0f, 0.0f, 0.0f, 2_000_000L, new long[0]));
        assertFalse(NightjarInputJni.gyroLivenessStep(1_000_000L, 0.0f, 0.0f, 0.0f, 2_000_000L, new long[2]));
    }
}
