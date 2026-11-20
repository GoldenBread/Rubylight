package com.limelight.binding.input.pad;

import java.util.Locale;

/**
 * Where the modern on-screen controller's controls sit, how big they are, how opaque, and which are
 * hidden. A control the user never moved follows the default for the current surface, so a layout
 * nobody edited still fits after a rotation; a moved control keeps its center as a fraction of the
 * surface. Plain data and math, so persistence and geometry are tested without a device.
 */
public final class PadLayout {
    /** The controls, in drawing order. Shapes: round sticks and buttons, pill shoulders and triggers. */
    public enum Control {
        LEFT_STICK(Shape.STICK), RIGHT_STICK(Shape.STICK), DPAD(Shape.DPAD), FACE(Shape.FACE),
        LB(Shape.PILL), RB(Shape.PILL), LT(Shape.PILL), RT(Shape.PILL),
        L3(Shape.ROUND), R3(Shape.ROUND), SELECT(Shape.ROUND), START(Shape.ROUND), HOME(Shape.ROUND),
        /** A PlayStation pad's touchpad click, below the top row as on the pad itself. */
        TOUCHPAD(Shape.PILL),
        /** A stick that holds W, A, S and D instead of moving an axis. Not in the default layout: Show adds it. */
        WASD(Shape.STICK, true),
        /**
         * A Steam Deck-like touchpad that moves the PC's pointer, or the camera as a right stick, with tap, right-click
         * and two-finger scroll. Not in the default layout: the editor adds it.
         */
        POINTER_PAD(Shape.TOUCHPAD, true),
        /**
         * The face cluster's four buttons as controls of their own, for a player who splits the cluster to place and
         * size each one: A bottom, B right, X left and Y top in the cluster, as {@link PadInput#FACE_CENTERS}. Left
         * out of every layout until the editor's Split shows them in the cluster's place, and never shown together
         * with the cluster ({@link #shown}); an older build skips their names and keeps the rest of the layout.
         */
        A(Shape.FACE_BUTTON, true), B(Shape.FACE_BUTTON, true), X(Shape.FACE_BUTTON, true), Y(Shape.FACE_BUTTON, true),
        /** The D-pad's four directions as controls of their own, split as the face buttons are. */
        DPAD_UP(Shape.DPAD_ARM, true), DPAD_DOWN(Shape.DPAD_ARM, true), DPAD_LEFT(Shape.DPAD_ARM, true),
        DPAD_RIGHT(Shape.DPAD_ARM, true),
        /**
         * A Steam Controller's (2026) trackpads, back buttons and grip touch, as the classic controller offers them.
         * They show only with the Steam Controller controls setting; {@link ModernController#forcedHidden} keeps them
         * off otherwise.
         */
        LEFT_PAD(Shape.PAD, false, true), RIGHT_PAD(Shape.PAD, false, true),
        L4(Shape.PILL, false, true), L5(Shape.PILL, false, true), R4(Shape.PILL, false, true), R5(Shape.PILL, false, true),
        LEFT_GRIP(Shape.PILL, false, true), RIGHT_GRIP(Shape.PILL, false, true);

        public final Shape shape;
        /** Whether a layout nobody edited leaves this control out. */
        public final boolean hiddenByDefault;
        /** Whether this is one of the Steam Controller controls, shown only with their setting. */
        public final boolean steam;

        Control(Shape shape) {
            this(shape, false, false);
        }

        Control(Shape shape, boolean hiddenByDefault) {
            this(shape, hiddenByDefault, false);
        }

        Control(Shape shape, boolean hiddenByDefault, boolean steam) {
            this.shape = shape;
            this.hiddenByDefault = hiddenByDefault;
            this.steam = steam;
        }

        static Control named(String name) {
            for (Control control : values()) {
                if (control.name().equals(name)) return control;
            }
            return null;
        }

        /** The cluster a split member belongs to: FACE for A, B, X and Y, DPAD for its directions; null otherwise. */
        public Control cluster() {
            switch (shape) {
                case FACE_BUTTON: return FACE;
                case DPAD_ARM: return DPAD;
                default: return null;
            }
        }

        /**
         * A member's place among its cluster's four: bottom, right, left and top for the face buttons, as
         * {@link PadInput#FACE_CENTERS}; up, down, left and right for the D-pad. -1 for every other control.
         */
        public int memberIndex() {
            switch (this) {
                case A: case DPAD_UP: return 0;
                case B: case DPAD_DOWN: return 1;
                case X: case DPAD_LEFT: return 2;
                case Y: case DPAD_RIGHT: return 3;
                default: return -1;
            }
        }
    }

    /** The four members a cluster splits into, in {@link Control#memberIndex} order; none for any other control. */
    public static Control[] members(Control cluster) {
        if (cluster == Control.FACE) return new Control[] {Control.A, Control.B, Control.X, Control.Y};
        if (cluster == Control.DPAD) {
            return new Control[] {Control.DPAD_UP, Control.DPAD_DOWN, Control.DPAD_LEFT, Control.DPAD_RIGHT};
        }
        return new Control[0];
    }

    /**
     * {@code PAD} is a Steam Controller trackpad: a rounded square. {@code TOUCHPAD} is the pointer touchpad, wider than
     * tall. {@code FACE_BUTTON} is one face button of a split cluster, {@code DPAD_ARM} one direction of a split D-pad.
     */
    public enum Shape { STICK, DPAD, FACE, PILL, ROUND, PAD, TOUCHPAD, FACE_BUTTON, DPAD_ARM }

    /** The face letters and shoulder names printed on the controls. */
    public enum Glyph { XBOX, PLAYSTATION, NINTENDO }

