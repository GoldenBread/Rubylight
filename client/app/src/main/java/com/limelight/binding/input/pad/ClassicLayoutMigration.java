package com.limelight.binding.input.pad;

import java.util.Map;

/**
 * Carries a classic on-screen controller's saved layout over to the modern one the first time it is shown, so a
 * player who arranged the old controls finds them roughly where they were instead of at the modern defaults. The
 * classic layout stores each control's box in pixels; this maps those boxes onto the modern controls' places and
 * sizes. Controls the classic layout does not have (the WASD and pointer touchpads) keep their default place.
 *
 * <p>An approximate mapping on purpose: the modern face cluster is one control where the classic had four buttons,
 * and widths are relative to the modern unit, so the result is a starting point the editor can adjust.
 */
public final class ClassicLayoutMigration {
    // The classic controller's element ids (VirtualControllerElement.EID_*).
    private static final int DPAD = 1, LT = 2, RT = 3, LB = 4, RB = 5, A = 6, B = 7, X = 8, Y = 9,
            BACK = 10, START = 11, LS = 12, RS = 13, LSB = 14, RSB = 15, GUIDE = 16, TOUCHPAD = 17,
            L4 = 18, R4 = 19, L5 = 20, R5 = 21, LEFT_PAD = 22, RIGHT_PAD = 23, LEFT_GRIP = 24, RIGHT_GRIP = 25;

    private ClassicLayoutMigration() {
    }

    /**
     * The modern layout that stands in for the classic one.
     *
     * @param boxes classic element id to {left, top, width, height} in pixels of the saved screen
     * @param width  the saved screen's width in pixels
     * @param height the saved screen's height in pixels
     * @param density the display's density, for the modern controls' minimum touch
     */
    public static PadLayout migrate(Map<Integer, float[]> boxes, float width, float height, float density) {
        PadLayout layout = new PadLayout();
        if (boxes == null || width <= 0 || height <= 0) return layout;
        for (Map.Entry<Integer, float[]> entry : boxes.entrySet()) {
            PadLayout.Control control = control(entry.getKey());
            float[] box = entry.getValue();
            if (control == null || box == null || box.length < 4 || !(box[2] > 0) || !(box[3] > 0)) continue;
            place(layout, control, box[0] + box[2] / 2, box[1] + box[3] / 2, box[2], box[3],
                    width, height, density);
            // A control the modern default leaves out is shown when the classic layout had it.
            layout.spec(control).hidden = false;
        }
        faceCluster(layout, boxes, width, height, density);
        return layout;
    }

    /** The modern control a classic element stands for; null for an id this version does not know. */
    static PadLayout.Control control(int elementId) {
        switch (elementId) {
            case LS: return PadLayout.Control.LEFT_STICK;
            case RS: return PadLayout.Control.RIGHT_STICK;
            case DPAD: return PadLayout.Control.DPAD;
            case LT: return PadLayout.Control.LT;
            case RT: return PadLayout.Control.RT;
            case LB: return PadLayout.Control.LB;
            case RB: return PadLayout.Control.RB;
            case BACK: return PadLayout.Control.SELECT;
            case START: return PadLayout.Control.START;
            case LSB: return PadLayout.Control.L3;
            case RSB: return PadLayout.Control.R3;
            case GUIDE: return PadLayout.Control.HOME;
            case TOUCHPAD: return PadLayout.Control.TOUCHPAD;
            case L4: return PadLayout.Control.L4;
            case R4: return PadLayout.Control.R4;
            case L5: return PadLayout.Control.L5;
            case R5: return PadLayout.Control.R5;
            case LEFT_PAD: return PadLayout.Control.LEFT_PAD;
            case RIGHT_PAD: return PadLayout.Control.RIGHT_PAD;
            case LEFT_GRIP: return PadLayout.Control.LEFT_GRIP;
            case RIGHT_GRIP: return PadLayout.Control.RIGHT_GRIP;
            // The four face buttons are one cluster; faceCluster handles them.
            case A: case B: case X: case Y: return null;
            default: return null;
        }
    }

    /**
     * The four classic face buttons become the modern cluster: its center is theirs, and its size spans their
     * outermost edges, so a hand that knew the old spacing finds the new cluster the same size.
     */
    private static void faceCluster(PadLayout layout, Map<Integer, float[]> boxes, float width, float height,
                                    float density) {
        float left = Float.MAX_VALUE, top = Float.MAX_VALUE, right = -1, bottom = -1;
        for (int id : new int[] {A, B, X, Y}) {
            float[] box = boxes.get(id);
            if (box == null || box.length < 4 || !(box[2] > 0) || !(box[3] > 0)) continue;
            left = Math.min(left, box[0]);
            top = Math.min(top, box[1]);
            right = Math.max(right, box[0] + box[2]);
            bottom = Math.max(bottom, box[1] + box[3]);
        }
        if (right <= left || bottom <= top) return;
        float u = PadLayout.unit(width, height, density);
        float base = Math.max(PadLayout.width(PadLayout.Control.FACE, u, density, 1), 1);
        float scale = Math.max(right - left, bottom - top) / base;
        place(layout, PadLayout.Control.FACE, (left + right) / 2, (top + bottom) / 2,
                right - left, bottom - top, width, height, density);
        layout.setScale(PadLayout.Control.FACE, scale);
    }

    private static void place(PadLayout layout, PadLayout.Control control, float x, float y, float w, float h,
                              float width, float height, float density) {
        float u = PadLayout.unit(width, height, density);
        float base = Math.max(PadLayout.width(control, u, density, 1), 1);
        layout.setScale(control, Math.max(w, h) / base);
        layout.moveTo(control, x, y, width, height, density);
    }
}
