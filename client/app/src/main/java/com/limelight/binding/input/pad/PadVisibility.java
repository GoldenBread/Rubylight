package com.limelight.binding.input.pad;

/**
 * Whether the on-screen controls belong on screen: their switch is on, and a connected controller hides them only
 * when the player has not arranged to use touch with that controller.
 */
public final class PadVisibility {
    private PadVisibility() {
    }

    /**
     * @param enabled     the Show on-screen controls setting
     * @param autoHide    Hide while a controller is connected
     * @param controllerHere an external controller is connected
     * @param padReachesHost the controller's input goes to the PC (Controllers is not Off); when it does not, the
     *                       app ignores the pad, so the touch controls must stay
     * @param touchWithPad   a setting deliberately uses the touch controls alongside a pad: Only L3/R3, or the
     *                       on-screen controls as their own player
     */
    public static boolean shouldShow(boolean enabled, boolean autoHide, boolean controllerHere, boolean padReachesHost,
                                     boolean touchWithPad) {
        return enabled && !(autoHide && controllerHere && padReachesHost && !touchWithPad);
    }
}
