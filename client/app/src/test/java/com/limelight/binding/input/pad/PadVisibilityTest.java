package com.limelight.binding.input.pad;

import org.junit.Test;

import static org.junit.Assert.*;

public class PadVisibilityTest {
    @Test public void controlsShowWhileTheirSwitchIsOnAndNoControllerHidesThem() {
        assertTrue(PadVisibility.shouldShow(true, true, false));
        assertTrue(PadVisibility.shouldShow(true, false, true));
        assertFalse(PadVisibility.shouldShow(true, true, true));
        assertFalse(PadVisibility.shouldShow(false, false, false));
        assertFalse(PadVisibility.shouldShow(false, true, false));
    }
}
