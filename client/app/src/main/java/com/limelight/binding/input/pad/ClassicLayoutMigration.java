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
 * this one, over to this controller once, so a player who arranged the old controls finds them where they were
 * instead of at the defaults. The classic controller kept one layout for all games and both orientations: each
 * control's box in pixels of the landscape screen and its own opacity, in the {@link #CLASSIC_PREFS} preferences.
 * Each box becomes the control's place and its free width and height, up to the whole screen, so a layout of
 * controls stretched over the screen comes out the same. Controls the classic layout does not have (the WASD and
 * pointer touchpads) keep their default place.
 *
 * <p>The classic A, B, X and Y were four buttons. Standing as the classic controller placed them, they become the
 * face cluster; moved, resized or faded apart, they become the cluster's four split members, each where the player
 * put it. The classic D-pad was one control and stays one.
 */
public final class ClassicLayoutMigration {
    /** The classic controller's saved layout: one JSON box per control, keyed by its element id. */
    public static final String CLASSIC_PREFS = "OSC";
    /**
     * Set in the controller's own preferences once the carry-over is done or moot; {@link ModernController#clearLayouts}
     * sets it too, so a reset means the defaults.
     */
    static final String MIGRATED = "classic-migrated";
    /**
     * Set when the retired Controller style was the classic one: the classic layout is the one in use, so it is carried
     * over once more even when an earlier carry-over set {@link #MIGRATED}. Cleared by that carry-over.
     */
    static final String CLASSIC_STYLE = "classic-style";

    // The classic controller's element ids (its VirtualControllerElement.EID_*), as its preferences stored them.
    private static final int DPAD = 1, LT = 2, RT = 3, LB = 4, RB = 5, A = 6, B = 7, X = 8, Y = 9,
            BACK = 10, START = 11, LS = 12, RS = 13, LSB = 14, RSB = 15, GUIDE = 16, TOUCHPAD = 17,
            L4 = 18, R4 = 19, L5 = 20, R5 = 21, LEFT_PAD = 22, RIGHT_PAD = 23, LEFT_GRIP = 24, RIGHT_GRIP = 25;
    private static final int LAST_ID = RIGHT_GRIP;
    /** A classic box: left, top, width and height in pixels, then the control's own opacity in percent. */
    static final int LEFT = 0, TOP = 1, WIDTH = 2, HEIGHT = 3, OPACITY = 4;
    /** The classic face buttons in {@link PadLayout.Control#memberIndex} order: bottom, right, left, top. */
    private static final int[] FACE_IDS = {A, B, X, Y};
    /**
     * With the Nintendo button layout on, the classic face buttons sent B at the bottom, A on the right, Y on the left
     * and X at the top: the member that sends what each position sent, by position.
     */
    private static final int[] SWAPPED = {1, 0, 3, 2};
    /** How far the classic face buttons may stand from their diamond and still be the cluster, in button sizes. */
    private static final float DIAMOND_SLACK = .25f, SIZE_SLACK = .2f;

    private ClassicLayoutMigration() {
    }

