package com.limelight.binding.input.pad;

/**
 * Where the modern on-screen controller keeps a layout: one for each orientation, for all games or one game. Older
 * versions kept a single layout per game, which is still read (and is what a landscape layout falls back to until it
 * has been saved once), so an edited layout is not lost when the app gains separate portrait and landscape ones.
 *
 * <p>Plain strings, so the keys are tested without a device.
 */
public final class PadLayoutKeys {
    /** The prefix of every stored modern layout, also the key of the oldest shared layout. */
    public static final String ROOT = "layout";
    public static final String PORTRAIT = "portrait", LANDSCAPE = "landscape";

    private PadLayoutKeys() {
    }

    /** The key of the layout for one orientation and game: "layout:portrait" or "layout:landscape:pc-1/app-7". */
    public static String key(boolean landscape, String gameKey) {
        StringBuilder key = new StringBuilder(ROOT).append(':').append(landscape ? LANDSCAPE : PORTRAIT);
        if (gameKey != null && !gameKey.isEmpty()) key.append(':').append(gameKey);
        return key.toString();
    }

    /** The key an older version stored, with no orientation: "layout" or "layout:pc-1/app-7". */
    public static String legacyKey(String gameKey) {
        return gameKey == null || gameKey.isEmpty() ? ROOT : ROOT + ':' + gameKey;
    }

    /** The key of the other orientation, for dropping a game's layouts when a shared one is saved. */
    public static String otherOrientationKey(boolean landscape, String gameKey) {
        return key(!landscape, gameKey);
    }

    /** The prefix of where the layout editor's panel was left, one for each orientation. Not a layout. */
    public static final String PANEL = "editor-panel";

    /** The key of where the editor's panel was left in one orientation: "editor-panel:portrait" or ":landscape". */
    public static String panelKey(boolean landscape) {
        return PANEL + ':' + (landscape ? LANDSCAPE : PORTRAIT);
    }

    /** Whether a stored key belongs to a modern layout this version writes, for clearing them all. */
    public static boolean ours(String key) {
        return key != null && (key.equals(ROOT) || key.startsWith(ROOT + ':'));
    }
}
