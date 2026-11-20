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
     * Added to a layout's key for where a classic player's carry-over keeps the layout it replaced, dropped or swapped
     * ("layout-before-carry-over", "layout:landscape:pc-1/app-7-before-carry-over"). Nothing reads these back: they
     * exist only so those layouts can be recovered by hand. A reset of the layouts clears them with the rest.
     */
    static final String BACKUP_SUFFIX = "-before-carry-over";
    /** Where the layout for all games under the older shared key goes when it is replaced or dropped. */
    static final String BACKUP = PadLayoutKeys.ROOT + BACKUP_SUFFIX;

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
     * Runs once per install, when the controller is first built, and is marked done whatever it found, so a classic
     * layout written later is never read and a reset of the layouts ({@link ModernController#clearLayouts}) gives
     * the defaults.
     *
     * <p>A player still on the classic controller ({@link #CLASSIC_STYLE}) gets the arrangement they were using: the
     * classic layout (their boxes, or the defaults, with the face buttons swapped as the Nintendo button layout had
     * them) becomes the layout for all games in both orientations. It replaces every layout for all games this
     * controller holds, which they saved when they tried it: the older shared key (an earlier carry-over's, or one the
     * editors from Nightjar 5.4.1 up to 6.0 saved) and the per-orientation keys since 6.0. A layout they saved for one
     * game stays theirs; with the Nintendo button layout its face buttons are swapped, as the classic controller sent
     * them. Each layout replaced, dropped or swapped is kept under its key with {@link #BACKUP_SUFFIX}.
     *
     * <p>Any other first run (no retired style was read) carries the classic layout over only into an orientation
     * without a layout for all games, so a layout arranged in this controller wins.
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
        Map<Integer, float[]> boxes = boxes(context.getSharedPreferences(CLASSIC_PREFS, Context.MODE_PRIVATE));
        DisplayMetrics metrics = context.getResources().getDisplayMetrics();
        float longSide = Math.max(metrics.widthPixels, metrics.heightPixels);
        float shortSide = Math.min(metrics.widthPixels, metrics.heightPixels);
        if (classicStyle) {
            classicPlayer(store, edit, boxes, swapFaces, longSide, shortSide, metrics.density);
        } else {
            String shared = PadLayoutKeys.legacyKey(null);
            boolean free = !store.contains(shared) &&
                    !(store.contains(PadLayoutKeys.key(true, null)) && store.contains(PadLayoutKeys.key(false, null)));
            if (free && !boxes.isEmpty()) {
                // The classic layout is arranged in landscape pixels; a first start in portrait must map it with the
                // same dimensions, because the shared key it is stored under is used in both orientations.
                PadLayout migrated = migrate(boxes, longSide, shortSide, metrics.density, swapFaces);
                if (!migrated.isDefault()) edit.putString(shared, migrated.serialize());
            }
        }
        edit.apply();
    }

    /** The carry-over for a player still on the classic controller ({@link #carryOverOnce}). */
    private static void classicPlayer(SharedPreferences store, SharedPreferences.Editor edit, Map<Integer, float[]> boxes,
                                      boolean swapFaces, float longSide, float shortSide, float density) {
        String landscapeKey = PadLayoutKeys.key(true, null), portraitKey = PadLayoutKeys.key(false, null);
        Map<String, ?> saved = store.getAll();
        for (String key : new String[] {PadLayoutKeys.legacyKey(null), landscapeKey, portraitKey}) {
            backUp(saved, edit, key);
            edit.remove(key);
        }
        if (shortSide <= 0) return;
        if (!boxes.isEmpty()) {
            // The classic controller had one layout, in landscape pixels, for both orientations.
            PadLayout migrated = migrate(boxes, longSide, shortSide, density, swapFaces);
            if (!migrated.isDefault()) {
                edit.putString(landscapeKey, migrated.serialize()).putString(portraitKey, migrated.serialize());
            }
        } else if (swapFaces) {
            // The classic controller followed the Nintendo button layout on screen even before it was arranged: its
            // own defaults put B at the bottom. Each orientation has default places of its own.
            edit.putString(landscapeKey, swappedDefaults(longSide, shortSide, density).serialize());
            edit.putString(portraitKey, swappedDefaults(shortSide, longSide, density).serialize());
        }
        if (!swapFaces) return;
        // A layout for one game: its face buttons send what the classic ones there sent.
        String portraitGames = portraitKey + ':', landscapeGames = landscapeKey + ':';
        for (Map.Entry<String, ?> entry : saved.entrySet()) {
            String key = entry.getKey();
            if (!(entry.getValue() instanceof String) || !key.startsWith(PadLayoutKeys.ROOT + ':') ||
                    key.equals(landscapeKey) || key.equals(portraitKey) || key.endsWith(BACKUP_SUFFIX)) continue;
            String text = (String) entry.getValue();
            PadLayout layout = PadLayout.parse(text);
            // A game's portrait layout has portrait places; its landscape one, and the older layout for both
            // orientations, which landscape reads first, landscape places.
            boolean portrait = key.startsWith(portraitGames);
            float width = portrait ? shortSide : longSide, height = portrait ? longSide : shortSide;
            if (!swapFaceButtons(layout, width, height, density)) continue;
            backUp(saved, edit, key);
            edit.putString(key, layout.serialize());
            // The older layout is the game's portrait one too, until it has one of its own: portrait gets its copy,
            // swapped on portrait places. Nothing was under that key, so there is nothing to back up.
            if (portrait || key.startsWith(landscapeGames)) continue;
            String portraitGame = PadLayoutKeys.key(false, key.substring(PadLayoutKeys.ROOT.length() + 1));
            if (saved.containsKey(portraitGame)) continue;
            PadLayout copy = PadLayout.parse(text);
            swapFaceButtons(copy, shortSide, longSide, density);
            edit.putString(portraitGame, copy.serialize());
        }
    }

    /** Keeps what {@code key} holds under its backup key, unless an earlier backup is there. */
    private static void backUp(Map<String, ?> saved, SharedPreferences.Editor edit, String key) {
        Object value = saved.get(key);
        if (value instanceof String && !saved.containsKey(key + BACKUP_SUFFIX)) {
            edit.putString(key + BACKUP_SUFFIX, (String) value);
        }
    }

    /**
     * Swaps the face buttons as the classic controller's were with the Nintendo button layout on, on a surface this
     * size: each place holds the member that sends what the classic button there sent (B at the bottom, A on the
     * right, Y on the left, X at the top). A whole cluster is split where it stands; split members trade places.
     *
     * @return false when the face buttons are hidden or already in the Nintendo arrangement, so there is nothing to swap
     */
    static boolean swapFaceButtons(PadLayout layout, float width, float height, float density) {
        PadLayout.Control[] members = PadLayout.members(PadLayout.Control.FACE);
        // Already in the Nintendo arrangement: nothing to swap.
        if (layout.facesSwapped()) return false;
        if (layout.isSplit(PadLayout.Control.FACE)) {
            // SWAPPED trades A with B and X with Y, so each pair swaps its whole spec.
            for (int i = 0; i < members.length; i++) {
                int other = SWAPPED[i];
                if (other < i) continue;
                PadLayout.Spec a = layout.spec(members[i]), b = layout.spec(members[other]);
                PadLayout.Spec held = new PadLayout.Spec();
                held.copyFrom(a);
                a.copyFrom(b);
                b.copyFrom(held);
            }
            layout.setFacesSwapped(true);
            return true;
        }
        if (layout.spec(PadLayout.Control.FACE).hidden) return false;
        layout.split(PadLayout.Control.FACE, width, height, density);
        float[][] places = new float[members.length][];
        for (int i = 0; i < members.length; i++) places[i] = layout.bounds(members[i], width, height, density, new float[4]);
        for (int i = 0; i < members.length; i++) {
            float[] place = places[i];
            layout.setBounds(members[SWAPPED[i]], place[0], place[1], place[2], place[3], width, height, density);
        }
        layout.setFacesSwapped(true);
        return true;
    }

    /** The default layout as the classic controller showed it with the Nintendo button layout on, on this surface. */
    static PadLayout swappedDefaults(float width, float height, float density) {
        PadLayout layout = new PadLayout();
        swapFaceButtons(layout, width, height, density);
        return layout;
    }

    /**
     * Takes the retired Controller style into account once, at the first reading of the settings by this version.
     * A player who had picked this controller keeps the layouts they have, or its defaults, rather than an older
     * classic one: the carry-over is marked done without it. Any other player used the classic controller, which was
     * also the style of an install that never stored one: when the controller is next built, their classic
     * arrangement replaces the layouts for all games they saved when they tried this one, with the face buttons as
     * the Nintendo button layout set them now, which also swaps them in their layouts for one game. A fresh install reads its settings before anyone can turn that setting on, so it keeps this
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
        // The layout knows its arrangement from the start, so a later Join, preset or Add keeps B at the bottom.
        layout.setFacesSwapped(swapFaces);
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