    /**
     * Runs once per install, when the controller is first built: the classic layout becomes the layout for all games
     * in every orientation that has no layout for all games yet, so a layout the player arranged in this controller
     * always wins. It is marked done whatever it found, so it never runs again, and a classic layout written later
     * is never read.
     *
     * <p>Older builds carried the classic layout over the first time a player tried this controller, and set
     * {@link #MIGRATED} with it. A player who went back to the classic controller ({@link #CLASSIC_STYLE}) kept
     * arranging it after that, so for them it runs once more and replaces that earlier carry-over's layout.
     *
     * @param store the controller's own preferences ({@link ModernController#PREFS})
     * @param swapFaces the Nintendo button layout setting, which the classic face buttons followed
     */
    static void carryOverOnce(Context context, SharedPreferences store, boolean swapFaces) {
        boolean earlier = store.getBoolean(MIGRATED, false), classicStyle = store.getBoolean(CLASSIC_STYLE, false);
        if (earlier && !classicStyle) return;
        SharedPreferences.Editor edit = store.edit().putBoolean(MIGRATED, true).remove(CLASSIC_STYLE);
        String shared = PadLayoutKeys.legacyKey(null);
        // The older shared key is both orientations' fallback (PadLayoutKeys); it reaches a player only in an
        // orientation without a shared layout of its own. Since this controller saves per orientation, only a
        // carry-over writes it, and it set the flag with it: with the flag already set, a layout there is an earlier
        // carry-over's, older than the classic layout in use.
        boolean stale = earlier && store.contains(shared);
        boolean free = (!store.contains(shared) || stale) &&
                !(store.contains(PadLayoutKeys.key(true, null)) && store.contains(PadLayoutKeys.key(false, null)));
        if (free) {
            if (stale) edit.remove(shared);
            Map<Integer, float[]> boxes = boxes(context.getSharedPreferences(CLASSIC_PREFS, Context.MODE_PRIVATE));
            if (!boxes.isEmpty()) {
                DisplayMetrics metrics = context.getResources().getDisplayMetrics();
                // The classic layout is arranged in landscape pixels; a first start in portrait must map it with the
                // same dimensions, because the shared key it is stored under is used in both orientations.
                float width = Math.max(metrics.widthPixels, metrics.heightPixels);
                float height = Math.min(metrics.widthPixels, metrics.heightPixels);
                PadLayout migrated = migrate(boxes, width, height, metrics.density, swapFaces);
                if (!migrated.isDefault()) edit.putString(shared, migrated.serialize());
            }
        }
        edit.apply();
    }

    /**
     * The retired Controller style was the classic controller: its layout is the one in use, so it is carried over
     * when the controller is next built, replacing what an earlier carry-over left.
     */
    public static void classicStyle(Context context) {
        context.getSharedPreferences(ModernController.PREFS, Context.MODE_PRIVATE).edit()
                .putBoolean(CLASSIC_STYLE, true).apply();
    }

    /**
     * Marks the carry-over done without it: a player who had picked this controller as their Controller style keeps
     * the layouts they have, or its defaults, rather than an older classic one.
     */
    public static void skip(Context context) {
        context.getSharedPreferences(ModernController.PREFS, Context.MODE_PRIVATE).edit()
                .putBoolean(MIGRATED, true).apply();
    }

    /**
     * The classic controls' saved boxes, each {left, top, width, height, opacity}; a layout saved before per-control
     * opacity is fully opaque, and a corrupt control is simply not carried over.
     */
    static Map<Integer, float[]> boxes(SharedPreferences classic) {
        Map<Integer, float[]> boxes = new HashMap<>();
        Map<String, ?> saved = classic.getAll();
        for (int id = DPAD; id <= LAST_ID; id++) {
            Object json = saved.get(String.valueOf(id));
            if (!(json instanceof String)) continue;
            try {
                JSONObject config = new JSONObject((String) json);
                boxes.put(id, new float[] {config.getInt("LEFT"), config.getInt("TOP"),
                        config.getInt("WIDTH"), config.getInt("HEIGHT"),
                        config.optInt("OPACITY", PadLayout.MAX_OPACITY)});
            } catch (JSONException ignored) {
                // Not carried over.
            }
        }
        return boxes;
    }

    /** {@link #migrate(Map, float, float, float, boolean)} with the face buttons as they stand. */
    public static PadLayout migrate(Map<Integer, float[]> boxes, float width, float height, float density) {
        return migrate(boxes, width, height, density, false);
    }