    /** What the pointer touchpad moves: the PC's pointer, or the camera as the right stick. */
    public enum PointerMode { MOUSE, CAMERA }

    /** A ready-made selection of controls; {@link #applyPreset} places them at their defaults. */
    public enum Preset { DEFAULT, MINIMAL, FULL }

    /**
     * The smallest and the largest scale a control's width or height stores. Whatever the scale, a control is never
     * smaller than a full touch ({@link #MIN_TOUCH_DP}, {@link #MIN_PILL_WIDTH_DP}) and never wider or taller than
     * the surface ({@link #bounds}); the editor grows a control up to the whole surface, and MAX_SCALE only bounds
     * what a broken or hand-written layout can hold. Layouts saved before free resizing stayed within 0.5 to 2.
     */
    public static final float MIN_SCALE = .5f, MAX_SCALE = 100f;
    public static final int MIN_OPACITY = 10, MAX_OPACITY = 100;
    /**
     * The longest side of the default unit: a tablet's controls stop growing past a large phone's. It sets the
     * default sizes only; a control resized in the editor can still fill the whole surface.
     */
    static final float MAX_UNIT_DP = 440;
    /**
     * Layouts are written as v1, which an older build still reads: it skips the controls, glyphs and pointer mode it
     * does not know instead of dropping the whole layout, so a downgrade keeps every control it has. The short-lived
     * v2 prefix this version used is still read.
     */
    private static final String VERSION = "v1";
    private static final String VERSION_V2 = "v2";

    /**
     * One control's settings. {@code cx} and {@code cy} are fractions of the surface; NaN means the default place.
     *
     * <p>{@code widthScale} and {@code heightScale} size the control's box independently, each a multiple of its
     * default width or height ({@link #baseWidth} or {@link #baseHeight} times the {@link #unit}), so a control can
     * be resized to any shape up to the whole surface. A layout saved before free resizing had one scale for both
     * and loads with the two equal, so it keeps its size and shape. {@link #serialize} writes the larger as the one
     * scale an older build reads and adds the two as the seventh and eighth fields.
     */
    public static final class Spec {
        public float cx = Float.NaN, cy = Float.NaN, widthScale = 1, heightScale = 1;
        public int opacity = MAX_OPACITY;
        public boolean hidden;
        /** The control's own default for {@link #hidden}; a spec that matches it is not saved. */
        final boolean defaultHidden;

        public Spec() {
            this(false);
        }

        Spec(boolean defaultHidden) {
            this.defaultHidden = defaultHidden;
            hidden = defaultHidden;
        }

        public boolean placed() {
            return !Float.isNaN(cx) && !Float.isNaN(cy);
        }

        /** The larger of the two scales: the size the editor shows, and the one scale an older build reads. */
        public float scale() {
            return Math.max(widthScale, heightScale);
        }

        void copyFrom(Spec other) {
            cx = other.cx; cy = other.cy; widthScale = other.widthScale; heightScale = other.heightScale;
            opacity = other.opacity; hidden = other.hidden;
        }

        boolean isDefault() {
            return !placed() && widthScale == 1 && heightScale == 1 && opacity == MAX_OPACITY && hidden == defaultHidden;
        }
    }

    private final Spec[] specs = new Spec[Control.values().length];
    private Glyph glyph = Glyph.XBOX;
    private PointerMode pointerMode = PointerMode.MOUSE;

    public PadLayout() {
        for (Control control : Control.values()) specs[control.ordinal()] = new Spec(control.hiddenByDefault);
    }

    public Spec spec(Control control) {
        return specs[control.ordinal()];
    }

    public PadLayout copy() {
        PadLayout copy = new PadLayout();
        for (int i = 0; i < specs.length; i++) copy.specs[i].copyFrom(specs[i]);
        copy.glyph = glyph;
        copy.pointerMode = pointerMode;
        return copy;
    }

    public void copyFrom(PadLayout other) {
        for (int i = 0; i < specs.length; i++) specs[i].copyFrom(other.specs[i]);
        glyph = other.glyph;
        pointerMode = other.pointerMode;
    }

    public boolean isDefault() {
        if (glyph != Glyph.XBOX || pointerMode != PointerMode.MOUSE) return false;
        for (Spec spec : specs) {
            if (!spec.isDefault()) return false;
        }
        return true;
    }

    public void reset() {
        for (Spec spec : specs) spec.copyFrom(new Spec(spec.defaultHidden));
        glyph = Glyph.XBOX;
        pointerMode = PointerMode.MOUSE;
    }

    public Glyph glyph() {
        return glyph;
    }

    public void setGlyph(Glyph glyph) {
        this.glyph = glyph == null ? Glyph.XBOX : glyph;
    }

    public PointerMode pointerMode() {
        return pointerMode;
    }

    public void setPointerMode(PointerMode mode) {
        pointerMode = mode == null ? PointerMode.MOUSE : mode;
    }

