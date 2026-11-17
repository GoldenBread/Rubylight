package com.limelight.binding.input.pad;

import org.junit.Test;

import static org.junit.Assert.*;

public class PadVisibilityTest {
    private static final boolean ON = true, OFF = false, HERE = true, GONE = false, PAD_SENT = true, TOUCH_WITH_PAD = true;

    @Test public void controlsShowWhileTheirSwitchIsOnAndNoControllerHidesThem() {
        assertTrue(PadVisibility.shouldShow(ON, true, GONE, PAD_SENT, !TOUCH_WITH_PAD));
        assertTrue(PadVisibility.shouldShow(ON, false, HERE, PAD_SENT, !TOUCH_WITH_PAD));
        assertFalse(PadVisibility.shouldShow(ON, true, HERE, PAD_SENT, !TOUCH_WITH_PAD));
        assertFalse(PadVisibility.shouldShow(OFF, false, GONE, PAD_SENT, !TOUCH_WITH_PAD));
        assertFalse(PadVisibility.shouldShow(OFF, true, GONE, PAD_SENT, !TOUCH_WITH_PAD));
    }

    /** Only L3/R3 and the on-screen controls as their own player use the touch controls with a pad, so it must not hide them. */
    @Test public void aPadNeverHidesTouchControlsThatAreMeantToBeUsedWithIt() {
        assertTrue(PadVisibility.shouldShow(ON, true, HERE, PAD_SENT, TOUCH_WITH_PAD));
    }

    /** Controllers set to Off never reach the PC; the app ignores that pad, so it cannot take the touch controls away. */
    @Test public void aPadTheAppDoesNotSendKeepsTheTouchControls() {
        assertTrue(PadVisibility.shouldShow(ON, true, HERE, !PAD_SENT, !TOUCH_WITH_PAD));
    }

    @Test public void hidingStillNeedsTheSettingAndTheSwitch() {
        assertTrue("The hiding setting off never hides", PadVisibility.shouldShow(ON, false, HERE, PAD_SENT, TOUCH_WITH_PAD));
        assertFalse(PadVisibility.shouldShow(OFF, true, HERE, PAD_SENT, TOUCH_WITH_PAD));
    }
}
