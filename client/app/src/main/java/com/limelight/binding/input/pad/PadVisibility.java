package com.limelight.binding.input.pad;

/** Whether the on-screen controls belong on screen: their switch is on and no physical controller is hiding them. */
public final class PadVisibility {
    private PadVisibility() {
    }

    /**
     * @param enabled        the Show on-screen controls setting
     * @param autoHide       Hide while a controller is connected
     * @param controllerHere an external controller is connected
     */
    public static boolean shouldShow(boolean enabled, boolean autoHide, boolean controllerHere) {
        return enabled && !(autoHide && controllerHere);
    }
}
