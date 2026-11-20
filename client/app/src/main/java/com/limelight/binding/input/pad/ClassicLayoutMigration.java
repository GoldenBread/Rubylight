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
    /** The Nintendo button layout setting when the style was retired, which the classic controller then followed. */
    static final String CLASSIC_FLIP = "classic-flip";
    /**
     * Set once the retired Controller style has been read, at the first reading of the settings by this version, so
     * the decision is taken once; {@link ModernController#clearLayouts} keeps it.
     */
    static final String STYLE_RETIRED = "style-retired";
    /**
     * Where the carry-over keeps the layout for all games it replaced or dropped from the older shared key. Nothing
     * reads it back: it exists only so that layout can be recovered by hand.
     */
    static final String BACKUP = "layout-before-carry-over";

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
     * <p>For a player who was still using the classic controller ({@link #CLASSIC_STYLE}) the classic layout is the
     * one in use, so it replaces what the older shared key holds. That key is both orientations' fallback
     * (PadLayoutKeys), and it holds either a layout an earlier carry-over wrote the first time the player tried this
     * controller (which set {@link #MIGRATED} with it) or one the editors from Nightjar 5.4.1 up to 6.0 saved for all
     * games. Either is older than the classic layout the player went back to. What it held is kept under
     * {@link #BACKUP}. The layouts for all games saved per orientation since 6.0 are this controller's own and win.
     *
     * @param store the controller's own preferences ({@link ModernController#PREFS})
     * @param swapFaces the Nintendo button layout setting, which the classic face buttons followed
     */
    static void carryOverOnce(Context context, SharedPreferences store, boolean swapFaces) {
        boolean earlier = store.getBoolean(MIGRATED, false), classicStyle = store.getBoolean(CLASSIC_STYLE, false);
        if (earlier && !classicStyle) return;
        // A classic player's face buttons followed the setting as it was when the classic controller went away.
        if (classicStyle) swapFaces = store.getBoolean(CLASSIC_FLIP, swapFaces);
        SharedPreferences.Editor edit = store.edit().putBoolean(MIGRATED, true).remove(CLASSIC_STYLE)
                .remove(CLASSIC_FLIP);
        String shared = PadLayoutKeys.legacyKey(null);
        String landscapeKey = PadLayoutKeys.key(true, null), portraitKey = PadLayoutKeys.key(false, null);
        boolean ownLandscape = store.contains(landscapeKey), ownPortrait = store.contains(portraitKey);
        // With a layout of this controller's own in both orientations, the shared key reaches nobody.
        if (ownLandscape && ownPortrait || !classicStyle && store.contains(shared)) {
            edit.apply();
            return;
        }
        Map<Integer, float[]> boxes = boxes(context.getSharedPreferences(CLASSIC_PREFS, Context.MODE_PRIVATE));
        if (classicStyle && store.contains(shared)) {
            // Replaced or dropped, never lost.
            if (!store.contains(BACKUP)) edit.putString(BACKUP, store.getString(shared, null));
            edit.remove(shared);
        }
        DisplayMetrics metrics = context.getResources().getDisplayMetrics();
        float longSide = Math.max(metrics.widthPixels, metrics.heightPixels);
        float shortSide = Math.min(metrics.widthPixels, metrics.heightPixels);
        if (!boxes.isEmpty()) {
            // The classic layout is arranged in landscape pixels; a first start in portrait must map it with the
            // same dimensions, because the shared key it is stored under is used in both orientations.
            PadLayout migrated = migrate(boxes, longSide, shortSide, metrics.density, swapFaces);
            if (!migrated.isDefault()) edit.putString(shared, migrated.serialize());
        } else if (classicStyle && swapFaces && shortSide > 0) {
            // The classic controller followed the Nintendo button layout on screen even before it was arranged: its
            // own defaults put B at the bottom. This controller never did, so only a classic player gets it. Each
            // orientation has default places of its own, so each gets its layout, where it has none of its own.
            if (!ownLandscape) {
                edit.putString(landscapeKey, swappedDefaults(longSide, shortSide, metrics.density).serialize());
            }
            if (!ownPortrait) {
                edit.putString(portraitKey, swappedDefaults(shortSide, longSide, metrics.density).serialize());
            }
        }
        edit.apply();
    }

    /**
     * The default layout as the classic controller showed it with the Nintendo button layout on, on a surface this
     * size: the face cluster split, each place holding the member that sends what the classic button there sent (B at
     * the bottom, A on the right, Y on the left, X at the top), so every place still sends it.
     */
    static PadLayout swappedDefaults(float width, float height, float density) {
        PadLayout layout = new PadLayout();
        layout.split(PadLayout.Control.FACE, width, height, density);
        PadLayout.Control[] members = PadLayout.members(PadLayout.Control.FACE);
        float[][] places = new float[members.length][];
        for (int i = 0; i < members.length; i++) places[i] = layout.bounds(members[i], width, height, density, new float[4]);
        for (int i = 0; i < members.length; i++) {
            float[] place = places[i];
            layout.setBounds(members[SWAPPED[i]], place[0], place[1], place[2], place[3], width, height, density);
        }
        return layout;
    }

    /**
     * Takes the retired Controller style into account once, at the first reading of the settings by this version.
     * A player who had picked this controller keeps the layouts they have, or its defaults, rather than an older
     * classic one: the carry-over is marked done without it. Any other player used the classic controller, which was
     * also the style of an install that never stored one: their classic layout is carried over when the controller
     * is next built, replacing what an earlier carry-over left, with the face buttons as the Nintendo button layout
     * set them now. A fresh install reads its settings before anyone can turn that setting on, so it keeps this
     * controller's plain defaults.
     *
     * @param modern whether the stored style was this controller
     * @param flip the Nintendo button layout setting now
     */
    public static void retireStyle(Context context, boolean modern, boolean flip) {
        SharedPreferences store = context.getSharedPreferences(ModernController.PREFS, Context.MODE_PRIVATE);
        if (store.getBoolean(STYLE_RETIRED, false)) return;
        SharedPreferences.Editor edit = store.edit().putBoolean(STYLE_RETIRED, true);
        if (modern) {
            edit.putBoolean(MIGRATED, true);
        } else {
            edit.putBoolean(CLASSIC_STYLE, true).putBoolean(CLASSIC_FLIP, flip);
        }
        edit.apply();
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
