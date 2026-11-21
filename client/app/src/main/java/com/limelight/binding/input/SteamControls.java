package com.limelight.binding.input;

import com.limelight.nvstream.input.ControllerPacket;

/**
 * The on-screen controls a Steam Controller (2026) has and a phone's pad lacks: two trackpads,
 * the back buttons L4, R4, L5 and R5, and grip touch, for a PC whose controller type is the Steam
 * Controller. Flags and capability bits match {@code client/protocol/core/src/steam_controller.rs}.
 *
 * <p>The trackpads ride on touchpad 0, which such a PC splits into the left pad (x below 0.5) and
 * the right pad, each half stretched back to the pad's full width.
 */
public final class SteamControls {
    private SteamControls() {}

    /** Arrival capability: grip touch through the grip touch flags. */
    public static final short LI_CCAP_GRIP_SENSE = 0x200;
    /** Arrival capability: stick touch through the stick touch flags. */
    public static final short LI_CCAP_STICK_TOUCH = 0x400;
    /** Arrival capability: the controls send the pad and trigger clicks through the click flags. */
    public static final short LI_CCAP_SC26_BUTTONS = 0x1000;

    /** Held, not pressed: in the upper half of the button word. */
    public static final int LEFT_GRIP_TOUCH_FLAG = 0x0040_0000;
    public static final int RIGHT_GRIP_TOUCH_FLAG = 0x0080_0000;
    public static final int LEFT_STICK_TOUCH_FLAG = 0x0100_0000;
    public static final int RIGHT_STICK_TOUCH_FLAG = 0x0200_0000;
    /** The pads' own clicks and the triggers' end-stop clicks, pressed like buttons. */
    public static final int LEFT_PAD_CLICK_FLAG = 0x0400_0000;
    public static final int RIGHT_PAD_CLICK_FLAG = 0x0800_0000;
    public static final int LEFT_TRIGGER_CLICK_FLAG = 0x1000_0000;
    public static final int RIGHT_TRIGGER_CLICK_FLAG = 0x2000_0000;

    /** A trigger pulled this far clicks: the firmware's end-stop threshold, as sc26_usb.h has it. */
    public static final int TRIGGER_CLICK_THRESHOLD = 0xF0;

    /** The back buttons as the PC maps them: R4, L4, R5, L5 are paddles 1 to 4. */
    public static final int R4_FLAG = ControllerPacket.PADDLE1_FLAG;
    public static final int L4_FLAG = ControllerPacket.PADDLE2_FLAG;
    public static final int R5_FLAG = ControllerPacket.PADDLE3_FLAG;
    public static final int L5_FLAG = ControllerPacket.PADDLE4_FLAG;

    /** The buttons the controls add to their pad's arrival. */
    public static final int BUTTONS = R4_FLAG | L4_FLAG | R5_FLAG | L5_FLAG;

    /**
     * The capabilities the controls add to their pad's arrival. Not the touchpad: PCs that choose
     * a controller by its touchpad would make player 1 a PlayStation controller, which has none of
     * the back buttons or grips and which games that read only Xbox controllers cannot see. A PC
     * that makes the pad a Steam Controller takes the trackpads' touches without it. SC26 buttons
     * with it: such a PC then takes every click from the click flags alone instead of deriving a
     * trigger's from its pull, and the controls send every one it loses that way — the pads' from
     * a tap on them, a trigger's from a pull past {@link #TRIGGER_CLICK_THRESHOLD}.
     */
    public static final short CAPABILITIES =
            (short) (LI_CCAP_GRIP_SENSE | LI_CCAP_STICK_TOUCH | LI_CCAP_SC26_BUTTONS);

    public static final int LEFT_PAD = 0, RIGHT_PAD = 1;

    /** Finger ids of the two pads, apart from the small ids a physical pad's touchpad uses. */
    private static final int POINTER_BASE = 0x100;

    /** The largest x on the left pad: just short of the right pad's half. */
    private static final float LEFT_PAD_MAX_X = 0.4999f;

    /** The finger id a pad's touches carry. */
    public static int pointerId(int pad) {
        return POINTER_BASE + pad;
    }

    /** Touchpad 0's x for a point across a pad, 0 at its left edge and 1 at its right. */
    public static float touchX(int pad, float across) {
        float half = clamp(across) * 0.5f;
        return pad == LEFT_PAD ? Math.min(half, LEFT_PAD_MAX_X) : 0.5f + half;
    }

    /** Touchpad 0's y for a point down a pad, 0 at its top and 1 at its bottom. */
    public static float touchY(float down) {
        return clamp(down);
    }

    /**
     * The pressure every on-screen trackpad touch carries. Many screens report a constant 1.0 (a binary
     * pressure axis), and the host passes it on as the pad's full-scale pressure, which Steam reads as a pad
     * click on touch-down (#150). A light fixed pressure keeps a touch a touch; the click comes only from a
     * tap's click bit.
     */
    public static final float PAD_TOUCH_PRESSURE = 0.1f;

    /**
     * A physical pad's Touchpad button as the click of the pad its finger is on: the left pad's
     * for a touch left of the middle, otherwise the right pad's, which is also the answer when no
     * finger has touched yet (the driver's own click was the right pad's).
     */
    public static int touchpadClickFlag(float lastTouchX) {
        return lastTouchX < 0.5f ? LEFT_PAD_CLICK_FLAG : RIGHT_PAD_CLICK_FLAG;
    }

    /** The buttons with the Touchpad button, if held, also clicking the pad {@code lastTouchX} is on. */
    public static int withTouchpadClick(int buttons, float lastTouchX) {
        if ((buttons & ControllerPacket.TOUCHPAD_FLAG) == 0) return buttons;
        return buttons | touchpadClickFlag(lastTouchX);
    }

    private static float clamp(float value) {
        if (Float.isNaN(value)) return 0f;
        return Math.max(0f, Math.min(1f, value));
    }
}