    /**
     * Shows the controls a preset names, at their default places, sizes and opacity: Minimal keeps the sticks, the
     * D-pad, the face cluster and View and Menu; Full adds every other gamepad control and the pointer touchpad. Both
     * keep the face cluster and the D-pad whole. The
     * Steam Controller controls follow their own setting and stay as they are; the WASD stick, a keyboard control,
     * stays opt-in like the pointer touchpad.
     */
    public void applyPreset(Preset preset) {
        // A preset chooses which controls show; the glyph family and what the pointer touchpad does are the
        // player's other choices and stay as they are, so the editor's buttons do not visibly revert.
        Glyph keptGlyph = glyph;
        PointerMode keptPointerMode = pointerMode;
        Spec[] keptSteam = new Spec[specs.length];
        for (Control control : Control.values()) {
            if (!control.steam) continue;
            keptSteam[control.ordinal()] = new Spec(control.hiddenByDefault);
            keptSteam[control.ordinal()].copyFrom(spec(control));
        }
        reset();
        for (Control control : Control.values()) {
            if (control.steam) spec(control).copyFrom(keptSteam[control.ordinal()]);
        }
        glyph = keptGlyph;
        pointerMode = keptPointerMode;
        if (preset == Preset.DEFAULT) return;
        for (Control control : Control.values()) {
            if (control.steam) continue;
            boolean shown;
            switch (preset) {
                case MINIMAL:
                    shown = control == Control.LEFT_STICK || control == Control.RIGHT_STICK || control == Control.DPAD ||
                            control == Control.FACE || control == Control.SELECT || control == Control.START;
                    break;
                case FULL:
                    // Presets keep the face cluster and the D-pad whole; a split is the player's own arrangement.
                    shown = control != Control.WASD && control.cluster() == null;
                    break;
                default:
                    shown = !control.hiddenByDefault;
                    break;
            }
            spec(control).hidden = !shown;
        }
    }

    /** Gives the control one scale for its width and height: its default shape at {@code scale} times its size. */
    public void setScale(Control control, float scale) {
        setSize(control, scale, scale);
    }

    /** Sizes the control's width and height independently, each as a multiple of its default ({@link Spec}). */
    public void setSize(Control control, float widthScale, float heightScale) {
        Spec spec = spec(control);
        spec.widthScale = clampScale(widthScale);
        spec.heightScale = clampScale(heightScale);
    }

    private static float clampScale(float scale) {
        return Float.isNaN(scale) ? 1 : clamp(scale, MIN_SCALE, MAX_SCALE);
    }

    /**
     * Gives the control the box ({@code left}, {@code top}, {@code w}, {@code h}) in pixels of a {@code width} x
     * {@code height} surface: its center, and its width and height scales, each between a full touch and the
     * surface. The editor's corner handles size controls this way, and so can a layout carried over from pixel boxes.
     */
    public void setBounds(Control control, float left, float top, float w, float h, float width, float height,
                          float density) {
        float u = unit(width, height, density);
        setSize(control, Math.min(w, width) / Math.max(1e-6f, baseWidth(control) * u),
                Math.min(h, height) / Math.max(1e-6f, baseHeight(control) * u));
        moveTo(control, left + w / 2, top + h / 2, width, height, density);
    }

    /**
     * Scales the control from {@code widthScale} x {@code heightScale} by {@code factor} in both directions, keeping
     * its shape: the factor stops where a side would fall below {@link #MIN_SCALE} or grow past this surface.
     * Pinching and the editor's plus and minus buttons resize this way; the corner handles size each side alone.
     */
    public void scaleFrom(Control control, float widthScale, float heightScale, float factor, float width,
                          float height, float density) {
        float least = Math.max(MIN_SCALE / Math.max(1e-6f, widthScale), MIN_SCALE / Math.max(1e-6f, heightScale));
        float most = Float.MAX_VALUE;
        if (width > 0 && height > 0) {
            // As wide or as tall as the surface, whichever comes first; a surface not yet laid out sets no limit.
            float u = unit(width, height, density);
            most = Math.min(width / Math.max(1e-6f, baseWidth(control) * u * widthScale),
                    height / Math.max(1e-6f, baseHeight(control) * u * heightScale));
        }
        float f = Float.isNaN(factor) ? 1 : factor;
        // Where the two limits cross (a side at its smallest, another past a small surface), it can shrink, not grow.
        f = most >= least ? Math.max(least, Math.min(most, f)) : Math.min(f, 1);
        setSize(control, widthScale * f, heightScale * f);
    }

    /**
     * Whether the layout shows the control, the settings aside: not hidden, and for a split member, only while its
     * cluster is hidden, so a cluster and its members never show together.
     */
    public boolean shown(Control control) {
        if (spec(control).hidden) return false;
        Control cluster = control.cluster();
        return cluster == null || spec(cluster).hidden;
    }

    /** Whether a cluster is split: it is hidden and at least one of its members shows in its place. */
    public boolean isSplit(Control cluster) {
        if (!spec(cluster).hidden) return false;
        for (Control member : members(cluster)) {
            if (!spec(member).hidden) return true;
        }
        return false;
    }

    /**
     * Splits the face cluster or the D-pad into its four members: the cluster hides, and each member shows in its
     * button's direction from the cluster's centre on this {@code width} x {@code height} surface ({@link #memberBox}),
     * at that button's size (never below a full touch) and the cluster's opacity. {@link #join} undoes it.
     */
    public void split(Control cluster, float width, float height, float density) {
        Control[] members = members(cluster);
        if (members.length == 0) return;
        float[] box = bounds(cluster, width, height, density, new float[4]);
        float[][] boxes = memberBoxes(cluster, box[0], box[1], box[2], box[3], width, height, density);
        clearOfNeighbours(cluster, boxes, width, height, density);
        for (int i = 0; i < members.length; i++) {
            Control control = members[i];
            setBounds(control, boxes[i][0], boxes[i][1], boxes[i][2], boxes[i][3], width, height, density);
            spec(control).opacity = spec(cluster).opacity;
            spec(control).hidden = false;
        }
        spec(cluster).hidden = true;
    }

