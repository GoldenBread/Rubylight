package com.limelight.binding.input.pad;

import android.content.Context;
import android.content.SharedPreferences;
import android.util.DisplayMetrics;

import org.json.JSONException;
import org.json.JSONObject;

import java.util.HashMap;
import java.util.Map;

/**
 * Carries the layout of the classic on-screen controller, which earlier versions offered (and started with) beside
 * this one, over to this controller once, so a player who arranged the old controls finds them roughly where they
 * were instead of at the defaults. The classic controller kept one layout for all games and both orientations: each
 * control's box in pixels of the landscape screen, in the {@link #CLASSIC_PREFS} preferences. This maps those boxes
 * onto this controller's places and sizes. Controls the classic layout does not have (the WASD and pointer touchpads)
 * keep their default place.
 *
 * <p>An approximate mapping on purpose: the face cluster is one control where the classic had four buttons, and widths
 * are relative to this controller's unit, so the result is a starting point the editor can adjust.
 */
public final class ClassicLayoutMigration {
    /** The classic controller's saved layout: one JSON box per control, keyed by its element id. */
    public static final String CLASSIC_PREFS = "OSC";
    /**
     * Set in the controller's own preferences once the carry-over is done or moot; {@link ModernController#clearLayouts}
     * sets it too, so a reset means the defaults.
     */
    static final String MIGRATED = "classic-migrated";

    // The classic controller's element ids (its VirtualControllerElement.EID_*), as its preferences stored them.
    private static final int DPAD = 1, LT = 2, RT = 3, LB = 4, RB = 5, A = 6, B = 7, X = 8, Y = 9,
            BACK = 10, START = 11, LS = 12, RS = 13, LSB = 14, RSB = 15, GUIDE = 16, TOUCHPAD = 17,
            L4 = 18, R4 = 19, L5 = 20, R5 = 21, LEFT_PAD = 22, RIGHT_PAD = 23, LEFT_GRIP = 24, RIGHT_GRIP = 25;
    private static final int LAST_ID = RIGHT_GRIP;

    private ClassicLayoutMigration() {
    }

    /**
     * Runs once per install, when the controller is first built: the classic layout becomes the layout for all games
     * in every orientation that has no layout for all games yet, so a layout the player arranged in this controller
     * always wins. It is marked done whatever it found, so it never runs again, and a classic layout written later
     * is never read.
     *
     * @param store the controller's own preferences ({@link ModernController#PREFS})
     */
    static void carryOverOnce(Context context, SharedPreferences store) {
        if (store.getBoolean(MIGRATED, false)) return;
        SharedPreferences.Editor edit = store.edit().putBoolean(MIGRATED, true);
        // The older shared key is both orientations' fallback (PadLayoutKeys); it is free only while no shared layout
        // fills it, and it reaches a player only in an orientation without a shared layout of its own.
        boolean free = !store.contains(PadLayoutKeys.legacyKey(null)) &&
                !(store.contains(PadLayoutKeys.key(true, null)) && store.contains(PadLayoutKeys.key(false, null)));
        Map<Integer, float[]> boxes = free ?
                boxes(context.getSharedPreferences(CLASSIC_PREFS, Context.MODE_PRIVATE)) : new HashMap<>();
        if (!boxes.isEmpty()) {
            DisplayMetrics metrics = context.getResources().getDisplayMetrics();
            // The classic layout is arranged in landscape pixels; a first start in portrait must map it with the same
            // dimensions, because the shared key it is stored under is used in both orientations.
            float width = Math.max(metrics.widthPixels, metrics.heightPixels);
            float height = Math.min(metrics.widthPixels, metrics.heightPixels);
            PadLayout migrated = migrate(boxes, width, height, metrics.density);
            if (!migrated.isDefault()) edit.putString(PadLayoutKeys.legacyKey(null), migrated.serialize());
        }
        edit.apply();
    }

    /**
     * Marks the carry-over done without it: a player who had picked this controller as their Controller style keeps
     * the layouts they have, or its defaults, rather than an older classic one.
     */
    public static void skip(Context context) {
        context.getSharedPreferences(ModernController.PREFS, Context.MODE_PRIVATE).edit()
                .putBoolean(MIGRATED, true).apply();
    }

    /** The classic controls' saved boxes; a corrupt control is simply not carried over. */
    static Map<Integer, float[]> boxes(SharedPreferences classic) {
        Map<Integer, float[]> boxes = new HashMap<>();
        Map<String, ?> saved = classic.getAll();
        for (int id = DPAD; id <= LAST_ID; id++) {
            Object json = saved.get(String.valueOf(id));
            if (!(json instanceof String)) continue;
            try {
                JSONObject config = new JSONObject((String) json);
                boxes.put(id, new float[] {config.getInt("LEFT"), config.getInt("TOP"),
                        config.getInt("WIDTH"), config.getInt("HEIGHT")});
            } catch (JSONException ignored) {
                // Not carried over.
            }
        }
        return boxes;
    }

    /**
     * The layout that stands in for the classic one.
     *
     * @param boxes classic element id to {left, top, width, height} in pixels of the saved screen
     * @param width  the saved screen's width in pixels
     * @param height the saved screen's height in pixels
     * @param density the display's density, for the controls' minimum touch
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
            // A control the default leaves out is shown when the classic layout had it.
            layout.spec(control).hidden = false;
        }
        faceCluster(layout, boxes, width, height, density);
        return layout;
    }

    /** The control a classic element stands for; null for an id this version does not know. */
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
     * The four classic face buttons become the cluster: its center is theirs, and its size spans their outermost
     * edges, so a hand that knew the old spacing finds the cluster the same size.
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
