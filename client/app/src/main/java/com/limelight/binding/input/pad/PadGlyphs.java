package com.limelight.binding.input.pad;

/**
 * The face buttons' letters and the shoulders' names for each controller family. The positions never move: the
 * bottom button still sends A, the right one B, the left one X and the top one Y, as the PC's Xbox pad expects; only
 * the letter printed on them follows the chosen family, so a Nintendo player sees the pad their fingers know.
 */
public final class PadGlyphs {
    /** Face labels in {@link PadInput#FACE_CENTERS} order: bottom, right, left, top. */
    private static final String[][] FACE = {
            {"A", "B", "X", "Y"},             // Xbox
            {"\u2715", "\u25EF", "\u25A1", "\u25B3"}, // Cross, Circle, Square, Triangle
            {"B", "A", "Y", "X"},             // Nintendo: A on the right, B at the bottom
    };

    private PadGlyphs() {
    }

    /** The glyph on the face button at {@code index}: 0 bottom, 1 right, 2 left, 3 top. */
    public static String face(PadLayout.Glyph glyph, int index) {
        return FACE[glyph.ordinal()][index];
    }

    /**
     * The label on a shoulder or trigger pill: LB/RB/LT/RT, L1/R1/L2/R2, or L/R/ZL/ZR. Null for every other
     * pill, whose own name ({@code L4}, {@code L GRIP}) does not change with the family.
     */
    public static String shoulder(PadLayout.Control control, PadLayout.Glyph glyph) {
        switch (glyph) {
            case PLAYSTATION:
                switch (control) {
                    case LB: return "L1";
                    case RB: return "R1";
                    case LT: return "L2";
                    case RT: return "R2";
                    default: return null;
                }
            case NINTENDO:
                switch (control) {
                    case LB: return "L";
                    case RB: return "R";
                    case LT: return "ZL";
                    case RT: return "ZR";
                    default: return null;
                }
            default:
                switch (control) {
                    case LB: case RB: case LT: case RT: return control.name();
                    default: return null;
                }
        }
    }
}