    /**
     * Moves a split cluster's member boxes together off the other controls shown, where spreading them carried one
     * over a neighbour: by the least move that clears every neighbour and keeps them on this {@code width} x
     * {@code height} surface. Where no single move does, they stay. The Steam Controller controls, which a setting
     * shows, do not move them, as they do not move the WASD stick ({@link #clearOfDefaults}).
     */
    private void clearOfNeighbours(Control cluster, float[][] boxes, float width, float height, float density) {
        java.util.List<float[]> others = new java.util.ArrayList<>();
        for (Control control : Control.values()) {
            if (control == cluster || control.cluster() == cluster || control.steam || !shown(control)) continue;
            others.add(bounds(control, width, height, density, new float[4]));
        }
        if (clear(boxes, 0, 0, others, width, height)) return;
        // Each move puts an overlapped neighbour's edge two pixels past the member's, against rounding.
        float bestX = 0, bestY = 0, best = Float.MAX_VALUE;
        for (float[] box : boxes) {
            for (float[] other : others) {
                if (!overlaps(box, 0, 0, other)) continue;
                float[][] moves = {{other[0] + other[2] - box[0] + 2, 0}, {other[0] - box[0] - box[2] - 2, 0},
                        {0, other[1] + other[3] - box[1] + 2}, {0, other[1] - box[1] - box[3] - 2}};
                for (float[] move : moves) {
                    float length = Math.abs(move[0]) + Math.abs(move[1]);
                    if (length < best && clear(boxes, move[0], move[1], others, width, height)) {
                        best = length;
                        bestX = move[0];
                        bestY = move[1];
                    }
                }
            }
        }
        if (best == Float.MAX_VALUE) return;
        for (float[] box : boxes) {
            box[0] += bestX;
            box[1] += bestY;
        }
    }

    /** Whether {@code boxes} moved by {@code dx}, {@code dy} stay on the surface and off every one of {@code others}. */
    private static boolean clear(float[][] boxes, float dx, float dy, java.util.List<float[]> others, float width,
                                 float height) {
        for (float[] box : boxes) {
            if (box[0] + dx < 0 || box[1] + dy < 0 || box[0] + dx + box[2] > width || box[1] + dy + box[3] > height) {
                return false;
            }
            for (float[] other : others) {
                if (overlaps(box, dx, dy, other)) return false;
            }
        }
        return true;
    }

    private static boolean overlaps(float[] box, float dx, float dy, float[] other) {
        return box[0] + dx < other[0] + other[2] && other[0] < box[0] + dx + box[2]
                && box[1] + dy < other[1] + other[3] && other[1] < box[1] + dy + box[3];
    }

    /** Joins a split cluster again: its members hide, and the cluster shows where and as it was before the split. */
    public void join(Control cluster) {
        for (Control member : members(cluster)) spec(member).hidden = true;
        spec(cluster).hidden = false;
    }

    /**
     * Where a split member sits around its cluster's box ({@code left}, {@code top}, {@code width}, {@code height}),
     * as left, top, width and height in {@code out}: in its button's direction from the cluster's centre, a face
     * button's circle or a D-pad arm as far as its pressed amber reaches, at that size but never below a full touch.
     * The members spread outward from the centre as far as they must so that no two face buttons overlap, since one
     * finger would press both where they did: each sits at least its whole size, and two pixels against rounding, off
     * the centre lines. D-pad neighbours keep only their inner corner in common, in the cluster's proportions to the
     * arm's thickness, so a finger there holds the diagonal; opposite arms never meet, and the middle holds nothing.
     * {@link #memberBoxes} keeps the four on the surface together.
     */
    static void memberBox(Control member, float left, float top, float width, float height, float density, float[] out) {
        float size = Math.min(width, height), touch = MIN_TOUCH_DP * density;
        float middleX = left + width / 2, middleY = top + height / 2;
        float cx = middleX, cy = middleY, w, h;
        if (member.shape == Shape.FACE_BUTTON) {
            w = h = Math.max(2 * PadInput.FACE_RADIUS * size, touch);
            float[] center = PadInput.FACE_CENTERS[member.memberIndex()];
            float dx = (center[0] - .5f) * width, dy = (center[1] - .5f) * height, least = w + 2;
            if (dx != 0) cx += Math.signum(dx) * Math.max(Math.abs(dx), least);
            if (dy != 0) cy += Math.signum(dy) * Math.max(Math.abs(dy), least);
        } else {
            float arm = PadInput.DPAD_ARM * size, corner = PadInput.DPAD_CORNER * size;
            boolean vertical = member == Control.DPAD_UP || member == Control.DPAD_DOWN;
            float length = ((vertical ? height : width) - arm) / 2 + corner;
            float thick = Math.max(arm, touch), along = Math.max(length, touch);
            // The inner end reaches past the crossing arm's edge by the corner, grown with the arm.
            float inner = thick / 2 - corner * thick / Math.max(1e-6f, arm);
            w = vertical ? thick : along;
            h = vertical ? along : thick;
            if (member == Control.DPAD_UP) cy = middleY - inner - along / 2;
            else if (member == Control.DPAD_DOWN) cy = middleY + inner + along / 2;
            else if (member == Control.DPAD_LEFT) cx = middleX - inner - along / 2;
            else cx = middleX + inner + along / 2;
        }
        out[0] = cx - w / 2;
        out[1] = cy - h / 2;
        out[2] = w;
        out[3] = h;
    }

