package com.limelight.binding.input;

import com.limelight.nvstream.jni.NightjarInputJni;

/**
 * Fixed-step pointer movement for controller mouse mode: while X is held, each report moves the
 * pointer a fixed number of pixels in the stick's direction; Y doubles the step, wrapping to 1.
 */
public final class FixedStepMouse {
    public static final int MIN_STEP = 1;
    public static final int MAX_STEP = 128;

    private FixedStepMouse() {}

    /** The step after one press of Y: 1, 2, 4 … 128, then 1 again. */
    public static int nextStep(int step) {
        return NightjarInputJni.fixedStepNext(step);
    }

    /**
     * Movement along one axis for a stick component already scaled to pixels. Components under
     * one pixel stay still, so a nearly horizontal push does not drift vertically.
     */
    public static int delta(double component, int step) {
        return NightjarInputJni.fixedStepDelta(component, step);
    }
}