    /**
     * The layout that stands in for the classic one.
     *
     * @param boxes classic element id to {left, top, width, height[, opacity]} in pixels of the saved screen
     * @param width  the saved screen's width in pixels
     * @param height the saved screen's height in pixels
     * @param density the display's density, for the controls' minimum touch
     * @param swapFaces the Nintendo button layout was on, so each classic face button sent the other letter of its
     *                  pair; the button that sends that letter takes its place, so every place sends what it sent
     */
    public static PadLayout migrate(Map<Integer, float[]> boxes, float width, float height, float density,
                                    boolean swapFaces) {
        PadLayout layout = new PadLayout();
        if (boxes == null || width <= 0 || height <= 0) return layout;
        for (Map.Entry<Integer, float[]> entry : boxes.entrySet()) {
            PadLayout.Control control = control(entry.getKey());
            if (control != null && valid(entry.getValue())) {
                carry(layout, control, entry.getValue(), width, height, density);
            }
        }
        faces(layout, boxes, width, height, density, swapFaces);
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
            // The four face buttons become the cluster or its members; faces handles them.
            case A: case B: case X: case Y: return null;
            default: return null;
        }
    }

    private static boolean valid(float[] box) {
        return box != null && box.length >= 4 && box[WIDTH] > 0 && box[HEIGHT] > 0;
    }

    private static int opacity(float[] box) {
        return box.length > OPACITY ? Math.round(box[OPACITY]) : PadLayout.MAX_OPACITY;
    }

    /** The control takes the classic box as its place and size, the classic opacity, and shows. */
    private static void carry(PadLayout layout, PadLayout.Control control, float[] box, float width, float height,
                              float density) {
        layout.setBounds(control, box[LEFT], box[TOP], box[WIDTH], box[HEIGHT], width, height, density);
        layout.setOpacity(control, opacity(box));
        // A control the default leaves out is shown when the classic layout had it.
        layout.spec(control).hidden = false;
    }

    /**
     * The four classic face buttons. As the classic controller placed them (with the Nintendo button layout off), they
     * become the cluster over the box they span, so a hand that knew the old spacing finds it the same size. Otherwise
     * the cluster splits and each member takes its own button's box and opacity; a member whose button was not saved
     * sits over its place in the default cluster, as the classic controller showed an unsaved button at its default.
     */
    private static void faces(PadLayout layout, Map<Integer, float[]> boxes, float width, float height, float density,
                              boolean swap) {
        float[][] face = new float[FACE_IDS.length][];
        boolean any = false, all = true;
        for (int i = 0; i < FACE_IDS.length; i++) {
            float[] box = boxes.get(FACE_IDS[i]);
            face[i] = valid(box) ? box : null;
            any |= face[i] != null;
            all &= face[i] != null;
        }
        if (!any) return;
        if (all && !swap && classicDiamond(face)) {
            float left = Float.MAX_VALUE, top = Float.MAX_VALUE, right = -Float.MAX_VALUE, bottom = -Float.MAX_VALUE;
            for (float[] box : face) {
                left = Math.min(left, box[LEFT]);
                top = Math.min(top, box[TOP]);
                right = Math.max(right, box[LEFT] + box[WIDTH]);
                bottom = Math.max(bottom, box[TOP] + box[HEIGHT]);
            }
            carry(layout, PadLayout.Control.FACE, new float[] {left, top, right - left, bottom - top, opacity(face[0])},
                    width, height, density);
            return;
        }
        PadLayout.Control[] members = PadLayout.members(PadLayout.Control.FACE);
        for (int i = 0; i < face.length; i++) {
            PadLayout.Control member = members[swap ? SWAPPED[i] : i];
            if (face[i] != null) carry(layout, member, face[i], width, height, density);
            else layout.spec(member).hidden = false;
        }
        layout.spec(PadLayout.Control.FACE).hidden = true;
    }

    /**
     * Whether the four face buttons (bottom, right, left, top) still stand as the classic controller placed them: four
     * equal squares in a diamond, Y above A and X left of B, each one button's size from their common center, give or
     * take a quarter button, all at one opacity.
     */
    static boolean classicDiamond(float[][] face) {
        float size = 0;
        for (float[] box : face) size += box[WIDTH] + box[HEIGHT];
        size /= 2 * face.length;
        if (!(size > 0)) return false;
        float cx = 0, cy = 0;
        for (float[] box : face) {
            if (Math.abs(box[WIDTH] - size) > SIZE_SLACK * size || Math.abs(box[HEIGHT] - size) > SIZE_SLACK * size) {
                return false;
            }
            if (opacity(box) != opacity(face[0])) return false;
            cx += box[LEFT] + box[WIDTH] / 2;
            cy += box[TOP] + box[HEIGHT] / 2;
        }
        cx /= face.length;
        cy /= face.length;
        // Bottom, right, left and top of the common center, one button apart.
        float[][] offsets = {{0, 1}, {1, 0}, {-1, 0}, {0, -1}};
        for (int i = 0; i < face.length; i++) {
            float dx = face[i][LEFT] + face[i][WIDTH] / 2 - (cx + offsets[i][0] * size);
            float dy = face[i][TOP] + face[i][HEIGHT] / 2 - (cy + offsets[i][1] * size);
            if (Math.hypot(dx, dy) > DIAMOND_SLACK * size) return false;
        }
        return true;
    }
}