    /**
     * The boxes of {@code cluster}'s members ({@link #memberBox}) in {@link #members} order, moved together where
     * spreading them would carry one off this {@code surfaceWidth} x {@code surfaceHeight} surface, so they keep their
     * arrangement.
     */
    static float[][] memberBoxes(Control cluster, float left, float top, float width, float height, float surfaceWidth,
                                 float surfaceHeight, float density) {
        Control[] members = members(cluster);
        float[][] boxes = new float[members.length][4];
        float minX = Float.MAX_VALUE, minY = Float.MAX_VALUE, maxX = -Float.MAX_VALUE, maxY = -Float.MAX_VALUE;
        for (int i = 0; i < members.length; i++) {
            float[] box = boxes[i];
            memberBox(members[i], left, top, width, height, density, box);
            minX = Math.min(minX, box[0]);
            minY = Math.min(minY, box[1]);
            maxX = Math.max(maxX, box[0] + box[2]);
            maxY = Math.max(maxY, box[1] + box[3]);
        }
        float dx = onto(minX, maxX, surfaceWidth), dy = onto(minY, maxY, surfaceHeight);
        for (float[] box : boxes) {
            box[0] += dx;
            box[1] += dy;
        }
        return boxes;
    }

    /** How far to move a span from {@code min} to {@code max} onto 0 to {@code size}: centred if it is longer. */
    private static float onto(float min, float max, float size) {
        if (size <= 0) return 0;
        if (max - min >= size) return (size - min - max) / 2;
        if (min < 0) return -min;
        return max > size ? size - max : 0;
    }

    public void setOpacity(Control control, int opacity) {
        spec(control).opacity = Math.max(MIN_OPACITY, Math.min(MAX_OPACITY, opacity));
    }

    /** Puts the control's center at ({@code x}, {@code y}) pixels, kept so the whole control stays on the surface. */
    public void moveTo(Control control, float x, float y, float width, float height, float density) {
        float[] box = bounds(control, width, height, density, new float[4]);
        float halfW = box[2] / 2, halfH = box[3] / 2;
        Spec spec = spec(control);
        spec.cx = clamp(x, halfW, Math.max(halfW, width - halfW)) / Math.max(1, width);
        spec.cy = clamp(y, halfH, Math.max(halfH, height - halfH)) / Math.max(1, height);
    }

    /**
     * The default size unit: the short side of the surface, capped for tablets, and narrow enough
     * that the left and right clusters (about 1.75 units together) never meet on a portrait phone.
     */
    static float unit(float width, float height, float density) {
        return Math.min(Math.min(Math.min(width, height), MAX_UNIT_DP * density), width / 1.75f);
    }

    /** Width and height of a control at scale 1, in units. */
    static float baseWidth(Control control) {
        switch (control.shape) {
            // The WASD stick takes the right stick's size so its default place fits between the clusters.
            case STICK: return control == Control.LEFT_STICK ? .42f : .34f;
            case DPAD: return DPAD_BASE;
            case FACE: return FACE_BASE;
            case PILL: return .25f;
            case PAD: return .22f;
            case TOUCHPAD: return .30f;
            // A split member's box at scale 1 is its button in the default-sized cluster (memberBox).
            case FACE_BUTTON: return FACE_BASE * 2 * PadInput.FACE_RADIUS;
            case DPAD_ARM: return control == Control.DPAD_UP || control == Control.DPAD_DOWN ? DPAD_BASE * PadInput.DPAD_ARM :
                    DPAD_BASE * ((1 - PadInput.DPAD_ARM) / 2 + PadInput.DPAD_CORNER);
            default: return control == Control.L3 || control == Control.R3 ? .13f : .11f;
        }
    }

    static float baseHeight(Control control) {
        if (control.shape == Shape.PILL) return .12f;
        if (control.shape == Shape.TOUCHPAD) return .19f;
        if (control.shape == Shape.DPAD_ARM) {
            return control == Control.DPAD_UP || control == Control.DPAD_DOWN ?
                    DPAD_BASE * ((1 - PadInput.DPAD_ARM) / 2 + PadInput.DPAD_CORNER) : DPAD_BASE * PadInput.DPAD_ARM;
        }
        return baseWidth(control);
    }

    /** The D-pad's and the face cluster's side at scale 1, in units. */
    private static final float DPAD_BASE = .30f, FACE_BASE = .40f;

    /**
     * Width and height of a control in pixels at {@code scale}: never below a 48 dp touch, and the
     * pill shoulders and triggers never narrower than 72 dp, whatever the surface or the scale.
     */
    static float width(Control control, float u, float density, float scale) {
        return Math.max(baseWidth(control) * u * scale,
                (control.shape == Shape.PILL ? MIN_PILL_WIDTH_DP : MIN_TOUCH_DP) * density);
    }

    static float height(Control control, float u, float density, float scale) {
        return Math.max(baseHeight(control) * u * scale, MIN_TOUCH_DP * density);
    }

    /** The smallest touch a control offers, and the narrowest a pill gets so it still reads as one. */
    static final float MIN_TOUCH_DP = 48, MIN_PILL_WIDTH_DP = 72;

    /** How far below the top row the stream's microphone and menu buttons reach: their 12 dp margin and 48 dp size. */
    static final float TOP_BUTTONS_DP = 12 + 48;

    /**
     * The default center of a control, in pixels, from the nearest edges of the surface. Edge controls
     * keep their margins from their own size, so a control grown to its minimum touch on a small
     * surface still clears its neighbours: the shoulder pair, the top row and the stick clicks below
     * the clusters.
     */
    static void defaultCenter(Control control, float width, float height, float u, float density, float[] out) {
        float pillW = width(Control.LT, u, density, 1), pillH = height(Control.LT, u, density, 1);
        float round = width(Control.HOME, u, density, 1), click = width(Control.L3, u, density, 1);
        float rowStep = Math.max(.17f * u, round + 8 * density);
        float clickTop = height - .035f * u - click - .02f * u;
        float x, y;
        switch (control) {
            case LEFT_STICK:
                x = .28f * u;
                y = Math.min(height - .44f * u, clickTop - height(control, u, density, 1) / 2);
                break;
            case DPAD: x = .66f * u; y = height - .17f * u; break;
            case FACE:
                x = width - .27f * u;
                y = Math.min(height - .44f * u, clickTop - height(control, u, density, 1) / 2);
                break;
            case RIGHT_STICK: x = width - .68f * u; y = height - .19f * u; break;
            case LT: x = .045f * u + pillW / 2; y = .04f * u + pillH / 2; break;
            case LB: x = .045f * u + pillW / 2; y = .07f * u + pillH * 1.5f; break;
            case RT: x = width - .045f * u - pillW / 2; y = .04f * u + pillH / 2; break;
            case RB: x = width - .045f * u - pillW / 2; y = .07f * u + pillH * 1.5f; break;
            case L3: x = .035f * u + click / 2; y = height - .035f * u - click / 2; break;
            case R3: x = width - .035f * u - click / 2; y = height - .035f * u - click / 2; break;
            case SELECT: x = width / 2 - rowStep; y = .035f * u + round / 2; break;
            case START: x = width / 2 + rowStep; y = .035f * u + round / 2; break;
            case WASD: {
                // Between the clusters and below the touchpad, clear of every default control until it is moved, and
                // of the microphone and menu buttons, which sit below the top row. Where the clusters leave no room
                // below the buttons (16:10 and short 16:9 screens), it keeps its spot right below the touchpad;
                // the buttons can be dragged.
                float touchpadBottom = .035f * u + round + Math.max(.02f * u, 8 * density) + pillH;
                float buttonsBottom = touchpadBottom + TOP_BUTTONS_DP * density;
                float size = height(control, u, density, 1);
                x = width / 2;
                y = Math.max(height / 2, buttonsBottom + 8 * density + size / 2);
                if (!clearOfDefaults(control, x, y, size, width, height, u, density)) {
                    y = Math.max(height / 2, touchpadBottom + 8 * density + size / 2);
                }
                break;
            }
            case TOUCHPAD: x = width / 2; y = .035f * u + round + Math.max(.02f * u, 8 * density) + pillH / 2; break;
            case HOME: x = width / 2; y = .035f * u + round / 2; break;
            case POINTER_PAD: {
                // The middle of the surface, where a phone held in two hands leaves the thumb room, but never under the
                // stream's microphone and menu buttons, which sit below the top row: on a short surface (a phone in
                // landscape) it moves down below them, as far as it stays on the surface. Both it and the WASD stick
                // are left out of the default layout; with both added, one gets moved.
                float touchpadBottom = .035f * u + round + Math.max(.02f * u, 8 * density) + pillH;
                float buttonsBottom = touchpadBottom + TOP_BUTTONS_DP * density;
                float size = height(control, u, density, 1);
                x = width / 2;
                y = Math.min(Math.max(height / 2, buttonsBottom + 8 * density + size / 2), height - size / 2);
                break;
            }
            case A: case B: case X: case Y:
            case DPAD_UP: case DPAD_DOWN: case DPAD_LEFT: case DPAD_RIGHT: {
                // A split member never placed (the editor's Split always places them) sits where Split puts it from
                // the default cluster.
                Control cluster = control.cluster();
                defaultCenter(cluster, width, height, u, density, out);
                float w = width(cluster, u, density, 1), h = height(cluster, u, density, 1);
                float[][] boxes = memberBoxes(cluster, out[0] - w / 2, out[1] - h / 2, w, h, width, height, density);
                Control[] members = members(cluster);
                float[] member = boxes[0];
                for (int i = 0; i < members.length; i++) {
                    if (members[i] == control) member = boxes[i];
                }
                x = member[0] + member[2] / 2;
                y = member[1] + member[3] / 2;
                break;
            }
            default: {
                // A Steam Controller control: its place comes from the arrangement the surface has room for.
                float[] centers = steamCenters(width, height, u, density);
                int i = control.ordinal() - STEAM_FIRST;
                x = centers[2 * i];
                y = centers[2 * i + 1];
                break;
            }
        }
        out[0] = x;
        out[1] = y;
    }

    /** Whether a square {@code size} control centred at x, y stays off every other control at its default place. */
    private static boolean clearOfDefaults(Control self, float x, float y, float size, float width, float height,
                                           float u, float density) {
        if (y + size / 2 > height) return false;
        float[] center = new float[2];
        for (Control other : Control.values()) {
            // The controls the default layout leaves out, and the Steam Controller controls a setting keeps off, do not
            // push the WASD stick from its place; when both optional middle controls are shown, one gets moved.
            if (other == self || other.steam || other.hiddenByDefault) continue;
            defaultCenter(other, width, height, u, density, center);
            float w = width(other, u, density, 1), h = height(other, u, density, 1);
            if (Math.abs(center[0] - x) < (w + size) / 2 && Math.abs(center[1] - y) < (h + size) / 2) return false;
        }
        return true;
    }

    private static final int STEAM_FIRST = Control.LEFT_PAD.ordinal(), STEAM_COUNT = Control.values().length - STEAM_FIRST;

    /**
     * The default centers of the Steam Controller controls, x and y for each in {@link Control} order. Three
     * arrangements are tried, each first below the stream's microphone and menu buttons and then right below the top
     * row, and the first that keeps every one on the surface and off every default control and each other wins:
     * <ol>
     * <li>wide surfaces: the back buttons inward of the shoulders and triggers, the trackpads side by side in the
     * middle with a grip outside each;</li>
     * <li>narrower ones: the same, with the two grips stacked between the trackpads;</li>
     * <li>tall ones (portrait): rows of L4 and R4, L5 and R5 and the grips in the middle, the trackpads below.</li>
     * </ol>
     * Where none fits (the smallest phones), the first stands; every control can be moved, resized or hidden.
     */
    static float[] steamCenters(float width, float height, float u, float density) {
        float[] centers = new float[STEAM_COUNT * 2];
        for (boolean belowButtons : new boolean[] {true, false}) {
            for (int arrangement = 0; arrangement < 3; arrangement++) {
                steamArrangement(arrangement, belowButtons, width, u, density, centers);
                if (steamClear(centers, width, height, u, density)) return centers;
            }
        }
        steamArrangement(0, false, width, u, density, centers);
        return centers;
    }

    private static void steamArrangement(int arrangement, boolean belowButtons, float width, float u, float density,
                                         float[] out) {
        float pillW = width(Control.L4, u, density, 1), pillH = height(Control.L4, u, density, 1);
        float pad = width(Control.LEFT_PAD, u, density, 1), round = width(Control.HOME, u, density, 1);
        float gap = Math.max(.02f * u, 8 * density);
        // Below the touchpad's default place, and below the microphone and menu buttons that sit under it.
        float top = .035f * u + round + gap + pillH + (belowButtons ? TOP_BUTTONS_DP * density : 0);
        float backX = .045f * u + pillW + gap + pillW / 2, triggerY = .04f * u + pillH / 2, shoulderY = .07f * u + pillH * 1.5f;
        if (arrangement < 2) {
            steamAt(out, Control.L4, backX, triggerY);
            steamAt(out, Control.L5, backX, shoulderY);
            steamAt(out, Control.R4, width - backX, triggerY);
            steamAt(out, Control.R5, width - backX, shoulderY);
        }
        if (arrangement == 0) {
            float cy = top + gap + pad / 2, gripX = width / 2 - pad - gap / 2 - gap - pillW / 2;
            steamAt(out, Control.LEFT_PAD, width / 2 - pad / 2 - gap / 2, cy);
            steamAt(out, Control.RIGHT_PAD, width / 2 + pad / 2 + gap / 2, cy);
            steamAt(out, Control.LEFT_GRIP, gripX, cy);
            steamAt(out, Control.RIGHT_GRIP, width - gripX, cy);
        } else if (arrangement == 1) {
            float cy = top + gap + Math.max(pad, 2 * pillH + gap) / 2, offset = pillW / 2 + gap + pad / 2;
            steamAt(out, Control.LEFT_PAD, width / 2 - offset, cy);
            steamAt(out, Control.RIGHT_PAD, width / 2 + offset, cy);
            steamAt(out, Control.LEFT_GRIP, width / 2, cy - gap / 2 - pillH / 2);
            steamAt(out, Control.RIGHT_GRIP, width / 2, cy + gap / 2 + pillH / 2);
        } else {
            float left = width / 2 - gap / 2 - pillW / 2, right = width / 2 + gap / 2 + pillW / 2;
            float y = top + gap + pillH / 2;
            steamAt(out, Control.L4, left, y);
            steamAt(out, Control.R4, right, y);
            y += pillH + gap;
            steamAt(out, Control.L5, left, y);
            steamAt(out, Control.R5, right, y);
            y += pillH + gap;
            steamAt(out, Control.LEFT_GRIP, left, y);
            steamAt(out, Control.RIGHT_GRIP, right, y);
            y += pillH / 2 + gap + pad / 2;
            steamAt(out, Control.LEFT_PAD, width / 2 - pad / 2 - gap / 2, y);
            steamAt(out, Control.RIGHT_PAD, width / 2 + pad / 2 + gap / 2, y);
        }
    }

    private static void steamAt(float[] out, Control control, float x, float y) {
        int i = control.ordinal() - STEAM_FIRST;
        out[2 * i] = x;
        out[2 * i + 1] = y;
    }

    /** Whether the Steam Controller controls at these centers lie on the surface, apart from each other and every default control. */
    private static boolean steamClear(float[] centers, float width, float height, float u, float density) {
        Control[] controls = Control.values();
        float[] center = new float[2];
        for (int i = 0; i < STEAM_COUNT; i++) {
            Control control = controls[STEAM_FIRST + i];
            float w = width(control, u, density, 1), h = height(control, u, density, 1);
            float x = centers[2 * i], y = centers[2 * i + 1];
            if (x - w / 2 < 0 || y - h / 2 < 0 || x + w / 2 > width || y + h / 2 > height) return false;
            for (Control other : controls) {
                if (other.hiddenByDefault) continue;
                float ow = width(other, u, density, 1), oh = height(other, u, density, 1);
                if (other.steam) {
                    int j = other.ordinal() - STEAM_FIRST;
                    if (j <= i) continue;
                    center[0] = centers[2 * j];
                    center[1] = centers[2 * j + 1];
                } else {
                    defaultCenter(other, width, height, u, density, center);
                }
                if (Math.abs(center[0] - x) < (w + ow) / 2 && Math.abs(center[1] - y) < (h + oh) / 2) return false;
            }
        }
        return true;
    }

    /**
     * The control's box on a {@code width} x {@code height} surface as left, top, width and height in
     * pixels, written to {@code out} (four floats) and returned. The box always lies on the surface.
     */
    public float[] bounds(Control control, float width, float height, float density, float[] out) {
        Spec spec = spec(control);
        float u = unit(width, height, density);
        // Never wider or taller than the surface: a control grown to the whole surface fills it, and one sized on a
        // larger surface (before a fold or a window resize) fits again. A scale of 2 or less, all a layout saved
        // before free resizing has, never reaches the surface's size, so such a layout keeps its shape.
        float w = Math.min(width(control, u, density, spec.widthScale), Math.max(0, width));
        float h = Math.min(height(control, u, density, spec.heightScale), Math.max(0, height));
        float cx, cy;
        if (spec.placed()) {
            cx = spec.cx * width;
            cy = spec.cy * height;
        } else {
            defaultCenter(control, width, height, u, density, out);
            cx = out[0];
            cy = out[1];
        }
        out[0] = clamp(cx - w / 2, 0, Math.max(0, width - w));
        out[1] = clamp(cy - h / 2, 0, Math.max(0, height - h));
        out[2] = w;
        out[3] = h;
        return out;
    }

    /**
     * "v1|glyph=playstation|mode=camera|ID,cx,cy,scale,opacity,hidden[,widthScale,heightScale]|..." for edits; "v1"
     * alone for none. A control whose width and height scales differ adds them after the six fields every version
     * reads, and writes the larger as its one scale, which an older build shows (up to its own limit of 2).
     */
    public String serialize() {
        StringBuilder out = new StringBuilder(VERSION);
        if (glyph != Glyph.XBOX) out.append("|glyph=").append(glyph.name().toLowerCase(Locale.ROOT));
        if (pointerMode != PointerMode.MOUSE) out.append("|mode=").append(pointerMode.name().toLowerCase(Locale.ROOT));
        for (Control control : Control.values()) {
            Spec spec = spec(control);
            if (spec.isDefault()) continue;
            out.append('|').append(control.name()).append(',')
                    .append(spec.placed() ? String.format(Locale.ROOT, "%.4f", spec.cx) : "-").append(',')
                    .append(spec.placed() ? String.format(Locale.ROOT, "%.4f", spec.cy) : "-").append(',')
                    .append(String.format(Locale.ROOT, "%.3f", spec.scale())).append(',')
                    .append(spec.opacity).append(',')
                    .append(spec.hidden ? '1' : '0');
            if (spec.widthScale != spec.heightScale) {
                out.append(',').append(String.format(Locale.ROOT, "%.3f", spec.widthScale))
                        .append(',').append(String.format(Locale.ROOT, "%.3f", spec.heightScale));
            }
        }
        return out.toString();
    }

    /**
     * Reads {@link #serialize} output; "v2", the prefix a short-lived build wrote, loads too. The glyphs, the
     * pointer touchpad and its mode are optional: a layout saved before them loads as an Xbox layout with a mouse
     * touchpad. Unknown controls, unknown settings and broken entries are skipped and the rest kept, so a layout from
     * a newer version still loads what this one knows; an entry's fields past the eighth are ignored, as an older
     * build ignores the seventh and eighth. Null or another version gives the default layout.
     */
    public static PadLayout parse(String text) {
        PadLayout layout = new PadLayout();
        if (text == null) return layout;
        String[] parts = text.split("\\|");
        if (parts.length == 0 || (!VERSION.equals(parts[0]) && !VERSION_V2.equals(parts[0]))) return layout;
        for (int i = 1; i < parts.length; i++) {
            String part = parts[i];
            if (part.startsWith("glyph=")) {
                try {
                    layout.glyph = Glyph.valueOf(part.substring("glyph=".length()).toUpperCase(Locale.ROOT));
                } catch (IllegalArgumentException ignored) {
                    // A glyph from a newer build: Xbox, what this one shows.
                }
                continue;
            }
            if (part.startsWith("mode=")) {
                layout.pointerMode = "camera".equalsIgnoreCase(part.substring("mode=".length())) ?
                        PointerMode.CAMERA : PointerMode.MOUSE;
                continue;
            }
            String[] fields = part.split(",", -1);
            if (fields.length < 6) continue;
            Control control = Control.named(fields[0]);
            if (control == null) continue;
            try {
                Spec spec = new Spec();
                if (!"-".equals(fields[1]) && !"-".equals(fields[2])) {
                    spec.cx = clamp(Float.parseFloat(fields[1]), 0, 1);
                    spec.cy = clamp(Float.parseFloat(fields[2]), 0, 1);
                    if (Float.isNaN(spec.cx) || Float.isNaN(spec.cy)) {
                        spec.cx = Float.NaN;
                        spec.cy = Float.NaN;
                    }
                }
                spec.widthScale = spec.heightScale = clampScale(Float.parseFloat(fields[3]));
                spec.opacity = Math.max(MIN_OPACITY, Math.min(MAX_OPACITY, Integer.parseInt(fields[4])));
                spec.hidden = "1".equals(fields[5]);
                // A width and a height of their own, after the six fields; without both, the one scale serves both.
                if (fields.length >= 8) {
                    float w = parseScale(fields[6]), h = parseScale(fields[7]);
                    if (!Float.isNaN(w) && !Float.isNaN(h)) {
                        spec.widthScale = w;
                        spec.heightScale = h;
                    }
                }
                layout.spec(control).copyFrom(spec);
            } catch (NumberFormatException ignored) {
                // Skip this entry and keep the rest.
            }
        }
        return layout;
    }

    /** A saved width or height scale, clamped; NaN when the field is not a number. */
    private static float parseScale(String field) {
        try {
            float scale = Float.parseFloat(field);
            return Float.isNaN(scale) ? Float.NaN : clampScale(scale);
        } catch (NumberFormatException e) {
            return Float.NaN;
        }
    }

    static float clamp(float value, float min, float max) {
        return Math.max(min, Math.min(max, value));
    }
}
