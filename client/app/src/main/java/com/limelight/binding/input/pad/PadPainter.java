package com.limelight.binding.input.pad;

import android.content.Context;
import android.graphics.Canvas;
import android.graphics.Color;
import android.graphics.DashPathEffect;
import android.graphics.LinearGradient;
import android.graphics.Matrix;
import android.graphics.Paint;
import android.graphics.Path;
import android.graphics.RadialGradient;
import android.graphics.Region;
import android.graphics.RectF;
import android.graphics.Shader;
import android.graphics.Typeface;
import android.os.Build;

import androidx.core.content.ContextCompat;
import androidx.core.graphics.ColorUtils;

import com.limelight.R;
import com.limelight.binding.input.LayoutStick;
import com.limelight.binding.input.StickMath;
import com.limelight.nvstream.input.ControllerPacket;

import java.util.Arrays;

/**
 * Draws the modern controls in Nightjar's glass. At rest a control is dark glass with a soft light from above, a thin
 * bright rim and a soft shadow, so it reads over bright and dark games alike; its glyphs are crisp and, where the chosen
 * controller family colours them, in that family's colours. A press fills it with the app's amber, like every selected
 * thing in Nightjar, sinks it in a little and lights a halo around it that reaches past a thumb ({@link #drawHalo});
 * let go, it eases back within {@link #RELEASE_MS}. The stream view, the layout editor and its Add tiles share it, so
 * the editor shows exactly what plays.
 *
 * <p>A control draws only inside its own box, since its view's layer and the editor's cut there; the halo and a stick's
 * knob, which reach past it, draw in layers of their own. It is all fills, strokes and shaders built once and placed by
 * moving the canvas, so a frame allocates nothing and hardware acceleration draws it the same on every Android.
 */
final class PadPainter {
    /** The parts a press lights on their own: a face cluster's buttons, a D-pad's arms or the WASD keys; one elsewhere. */
    static final int PARTS = 4;
    /** How long a part takes to ease back once let go, the touchpad finger's trail to fade and the press ring to run. */
    static final long RELEASE_MS = 120, TRAIL_MS = 220, RIPPLE_MS = 240;
    /** How many of a touchpad finger's last places its trail keeps. */
    static final int TRAIL = 12;

    /** What one control shows besides its shape. */
    static final class State {
        /**
         * Gamepad flags held by this control (face buttons, D-pad directions, a button or a stick click); for the
         * WASD stick its held keys as {@link LayoutStick} direction bits.
         */
        int flags;
        /** A trigger or button held. */
        boolean pressed;
        /** A stick's position (-1..1, up positive) and its drawn center inside the box, in pixels; NaN centers the base. */
        float stickX, stickY, centerX = Float.NaN, centerY = Float.NaN;
        boolean stickActive;
        /** A trackpad's finger inside the box, in pixels; NaN while none touches it. */
        float touchX = Float.NaN, touchY = Float.NaN;
        /** The pointer touchpad's short mode word ({@link #pointerLabel}); empty for other controls. */
        String label = "";
        /**
         * How far each part is pressed in, 0 to 1: a face cluster's buttons in {@link PadInput#FACE_CENTERS} order, a
         * D-pad's arms and the WASD keys up, down, left, right, and part 0 for every other control. It is 1 the moment
         * a part is held; {@link PadControlView} eases it back to 0 once let go ({@link #easeBack}).
         */
        final float[] press = new float[PARTS];
        /** When each part last went down, for the ring that runs out from it ({@link #drawHalo}); 0 for not lately. */
        final long[] pressedAt = new long[PARTS];
        /** A touchpad finger's last places and when it was there, the oldest at {@link #trailStart}, for its trail. */
        final float[] trailX = new float[TRAIL], trailY = new float[TRAIL];
        final long[] trailAt = new long[TRAIL];
        int trailStart, trailCount;
        /** The time this frame draws at, on the clock of {@link #trailAt} and {@link #pressedAt}. */
        long now;

        void clear() {
            flags = 0; pressed = false; stickX = 0; stickY = 0; centerX = Float.NaN; centerY = Float.NaN; stickActive = false;
            touchX = Float.NaN; touchY = Float.NaN;
            Arrays.fill(press, 0);
            Arrays.fill(pressedAt, 0);
            trailStart = trailCount = 0;
        }

        /** The finger was at ({@code x}, {@code y}) at {@code at}: the trail grows, its oldest place giving way. */
        void trail(float x, float y, long at) {
            int index;
            if (trailCount < TRAIL) {
                index = (trailStart + trailCount) % TRAIL;
                trailCount++;
            } else {
                index = trailStart;
                trailStart = (trailStart + 1) % TRAIL;
            }
            trailX[index] = x;
            trailY[index] = y;
            trailAt[index] = at;
        }

        /** Whether some of the trail still shows at {@code now}. */
        boolean trailShows(long now) {
            return trailCount > 1 && now - trailAt[(trailStart + trailCount - 1) % TRAIL] < TRAIL_MS;
        }
    }

    private static final int GLASS = 0x520A0D12, GLASS_HELD = 0x7A0A0D12, RIM = 0x8CFFFFFF, RIM_PRESSED = 0x40FFFFFF,
            LABEL = 0xF2FFFFFF, SHADOW = 0x38000000, SHADOW_SOFT = 0x1A000000, DOTS = 0x33FFFFFF;
    /**
     * The face glyphs' colours per controller family, in {@link PadInput#FACE_CENTERS} order; null for the labels'
     * one neutral colour. PlayStation's shapes stay neutral, so they never take on the look of Sony's coloured mark.
     */
    private static final int[][] FACE_COLORS = {
            {0xFF7DD45E, 0xFFFF6B63, 0xFF5AAEFF, 0xFFFFD54F}, // Xbox: A green, B red, X blue, Y yellow
            null,                                              // PlayStation: neutral, as the other labels
            null,                                              // Nintendo: white, as on its pads
    };
    /**
     * In dp: the stroke the stick geometry is measured with, how far a shape that fills its box stays inside it (its
     * shadow's room), the rim, how far a press sinks a shape, and the shadow's drop and reach.
     */
    private static final float STROKE_DP = 1.5f, EDGE_DP = 3, RIM_DP = 1.25f, PRESS_IN_DP = 2, SHADOW_DROP_DP = 1,
            SHADOW_REACH_DP = 2;
    /** The halo's reach past the edge in dp and its strength in rings from the edge out, and how far the press ring runs. */
    private static final float HALO_DP = 12, RIPPLE_DP = 20;
    private static final float[] HALO = {.40f, .28f, .18f, .10f, .05f, .02f};
    /** A face cluster's free sliver between its buttons and its box, as a share of the box's shorter side. */
    private static final float FACE_SLACK = 1 - PadInput.FACE_CENTERS[0][1] - PadInput.FACE_RADIUS;
    /** The stick base's well: how far below the middle its shade is centred, as a share of the radius. */
    private static final float WELL_LIFT = .16f;

    private final int accent, onAccent;
    private final float density, edgeStroke;
    private final Paint fill = new Paint(Paint.ANTI_ALIAS_FLAG), stroke = new Paint(Paint.ANTI_ALIAS_FLAG),
            text = new Paint(Paint.ANTI_ALIAS_FLAG), dashed = new Paint(Paint.ANTI_ALIAS_FLAG),
            ring = new Paint(Paint.ANTI_ALIAS_FLAG), dots = new Paint(Paint.ANTI_ALIAS_FLAG),
            trail = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final RectF rect = new RectF(), inner = new RectF(), chip = new RectF();
    private final Path path = new Path(), unitPath = new Path(), clip = new Path();
    private final Matrix matrix = new Matrix();
    /** The held knob's shadow: a disc this much larger than the knob, dropped this share of its radius. */
    private static final float SHADOW_SCALE = 1.2f, SHADOW_DROP = .12f;
    // Unit shaders (radius 1, or the box 0..1): the canvas is moved and scaled onto each shape instead of the shader,
    // so a frame neither allocates nor rebuilds a native shader.
    private final Paint knobShadow = shaderPaint(new RadialGradient(0, 0, 1,
            new int[] {0x66000000, 0x4D000000, 0x00000000}, new float[] {0, .8f, 1}, Shader.TileMode.CLAMP));
    /** Round under a disc: clear in the middle, darkest just inside the edge, gone a little past it. */
    private final Paint discShadow = shaderPaint(new RadialGradient(0, 0, 1,
            new int[] {0, 0, 0x47000000, 0}, new float[] {0, .7f, .9f, 1}, Shader.TileMode.CLAMP));
    /** The light from above on glass, over the box 0..1. */
    private final Paint highlight = shaderPaint(new LinearGradient(0, 0, 0, 1,
            new int[] {0x4DFFFFFF, 0x14FFFFFF, 0x00FFFFFF}, new float[] {0, .46f, .72f}, Shader.TileMode.CLAMP));
    /** The stick base's well: shade under its upper wall. */
    private final Paint well = shaderPaint(new RadialGradient(0, 0, 1,
            new int[] {0, 0, 0x45000000}, new float[] {0, .78f, 1}, Shader.TileMode.CLAMP));
    private final Paint knobGloss = shaderPaint(new RadialGradient(-.35f, -.45f, 1.45f,
            new int[] {0xFFFFFFFF, 0xFFE9EBEF, 0xFFAFB4BE}, new float[] {0, .45f, 1}, Shader.TileMode.CLAMP));
    private final Paint knobGlossPressed, fingerGlow;
    /** The face letters and shoulder names; the layout editor changes it and every view redraws. */
    private PadLayout.Glyph glyph = PadLayout.Glyph.XBOX;
    /** The face cluster's buttons stand in the Nintendo arrangement ({@link PadLayout#facesSwapped}). */
    private boolean facesSwapped;
    /** The stick geometry for the last size drawn, recomputed only when a stick's size changes. */
    private float stickSize = Float.NaN, stickBase, stickKnob;
    private final float[] knobAt = new float[2];
    /** A touchpad's dot grid, refilled for each pad drawn and grown only when a larger pad needs more. */
    private float[] dotPoints = new float[128];
    /** The pointer touchpad's mode words in its chip ({@link #pointerLabel}). */
    private final String mouseWord, cameraWord;

    PadPainter(Context context) {
        density = context.getResources().getDisplayMetrics().density;
        mouseWord = context.getString(R.string.pad_touchpad_chip_mouse);
        cameraWord = context.getString(R.string.pad_touchpad_chip_camera);
        edgeStroke = STROKE_DP * density;
        accent = ContextCompat.getColor(context, R.color.night_primary);
        onAccent = ContextCompat.getColor(context, R.color.night_on_primary);
        stroke.setStyle(Paint.Style.STROKE);
        stroke.setStrokeWidth(edgeStroke);
        stroke.setStrokeCap(Paint.Cap.ROUND);
        stroke.setStrokeJoin(Paint.Join.ROUND);
        text.setTextAlign(Paint.Align.CENTER);
        text.setTypeface(Typeface.create("sans-serif-medium", Typeface.NORMAL));
        dashed.setStyle(Paint.Style.STROKE);
        dashed.setStrokeWidth(2 * density);
        dashed.setPathEffect(new DashPathEffect(new float[] {6 * density, 4 * density}, 0));
        ring.setStyle(Paint.Style.STROKE);
        dots.setStyle(Paint.Style.STROKE);
        dots.setStrokeCap(Paint.Cap.ROUND);
        dots.setColor(DOTS);
        trail.setStyle(Paint.Style.STROKE);
        trail.setStrokeCap(Paint.Cap.ROUND);
        knobGlossPressed = shaderPaint(new RadialGradient(-.35f, -.45f, 1.45f,
                new int[] {ColorUtils.blendARGB(accent, 0xFFFFFFFF, .45f), accent, ColorUtils.blendARGB(accent, 0xFF000000, .2f)},
                new float[] {0, .5f, 1}, Shader.TileMode.CLAMP));
        fingerGlow = shaderPaint(new RadialGradient(0, 0, 1, new int[] {alpha(accent, .55f), alpha(accent, 0)},
                new float[] {.3f, 1}, Shader.TileMode.CLAMP));
    }

    private static Paint shaderPaint(Shader shader) {
        Paint paint = new Paint(Paint.ANTI_ALIAS_FLAG);
        paint.setShader(shader);
        return paint;
    }

    int accent() {
        return accent;
    }

    /** The word the pointer touchpad shows for its mode, in play and in the editor. */
    String pointerLabel(boolean camera) {
        return camera ? cameraWord : mouseWord;
    }

    void setGlyph(PadLayout.Glyph glyph) {
        this.glyph = glyph;
    }

    /** Draws and reads the face cluster with its buttons in the Nintendo arrangement, or the standard one. */
    void setFacesSwapped(boolean swapped) {
        facesSwapped = swapped;
    }

    boolean facesSwapped() {
        return facesSwapped;
    }

    /** The parts {@code state} holds now, as bits in {@link State#press} order, with the standard face buttons. */
    static int heldParts(PadLayout.Control control, State state) {
        return heldParts(control, state, false);
    }

    /** The parts {@code state} holds now; a face cluster's places hold its buttons in the Nintendo order if swapped. */
    static int heldParts(PadLayout.Control control, State state, boolean facesSwapped) {
        switch (control.shape) {
            case FACE: {
                int[] order = PadInput.faceOrder(facesSwapped);
                int bits = 0;
                for (int i = 0; i < PARTS; i++) {
                    if ((state.flags & PadInput.FACE_FLAGS[order[i]]) != 0) bits |= 1 << i;
                }
                return bits;
            }
            case DPAD:
                return directions(state.flags, ControllerPacket.UP_FLAG, ControllerPacket.DOWN_FLAG,
                        ControllerPacket.LEFT_FLAG, ControllerPacket.RIGHT_FLAG);
            case STICK:
                if (control == PadLayout.Control.WASD) {
                    return directions(state.flags, LayoutStick.UP, LayoutStick.DOWN, LayoutStick.LEFT, LayoutStick.RIGHT);
                }
                return (state.flags & PadInput.buttonFlag(control)) != 0 ? 1 : 0;
            default:
                return state.pressed ? 1 : 0;
        }
    }

    private static int directions(int flags, int up, int down, int left, int right) {
        return ((flags & up) != 0 ? 1 : 0) | ((flags & down) != 0 ? 2 : 0) | ((flags & left) != 0 ? 4 : 0)
                | ((flags & right) != 0 ? 8 : 0);
    }

    /** How far a part let go {@code sinceMs} ago is still pressed in: from 1 back to 0 over {@link #RELEASE_MS}, fast at first. */
    static float easeBack(long sinceMs) {
        if (sinceMs <= 0) return 1;
        if (sinceMs >= RELEASE_MS) return 0;
        float rest = 1 - sinceMs / (float) RELEASE_MS;
        return rest * rest;
    }

    /** Draws {@code control} filling the box ({@code left}, {@code top}, {@code width}, {@code height}). */
    void draw(Canvas canvas, PadLayout.Control control, float left, float top, float width, float height, State state) {
        switch (control.shape) {
            case STICK: drawStick(canvas, control, left, top, width, height, state); break;
            case DPAD: drawDpad(canvas, left, top, width, height, state); break;
            case FACE: drawFace(canvas, left, top, width, height, state); break;
            case PILL: drawPill(canvas, control, left, top, width, height, state); break;
            case PAD: drawPad(canvas, left, top, width, height, state, PAD_CORNER,
                    control == PadLayout.Control.LEFT_PAD ? "L" : "R", null); break;
            case TOUCHPAD: drawPad(canvas, left, top, width, height, state, TOUCHPAD_CORNER, null, state.label); break;
            case FACE_BUTTON: drawFaceButton(canvas, control, left, top, width, height, state); break;
            case DPAD_ARM: drawDpadArm(canvas, control, left, top, width, height, state); break;
            default: drawRound(canvas, control, left, top, width, height, state); break;
        }
    }

    /** How far outside a control's box the editor draws its outline and, at the outline's corners, its handles. */
    /** A Steam trackpad's and the pointer touchpad's corner radius, as a share of their shorter side. */
    private static final float PAD_CORNER = .2f, TOUCHPAD_CORNER = .18f;
    static final float OUTLINE_DP = 3;
    /** The radius of the editor's round corner handles. */
    static final float HANDLE_DP = 6;

    /** The editor's outline around a control: dashed amber with round corner handles when selected, a faint line otherwise. */
    void drawOutline(Canvas canvas, float left, float top, float width, float height, boolean selected) {
        float pad = OUTLINE_DP * density, radius = 12 * density;
        rect.set(left - pad, top - pad, left + width + pad, top + height + pad);
        if (selected) {
            dashed.setColor(accent);
            canvas.drawRoundRect(rect, radius, radius, dashed);
            // Corner handles: dragging one sizes the control's width and height (PadEditor).
            float h = HANDLE_DP * density;
            fill.setColor(0xFFFFFFFF);
            stroke.setColor(accent);
            stroke.setStrokeWidth(2 * density);
            for (int corner = 0; corner < 4; corner++) {
                float x = (corner & 1) != 0 ? rect.right : rect.left, y = (corner & 2) != 0 ? rect.bottom : rect.top;
                canvas.drawCircle(x, y, h, fill);
                canvas.drawCircle(x, y, h, stroke);
            }
        } else {
            stroke.setColor(0x38FFFFFF);
            stroke.setStrokeWidth(density);
            canvas.drawRoundRect(rect, radius, radius, stroke);
        }
    }

    /**
     * The press cue past a control's edge, drawn under the controls by {@link PadControlView.HaloLayer}: an amber halo
     * around every held part that fades as it eases back, and a ring that runs out from it the moment it went down, so a
     * thumb covering a small button still shows it answered. Every part a player presses has one: face buttons, the
     * D-pad's arms, pills, round buttons, a stick's click, each held key of the WASD stick on its side of the ring and a
     * finger on a touchpad; pushing an analog stick shows in its arc instead. {@code alpha} is the control's own, and 0
     * draws nothing. Returns whether a ring is still running, so the layer draws the next frame.
     */
    boolean drawHalo(Canvas canvas, PadLayout.Control control, float left, float top, float width, float height,
                     State state, float alpha, long now) {
        if (alpha <= 0) return false;
        float edge = EDGE_DP * density;
        boolean running = false;
        switch (control.shape) {
            case FACE: {
                float size = Math.min(width, height), r = size * PadInput.FACE_RADIUS;
                if (!lit(state, PARTS, now)) break;
                // Around the held button and between the others, never across them.
                int saved = canvas.save();
                clip.reset();
                for (int i = 0; i < PARTS; i++) {
                    clip.addCircle(left + PadInput.FACE_CENTERS[i][0] * width, top + PadInput.FACE_CENTERS[i][1] * height, r,
                            Path.Direction.CW);
                }
                clipOut(canvas, clip);
                for (int i = 0; i < PARTS; i++) {
                    running |= haloOval(canvas, left + PadInput.FACE_CENTERS[i][0] * width,
                            top + PadInput.FACE_CENTERS[i][1] * height, r, r, state, i, alpha, now);
                }
                canvas.restoreToCount(saved);
                break;
            }
            case ROUND:
            case FACE_BUTTON:
                running = haloOval(canvas, left + width / 2, top + height / 2, width / 2 - edge, height / 2 - edge, state, 0,
                        alpha, now);
                break;
            case PILL:
                rect.set(left + edge, top + edge, left + width - edge, top + height - edge);
                running = haloRound(canvas, rect, Math.min(rect.width(), rect.height()) / 2, state, 0, alpha, now);
                break;
            case DPAD_ARM: {
                boolean vertical = control == PadLayout.Control.DPAD_UP || control == PadLayout.Control.DPAD_DOWN;
                rect.set(left + edge, top + edge, left + width - edge, top + height - edge);
                running = haloRound(canvas, rect, armRadius(rect, vertical), state, 0, alpha, now);
                break;
            }
            case DPAD: {
                float size = Math.min(width, height), arm = size * PadInput.DPAD_ARM, r = size * PadInput.DPAD_CORNER;
                float cx = left + width / 2, cy = top + height / 2, reach = (HALO_DP + RIPPLE_DP) * density;
                float l = left + edge, t = top + edge, rgt = left + width - edge, b = top + height - edge;
                for (int i = 0; i < PARTS; i++) {
                    if (state.press[i] <= 0 && !rippling(state, i, now)) continue;
                    // The arm from its tip to the bar across it; the halo only past that bar, outside the cross.
                    int saved = canvas.save();
                    switch (i) {
                        case 0:
                            rect.set(cx - arm / 2, t, cx + arm / 2, cy - arm / 2);
                            canvas.clipRect(l - reach, t - reach, rgt + reach, cy - arm / 2);
                            break;
                        case 1:
                            rect.set(cx - arm / 2, cy + arm / 2, cx + arm / 2, b);
                            canvas.clipRect(l - reach, cy + arm / 2, rgt + reach, b + reach);
                            break;
                        case 2:
                            rect.set(l, cy - arm / 2, cx - arm / 2, cy + arm / 2);
                            canvas.clipRect(l - reach, t - reach, cx - arm / 2, b + reach);
                            break;
                        default:
                            rect.set(cx + arm / 2, cy - arm / 2, rgt, cy + arm / 2);
                            canvas.clipRect(cx + arm / 2, t - reach, rgt + reach, b + reach);
                            break;
                    }
                    running |= haloRound(canvas, rect, r, state, i, alpha, now);
                    canvas.restoreToCount(saved);
                }
                break;
            }
            case STICK: {
                stickGeometry(Math.min(width, height));
                float base = stickBase, cx = left + width / 2, cy = top + height / 2;
                if (control != PadLayout.Control.WASD) {
                    running = haloOval(canvas, cx, cy, base, base, state, 0, alpha, now);
                    break;
                }
                // Each held key on its own side of the ring, as the D-pad's arms: up, down, left, right.
                float reach = base + (HALO_DP + RIPPLE_DP) * density, side = base * .5f;
                for (int i = 0; i < PARTS; i++) {
                    if (state.press[i] <= 0 && !rippling(state, i, now)) continue;
                    int saved = canvas.save();
                    switch (i) {
                        case 0: canvas.clipRect(cx - reach, cy - reach, cx + reach, cy - side); break;
                        case 1: canvas.clipRect(cx - reach, cy + side, cx + reach, cy + reach); break;
                        case 2: canvas.clipRect(cx - reach, cy - reach, cx - side, cy + reach); break;
                        default: canvas.clipRect(cx + side, cy - reach, cx + reach, cy + reach); break;
                    }
                    running |= haloOval(canvas, cx, cy, base, base, state, i, alpha, now);
                    canvas.restoreToCount(saved);
                }
                break;
            }
            case PAD:
            case TOUCHPAD: {
                rect.set(left + edge, top + edge, left + width - edge, top + height - edge);
                float corner = control.shape == PadLayout.Shape.PAD ? PAD_CORNER : TOUCHPAD_CORNER;
                running = haloRound(canvas, rect, Math.min(rect.width(), rect.height()) * corner, state, 0, alpha, now);
                break;
            }
            default:
                break;
        }
        return running;
    }

    /** Whether any of the first {@code parts} parts is pressed, easing back or ringing. */
    private static boolean lit(State state, int parts, long now) {
        for (int i = 0; i < parts; i++) {
            if (state.press[i] > 0 || rippling(state, i, now)) return true;
        }
        return false;
    }

    private static boolean rippling(State state, int part, long now) {
        long since = now - state.pressedAt[part];
        return state.pressedAt[part] != 0 && since >= 0 && since < RIPPLE_MS;
    }

    private void clipOut(Canvas canvas, Path shape) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            canvas.clipOutPath(shape);
        } else {
            canvas.clipPath(shape, Region.Op.DIFFERENCE);
        }
    }

    /** The halo and press ring around an oval or disc part. */
    private boolean haloOval(Canvas canvas, float cx, float cy, float rx, float ry, State state, int part, float alpha, long now) {
        float level = state.press[part];
        if (level > 0) {
            float step = HALO_DP * density / HALO.length;
            ring.setStrokeWidth(step);
            for (int k = 0; k < HALO.length; k++) {
                float out = (k + .5f) * step;
                ring.setColor(alpha(accent, alpha * level * HALO[k]));
                oval(canvas, cx, cy, rx + out, ry + out, ring);
            }
        }
        float out = ripple(state, part, alpha, now);
        if (out < 0) return false;
        oval(canvas, cx, cy, rx + out, ry + out, ring);
        return true;
    }

    /** The halo and press ring around a rounded part. */
    private boolean haloRound(Canvas canvas, RectF shape, float radius, State state, int part, float alpha, long now) {
        float level = state.press[part];
        if (level > 0) {
            float step = HALO_DP * density / HALO.length;
            ring.setStrokeWidth(step);
            for (int k = 0; k < HALO.length; k++) {
                float out = (k + .5f) * step;
                ring.setColor(alpha(accent, alpha * level * HALO[k]));
                inner.set(shape);
                inner.inset(-out, -out);
                canvas.drawRoundRect(inner, radius + out, radius + out, ring);
            }
        }
        float out = ripple(state, part, alpha, now);
        if (out < 0) return false;
        inner.set(shape);
        inner.inset(-out, -out);
        canvas.drawRoundRect(inner, radius + out, radius + out, ring);
        return true;
    }

    /**
     * Sets the ring paint for the press ring of {@code part} at {@code now} and returns how far past the edge it is,
     * or -1 when none runs: it starts at the edge and runs out {@link #RIPPLE_DP}, slowing and fading as it goes.
     */
    private float ripple(State state, int part, float alpha, long now) {
        if (!rippling(state, part, now)) return -1;
        float t = (now - state.pressedAt[part]) / (float) RIPPLE_MS, rest = 1 - t;
        ring.setStrokeWidth(2 * density);
        ring.setColor(alpha(accent, alpha * .6f * rest * rest));
        return (2 + (RIPPLE_DP - 2) * (1 - rest * rest)) * density;
    }

    private void oval(Canvas canvas, float cx, float cy, float rx, float ry, Paint paint) {
        if (rx == ry) {
            canvas.drawCircle(cx, cy, rx, paint);
        } else {
            rect.set(cx - rx, cy - ry, cx + rx, cy + ry);
            canvas.drawOval(rect, paint);
        }
    }

    /** Draws a unit shader's disc (radius 1 at the origin) stretched onto the oval at ({@code cx}, {@code cy}). */
    private static void unit(Canvas canvas, Paint paint, float cx, float cy, float rx, float ry) {
        if (rx <= 0 || ry <= 0) return;
        int saved = canvas.save();
        canvas.translate(cx, cy);
        canvas.scale(rx, ry);
        canvas.drawCircle(0, 0, 1, paint);
        canvas.restoreToCount(saved);
    }

    /** The light from above on an oval. */
    private void highlightOval(Canvas canvas, float cx, float cy, float rx, float ry) {
        if (rx <= 0 || ry <= 0) return;
        int saved = canvas.save();
        canvas.translate(cx - rx, cy - ry);
        canvas.scale(2 * rx, 2 * ry);
        canvas.drawOval(0, 0, 1, 1, highlight);
        canvas.restoreToCount(saved);
    }

    /** The light from above on a rounded rectangle; the box is scaled to 0..1, so the corners stay round. */
    private void highlightRound(Canvas canvas, RectF shape, float radius) {
        float w = shape.width(), h = shape.height();
        if (w <= 0 || h <= 0) return;
        int saved = canvas.save();
        canvas.translate(shape.left, shape.top);
        canvas.scale(w, h);
        canvas.drawRoundRect(0, 0, 1, 1, radius / w, radius / h, highlight);
        canvas.restoreToCount(saved);
    }

    /** A soft shadow along a rounded rectangle's edge, dropped a little; {@code reach} past the edge at most. */
    private void shadowRound(Canvas canvas, RectF shape, float radius, float reach, float strength) {
        if (reach <= 0 || strength <= 0) return;
        int saved = canvas.save();
        canvas.translate(0, Math.min(SHADOW_DROP_DP * density, reach / 2));
        ring.setStrokeWidth(2 * reach);
        ring.setColor(alpha(SHADOW_SOFT, strength));
        canvas.drawRoundRect(shape, radius, radius, ring);
        ring.setStrokeWidth(reach);
        ring.setColor(alpha(SHADOW, strength));
        canvas.drawRoundRect(shape, radius, radius, ring);
        canvas.restoreToCount(saved);
    }

    /**
     * A round glass part pressed in by {@code level}: its shadow, then the disc or oval sunk in by the press, filling
     * with amber, its light from above and its rim. The shadow keeps within {@code reach} of the edge.
     */
    private void disc(Canvas canvas, float cx, float cy, float rx, float ry, float level, float reach) {
        if (level < 1 && reach > 0) {
            discShadow.setAlpha(Math.round(255 * (1 - level)));
            float drop = Math.min(SHADOW_DROP_DP * density, reach / 2);
            unit(canvas, discShadow, cx, cy + drop, rx + reach - drop, ry + reach - drop);
        }
        float sink = PRESS_IN_DP * density * level;
        rx -= sink;
        ry -= sink;
        fill.setColor(blend(GLASS, accent, level));
        oval(canvas, cx, cy, rx, ry, fill);
        highlightOval(canvas, cx, cy, rx, ry);
        float rim = RIM_DP * density;
        stroke.setStrokeWidth(rim);
        stroke.setColor(blend(RIM, RIM_PRESSED, level));
        oval(canvas, cx, cy, rx - rim / 2, ry - rim / 2, stroke);
    }

    /** A rounded glass part pressed in by {@code level}, as {@link #disc}; afterwards {@link #inner} holds its shape. */
    private void slab(Canvas canvas, RectF shape, float radius, float level, float reach) {
        if (level < 1) shadowRound(canvas, shape, radius, reach, 1 - level);
        float sink = PRESS_IN_DP * density * level;
        inner.set(shape);
        inner.inset(sink, sink);
        float r = Math.max(0, radius - sink);
        fill.setColor(blend(GLASS, accent, level));
        canvas.drawRoundRect(inner, r, r, fill);
        highlightRound(canvas, inner, r);
        float rim = RIM_DP * density;
        stroke.setStrokeWidth(rim);
        stroke.setColor(blend(RIM, RIM_PRESSED, level));
        rect.set(inner);
        rect.inset(rim / 2, rim / 2);
        canvas.drawRoundRect(rect, Math.max(0, r - rim / 2), Math.max(0, r - rim / 2), stroke);
    }

    private void label(Canvas canvas, String label, float cx, float cy, float size, int color) {
        text.setTextSize(size);
        text.setColor(color);
        canvas.drawText(label, cx, cy - (text.descent() + text.ascent()) / 2, text);
    }

    private void drawStick(Canvas canvas, PadLayout.Control control, float left, float top, float width, float height, State state) {
        drawStickBase(canvas, control, left, top, width, height, state);
        drawStickKnob(canvas, control, left, top, width, height, state);
    }

    /**
     * A stick's base: a glass well, shaded under its upper wall and lit on its lower one, with its rim, a ring where the
     * knob rests and ticks at the four directions; while pushed, an amber arc on the rim points where, growing and
     * glowing with the push. The WASD stick shows its four keys instead of the ticks, a held one in amber.
     */
    void drawStickBase(Canvas canvas, PadLayout.Control control, float left, float top, float width, float height,
                       State state) {
        stickGeometry(Math.min(width, height));
        float base = stickBase;
        float cx = left + width / 2, cy = top + height / 2;
        fill.setColor(state.stickActive ? GLASS_HELD : GLASS);
        canvas.drawCircle(cx, cy, base, fill);
        // The shade's centre sits below the middle, so the upper wall is darkest.
        int saved = canvas.save();
        canvas.translate(cx, cy + base * WELL_LIFT);
        canvas.scale(base, base);
        canvas.drawCircle(0, -WELL_LIFT, 1, well);
        canvas.restoreToCount(saved);
        stroke.setStrokeWidth(density);
        stroke.setColor(0x29FFFFFF);
        float lit = base - 2.5f * density;
        rect.set(cx - lit, cy - lit, cx + lit, cy + lit);
        canvas.drawArc(rect, 30, 120, false, stroke);
        float rim = RIM_DP * density;
        stroke.setStrokeWidth(rim);
        stroke.setColor(RIM);
        canvas.drawCircle(cx, cy, base - rim / 2, stroke);
        stroke.setStrokeWidth(density);
        stroke.setColor(0x1FFFFFFF);
        canvas.drawCircle(cx, cy, base * .64f, stroke);
        if (control != PadLayout.Control.WASD) {
            stroke.setStrokeWidth(1.5f * density);
            stroke.setColor(0x52FFFFFF);
            for (int i = 0; i < 4; i++) {
                int dx = i == 0 ? 1 : i == 2 ? -1 : 0, dy = i == 1 ? 1 : i == 3 ? -1 : 0;
                canvas.drawLine(cx + dx * base * .80f, cy + dy * base * .80f, cx + dx * base * .89f, cy + dy * base * .89f, stroke);
            }
        }
        float magnitude = (float) Math.min(1, Math.hypot(state.stickX, state.stickY));
        if (magnitude > 0) {
            float angle = (float) Math.toDegrees(Math.atan2(-state.stickY, state.stickX));
            float sweep = 30 + 60 * magnitude;
            float r = base - 3.5f * density;
            rect.set(cx - r, cy - r, cx + r, cy + r);
            stroke.setColor(alpha(accent, .3f * magnitude));
            stroke.setStrokeWidth((5 + 3 * magnitude) * density);
            canvas.drawArc(rect, angle - sweep / 2, sweep, false, stroke);
            stroke.setColor(accent);
            stroke.setStrokeWidth((2.5f + 2 * magnitude) * density);
            canvas.drawArc(rect, angle - sweep / 2, sweep, false, stroke);
        }
        if (control == PadLayout.Control.WASD) {
            float reach = base * .7f, size = base * .26f;
            wasdLabel(canvas, "W", cx, cy - reach, size, state.press[0]);
            wasdLabel(canvas, "S", cx, cy + reach, size, state.press[1]);
            wasdLabel(canvas, "A", cx - reach, cy, size, state.press[2]);
            wasdLabel(canvas, "D", cx + reach, cy, size, state.press[3]);
        }
    }

    private void wasdLabel(Canvas canvas, String key, float cx, float cy, float size, float level) {
        label(canvas, key, cx, cy, size, blend(LABEL, accent, level));
    }

    /**
     * A stick's knob for the control in the box ({@code left}, {@code top}, {@code width},
     * {@code height}): glossy, lit from the upper left. Its centre travels to the base's rim at full deflection, so the
     * canvas must reach {@link #knobOverhang} past the box on every side; while held it lifts on a soft shadow.
     */
    void drawStickKnob(Canvas canvas, PadLayout.Control control, float left, float top, float width, float height, State state) {
        stickGeometry(Math.min(width, height));
        float base = stickBase, knob = stickKnob;
        // The base stays put; a floating stick's knob starts where the thumb landed.
        float cx = left + width / 2, cy = top + height / 2;
        float originX = cx, originY = cy;
        if (state.stickActive && !Float.isNaN(state.centerX)) {
            originX = left + state.centerX;
            originY = top + state.centerY;
        }
        StickMath.knob(cx, cy, originX, originY, state.stickX, state.stickY, base, knobAt);
        float kx = knobAt[0], ky = knobAt[1];
        // Light from above: the shadow falls a little below the knob and fades out past its edge; resting, it is faint.
        float shadow = knob * SHADOW_SCALE;
        knobShadow.setAlpha(state.stickActive ? 255 : 128);
        int saved = canvas.save();
        canvas.translate(kx, ky + knob * SHADOW_DROP);
        canvas.scale(shadow, shadow);
        canvas.drawCircle(0, 0, 1, knobShadow);
        canvas.restoreToCount(saved);
        boolean clicked = (state.flags & PadInput.buttonFlag(control)) != 0;
        // Held, the knob is solid: it sits on top of the rim it rides over, not behind glass.
        Paint gloss = clicked ? knobGlossPressed : knobGloss;
        gloss.setAlpha(clicked || state.stickActive ? 255 : 0xA6);
        unit(canvas, gloss, kx, ky, knob, knob);
        stroke.setStrokeWidth(density);
        stroke.setColor(0x33000000);
        canvas.drawCircle(kx, ky, knob - density / 2, stroke);
        if (clicked) {
            label(canvas, control == PadLayout.Control.LEFT_STICK ? "L3" : "R3", kx, ky, knob * .7f, onAccent);
        } else {
            // A small ring in the knob, like a thumb grip.
            stroke.setStrokeWidth(1.5f * density);
            stroke.setColor(state.stickActive ? 0x2E000000 : 0x40000000);
            canvas.drawCircle(kx, ky, knob * .55f, stroke);
        }
    }

    private void stickGeometry(float size) {
        if (size == stickSize) return;
        stickSize = size;
        stickBase = StickMath.baseRadius(size, edgeStroke);
        stickKnob = StickMath.knobRadius(size);
    }

    /** How far past each edge of a stick of this box its knob and the knob's shadow can draw. */
    float knobOverhang(float width, float height) {
        float size = Math.min(width, height), knob = StickMath.knobRadius(size);
        float shadowReach = knob * (SHADOW_SCALE - 1 + SHADOW_DROP);
        return StickMath.overhang(size, edgeStroke, shadowReach);
    }

    /** How far the finger travels for full deflection on a stick of this size; ModernController uses it as the stick radius. */
    static float travel(float size) {
        return StickMath.travel(size);
    }

    /**
     * The D-pad as one bevelled glass cross whose arms reach the box's edges, so a wide or tall box spreads it as
     * {@link PadInput#dpad} reads it; the arms keep the thickness of the box's shorter side. A held arm lights amber and
     * eases back once let go; a dimple marks the middle.
     */
    private void drawDpad(Canvas canvas, float left, float top, float width, float height, State state) {
        float size = Math.min(width, height), edge = EDGE_DP * density;
        float arm = size * PadInput.DPAD_ARM, r = size * PadInput.DPAD_CORNER;
        float cx = left + width / 2, cy = top + height / 2;
        float l = left + edge, t = top + edge, w = width - 2 * edge, h = height - 2 * edge;
        plusOutline(l, t, w, h, arm, r);
        // Its shadow along the outline, then the glass.
        float reach = SHADOW_REACH_DP * density;
        int saved = canvas.save();
        canvas.translate(0, Math.min(SHADOW_DROP_DP * density, reach / 2));
        ring.setStrokeWidth(2 * reach);
        ring.setColor(SHADOW_SOFT);
        canvas.drawPath(path, ring);
        ring.setStrokeWidth(reach);
        ring.setColor(SHADOW);
        canvas.drawPath(path, ring);
        canvas.restoreToCount(saved);
        fill.setColor(GLASS);
        canvas.drawPath(path, fill);
        float reachX = (w - arm) / 2, reachY = (h - arm) / 2;
        armFill(canvas, state.press[0], cx - arm / 2, t, cx + arm / 2, t + reachY + r, r);
        armFill(canvas, state.press[1], cx - arm / 2, t + h - reachY - r, cx + arm / 2, t + h, r);
        armFill(canvas, state.press[2], l, cy - arm / 2, l + reachX + r, cy + arm / 2, r);
        armFill(canvas, state.press[3], l + w - reachX - r, cy - arm / 2, l + w, cy + arm / 2, r);
        // The light from above over the whole cross: its outline taken to the box's 0..1 and scaled back.
        if (w > 0 && h > 0) {
            matrix.setTranslate(-l, -t);
            matrix.postScale(1 / w, 1 / h);
            path.transform(matrix, unitPath);
            saved = canvas.save();
            canvas.translate(l, t);
            canvas.scale(w, h);
            canvas.drawPath(unitPath, highlight);
            canvas.restoreToCount(saved);
        }
        fill.setColor(0x2E000000);
        canvas.drawCircle(cx, cy, arm * .26f, fill);
        float rim = RIM_DP * density;
        stroke.setStrokeWidth(rim);
        stroke.setColor(RIM);
        canvas.drawPath(path, stroke);
        float chevron = arm * .22f, insetX = reachX * .5f, insetY = reachY * .5f;
        chevron(canvas, cx, t + insetY, 0, -1, chevron, state.press[0]);
        chevron(canvas, cx, t + h - insetY, 0, 1, chevron, state.press[1]);
        chevron(canvas, l + insetX, cy, -1, 0, chevron, state.press[2]);
        chevron(canvas, l + w - insetX, cy, 1, 0, chevron, state.press[3]);
    }

    /** Puts the plus's outline, its arms {@code arm} thick with corners of radius {@code r}, in {@link #path}. */
    private void plusOutline(float left, float top, float width, float height, float arm, float r) {
        float ax = (width - arm) / 2, bx = ax + arm, ay = (height - arm) / 2, by = ay + arm;
        float right = left + width, bottom = top + height;
        path.reset();
        path.moveTo(left + ax + r, top);
        path.lineTo(left + bx - r, top);
        path.quadTo(left + bx, top, left + bx, top + r);
        path.lineTo(left + bx, top + ay);
        path.lineTo(right - r, top + ay);
        path.quadTo(right, top + ay, right, top + ay + r);
        path.lineTo(right, top + by - r);
        path.quadTo(right, top + by, right - r, top + by);
        path.lineTo(left + bx, top + by);
        path.lineTo(left + bx, bottom - r);
        path.quadTo(left + bx, bottom, left + bx - r, bottom);
        path.lineTo(left + ax + r, bottom);
        path.quadTo(left + ax, bottom, left + ax, bottom - r);
        path.lineTo(left + ax, top + by);
        path.lineTo(left + r, top + by);
        path.quadTo(left, top + by, left, top + by - r);
        path.lineTo(left, top + ay + r);
        path.quadTo(left, top + ay, left + r, top + ay);
        path.lineTo(left + ax, top + ay);
        path.lineTo(left + ax, top + r);
        path.quadTo(left + ax, top, left + ax + r, top);
        path.close();
    }

    private void armFill(Canvas canvas, float level, float l, float t, float r, float b, float radius) {
        if (level <= 0) return;
        fill.setColor(alpha(accent, level));
        canvas.drawRoundRect(l, t, r, b, radius, radius, fill);
    }

    private void chevron(Canvas canvas, float cx, float cy, int dx, int dy, float s, float level) {
        path.reset();
        // Tip toward (dx, dy), arms back from it.
        float tipX = cx + dx * s * .6f, tipY = cy + dy * s * .6f;
        float backX = cx - dx * s * .4f, backY = cy - dy * s * .4f;
        path.moveTo(backX - dy * s, backY + dx * s);
        path.lineTo(tipX, tipY);
        path.lineTo(backX + dy * s, backY - dx * s);
        stroke.setColor(blend(LABEL, onAccent, level));
        stroke.setStrokeWidth(2 * density);
        canvas.drawPath(path, stroke);
    }

    /**
     * The face cluster: each button in its own part of the box (top, bottom, left, right), spread over a wide or tall
     * box as {@link PadInput#face} reads it, and sized to the box's shorter side so the four never overlap.
     */
    private void drawFace(Canvas canvas, float left, float top, float width, float height, State state) {
        float size = Math.min(width, height), r = size * PadInput.FACE_RADIUS;
        // The buttons leave a sliver of the box free around them; their shadows stay inside it.
        float reach = Math.max(0, Math.min(SHADOW_REACH_DP * density, size * FACE_SLACK - SHADOW_DROP_DP * density));
        int[] order = PadInput.faceOrder(facesSwapped);
        for (int i = 0; i < PadInput.FACE_CENTERS.length; i++) {
            float cx = left + PadInput.FACE_CENTERS[i][0] * width, cy = top + PadInput.FACE_CENTERS[i][1] * height;
            float level = state.press[i];
            disc(canvas, cx, cy, r, r, level, reach);
            faceGlyph(canvas, order[i], cx, cy, r - PRESS_IN_DP * density * level, level);
        }
    }

    /** One face button of a split cluster: the cluster's glass button filling its box, with the glyph set's glyph. */
    private void drawFaceButton(Canvas canvas, PadLayout.Control control, float left, float top, float width,
                                float height, State state) {
        float level = state.press[0], edge = EDGE_DP * density;
        float cx = left + width / 2, cy = top + height / 2;
        float rx = width / 2 - edge, ry = height / 2 - edge;
        disc(canvas, cx, cy, rx, ry, level, SHADOW_REACH_DP * density);
        faceGlyph(canvas, control.memberIndex(), cx, cy, Math.min(rx, ry) - PRESS_IN_DP * density * level, level);
    }

    /**
     * The glyph on the face button at {@code index} of radius {@code r}: the family's letter, or PlayStation's shapes
     * drawn as lines so they stay crisp at any size; in the family's colour (PlayStation's neutral), dark on amber
     * while pressed.
     */
    private void faceGlyph(Canvas canvas, int index, float cx, float cy, float r, float level) {
        int[] colors = FACE_COLORS[glyph.ordinal()];
        int color = blend(colors == null ? LABEL : colors[index], onAccent, level);
        if (glyph != PadLayout.Glyph.PLAYSTATION) {
            label(canvas, PadGlyphs.face(glyph, index), cx, cy, r * .95f, color);
            return;
        }
        float s = r * .42f;
        stroke.setColor(color);
        stroke.setStrokeWidth(Math.max(1.5f * density, s * .24f));
        switch (index) {
            case 0: // Cross
                canvas.drawLine(cx - s * .8f, cy - s * .8f, cx + s * .8f, cy + s * .8f, stroke);
                canvas.drawLine(cx - s * .8f, cy + s * .8f, cx + s * .8f, cy - s * .8f, stroke);
                break;
            case 1: // Circle
                canvas.drawCircle(cx, cy, s * .9f, stroke);
                break;
            case 2: // Square
                rect.set(cx - s * .78f, cy - s * .78f, cx + s * .78f, cy + s * .78f);
                canvas.drawRoundRect(rect, s * .1f, s * .1f, stroke);
                break;
            default: // Triangle, raised a little so it looks centred
                path.reset();
                path.moveTo(cx, cy - s);
                path.lineTo(cx + s * .98f, cy + s * .66f);
                path.lineTo(cx - s * .98f, cy + s * .66f);
                path.close();
                canvas.drawPath(path, stroke);
                break;
        }
    }

    /** The corner radius of a split D-pad arm in {@code shape}, rounded as the cross's arms are. */
    private static float armRadius(RectF shape, boolean vertical) {
        float thickness = vertical ? shape.width() : shape.height();
        return Math.min(thickness * PadInput.DPAD_CORNER / PadInput.DPAD_ARM, Math.min(shape.width(), shape.height()) / 2);
    }

    /**
     * One direction of a split D-pad: its arm filling the box, rounded as the D-pad's arms are, with its chevron
     * pointing out; amber while held.
     */
    private void drawDpadArm(Canvas canvas, PadLayout.Control control, float left, float top, float width, float height,
                             State state) {
        boolean vertical = control == PadLayout.Control.DPAD_UP || control == PadLayout.Control.DPAD_DOWN;
        float level = state.press[0], edge = EDGE_DP * density;
        RectF shape = chip;
        shape.set(left + edge, top + edge, left + width - edge, top + height - edge);
        float thickness = vertical ? shape.width() : shape.height();
        slab(canvas, shape, armRadius(shape, vertical), level, SHADOW_REACH_DP * density);
        int dx = control == PadLayout.Control.DPAD_LEFT ? -1 : control == PadLayout.Control.DPAD_RIGHT ? 1 : 0;
        int dy = control == PadLayout.Control.DPAD_UP ? -1 : control == PadLayout.Control.DPAD_DOWN ? 1 : 0;
        float size = Math.min(thickness, vertical ? shape.height() : shape.width());
        chevron(canvas, shape.centerX(), shape.centerY(), dx, dy, size * .22f, level);
    }

    private void drawPill(Canvas canvas, PadLayout.Control control, float left, float top, float width, float height, State state) {
        float level = state.press[0], edge = EDGE_DP * density;
        RectF shape = chip;
        shape.set(left + edge, top + edge, left + width - edge, top + height - edge);
        // A stadium filling the box, lying or standing: the shorter side is the round end.
        float thickness = Math.min(shape.width(), shape.height());
        slab(canvas, shape, thickness / 2, level, SHADOW_REACH_DP * density);
        float sunk = Math.min(inner.width(), inner.height());
        int color = blend(LABEL, onAccent, level);
        if (control == PadLayout.Control.TOUCHPAD) {
            // Touchpad: a small pad with its click line, like the one on a PlayStation pad.
            float h = sunk * .4f, w = Math.min(inner.width() * .5f, h * 1.8f);
            float l = inner.centerX() - w / 2, t = inner.centerY() - h / 2;
            stroke.setColor(color);
            stroke.setStrokeWidth(1.5f * density);
            rect.set(l, t, l + w, t + h);
            canvas.drawRoundRect(rect, h * .25f, h * .25f, stroke);
            canvas.drawLine(rect.centerX(), t + h * .3f, rect.centerX(), t + h * .7f, stroke);
            return;
        }
        String name = PadGlyphs.shoulder(control, glyph);
        if (name == null) name = pillLabel(control);
        // A long label (the grips') shrinks to stay inside the pill.
        text.setTextSize(sunk * .42f);
        float fit = Math.min(1, inner.width() * .8f / Math.max(1, text.measureText(name)));
        label(canvas, name, inner.centerX(), inner.centerY(), sunk * .42f * fit, color);
    }

    static String pillLabel(PadLayout.Control control) {
        switch (control) {
            case LEFT_GRIP: return "L GRIP";
            case RIGHT_GRIP: return "R GRIP";
            default: return control.name();
        }
    }

    /**
     * A touchpad: a rounded glass pad with a faint dot grid, its rim lit amber while touched. A Steam Controller
     * trackpad shows its side's {@code letter} in the middle; the pointer touchpad shows its mode's word as a small chip
     * in a corner ({@code word}), so a glance says whether it moves the pointer or the camera. The finger shows as an
     * amber dot with a short trail that fades behind it.
     */
    private void drawPad(Canvas canvas, float left, float top, float width, float height, State state, float corner,
                         String letter, String word) {
        float edge = EDGE_DP * density, level = state.press[0];
        RectF shape = chip;
        shape.set(left + edge, top + edge, left + width - edge, top + height - edge);
        float size = Math.min(shape.width(), shape.height()), radius = size * corner;
        shadowRound(canvas, shape, radius, SHADOW_REACH_DP * density, 1);
        fill.setColor(blend(GLASS, GLASS_HELD, level));
        canvas.drawRoundRect(shape, radius, radius, fill);
        highlightRound(canvas, shape, radius);
        drawDots(canvas, shape, radius);
        float rim = RIM_DP * density;
        stroke.setStrokeWidth(rim);
        stroke.setColor(blend(RIM, accent, level));
        rect.set(shape);
        rect.inset(rim / 2, rim / 2);
        canvas.drawRoundRect(rect, radius, radius, stroke);
        if (letter != null) label(canvas, letter, shape.centerX(), shape.centerY(), size * .22f, alpha(LABEL, .6f));
        if (word != null && !word.isEmpty()) drawChip(canvas, word, shape, size, radius);
        drawTrail(canvas, left, top, width, height, state);
        if (!Float.isNaN(state.touchX)) {
            float fx = left + Math.max(0, Math.min(width, state.touchX)), fy = top + Math.max(0, Math.min(height, state.touchY));
            float dot = Math.min(7 * density, size * .1f);
            unit(canvas, fingerGlow, fx, fy, dot * 2.6f, dot * 2.6f);
            fill.setColor(accent);
            canvas.drawCircle(fx, fy, dot, fill);
            stroke.setStrokeWidth(1.5f * density);
            stroke.setColor(0xCCFFFFFF);
            canvas.drawCircle(fx, fy, dot, stroke);
        }
    }

    /** A faint grid of dots over a touchpad's surface, one draw for all of them. */
    private void drawDots(Canvas canvas, RectF area, float radius) {
        float step = 16 * density, margin = Math.max(radius * .7f, 12 * density);
        float l = area.left + margin, t = area.top + margin, r = area.right - margin, b = area.bottom - margin;
        if (r < l || b < t) return;
        int columns = (int) ((r - l) / step) + 1, rows = (int) ((b - t) / step) + 1;
        float x0 = (l + r - (columns - 1) * step) / 2, y0 = (t + b - (rows - 1) * step) / 2;
        int count = columns * rows * 2;
        if (dotPoints.length < count) dotPoints = new float[count];
        int k = 0;
        for (int row = 0; row < rows; row++) {
            for (int column = 0; column < columns; column++) {
                dotPoints[k++] = x0 + column * step;
                dotPoints[k++] = y0 + row * step;
            }
        }
        dots.setStrokeWidth(2 * density);
        canvas.drawPoints(dotPoints, 0, k, dots);
    }

    /** The pointer touchpad's mode word as a small amber chip in its upper left corner, sized to the pad. */
    private void drawChip(Canvas canvas, String word, RectF area, float size, float radius) {
        float textSize = Math.max(7 * density, Math.min(12 * density, size * .14f));
        text.setTextSize(textSize);
        float pad = textSize * .7f, w = text.measureText(word) + 2 * pad, h = textSize * 1.75f;
        float inset = Math.max(4 * density, Math.min(radius * .45f, size * .14f));
        rect.set(area.left + inset, area.top + inset, area.left + inset + w, area.top + inset + h);
        fill.setColor(alpha(accent, .22f));
        canvas.drawRoundRect(rect, h / 2, h / 2, fill);
        stroke.setStrokeWidth(density);
        stroke.setColor(alpha(accent, .6f));
        canvas.drawRoundRect(rect, h / 2, h / 2, stroke);
        label(canvas, word, rect.centerX(), rect.centerY(), textSize, accent);
    }

    /** The finger's last places as amber strokes, thinner and fainter the older they are, gone after {@link #TRAIL_MS}. */
    private void drawTrail(Canvas canvas, float left, float top, float width, float height, State state) {
        if (state.trailCount < 2) return;
        for (int i = 1; i < state.trailCount; i++) {
            int from = (state.trailStart + i - 1) % TRAIL, to = (state.trailStart + i) % TRAIL;
            long age = Math.max(0, state.now - state.trailAt[to]);
            if (age >= TRAIL_MS) continue;
            float fade = 1 - age / (float) TRAIL_MS, along = i / (float) (state.trailCount - 1);
            trail.setStrokeWidth((1.5f + 3.5f * along) * density);
            trail.setColor(alpha(accent, .7f * fade * (.3f + .7f * along)));
            canvas.drawLine(left + Math.max(0, Math.min(width, state.trailX[from])), top + Math.max(0, Math.min(height, state.trailY[from])),
                    left + Math.max(0, Math.min(width, state.trailX[to])), top + Math.max(0, Math.min(height, state.trailY[to])), trail);
        }
    }

    /** A round button filling its box: a disc in a square one, an oval in a resized one, its mark in the middle. */
    private void drawRound(Canvas canvas, PadLayout.Control control, float left, float top, float width, float height,
                           State state) {
        float level = state.press[0], edge = EDGE_DP * density;
        float cx = left + width / 2, cy = top + height / 2;
        float rx = width / 2 - edge, ry = height / 2 - edge;
        disc(canvas, cx, cy, rx, ry, level, SHADOW_REACH_DP * density);
        float r = Math.min(rx, ry) - PRESS_IN_DP * density * level;
        int color = blend(LABEL, onAccent, level);
        float g = r * .42f;
        stroke.setColor(color);
        stroke.setStrokeWidth(1.75f * density);
        switch (control) {
            case SELECT: {
                // View: two overlapping windows.
                float w = g * 1.1f, h = g * .85f, o = g * .35f;
                rect.set(cx - w / 2 - o, cy - h / 2 - o, cx + w / 2 - o, cy + h / 2 - o);
                canvas.drawRoundRect(rect, g * .15f, g * .15f, stroke);
                rect.offset(2 * o, 2 * o);
                fill.setColor(blend(0xFF22262E, accent, level));
                canvas.drawRoundRect(rect, g * .15f, g * .15f, fill);
                canvas.drawRoundRect(rect, g * .15f, g * .15f, stroke);
                break;
            }
            case START: {
                // Menu: three lines.
                for (int i = -1; i <= 1; i++) {
                    canvas.drawLine(cx - g * .75f, cy + i * g * .5f, cx + g * .75f, cy + i * g * .5f, stroke);
                }
                break;
            }
            case HOME: {
                // Home: a house.
                path.reset();
                path.moveTo(cx - g * .8f, cy - g * .05f);
                path.lineTo(cx, cy - g * .8f);
                path.lineTo(cx + g * .8f, cy - g * .05f);
                path.moveTo(cx - g * .55f, cy - g * .25f);
                path.lineTo(cx - g * .55f, cy + g * .7f);
                path.lineTo(cx + g * .55f, cy + g * .7f);
                path.lineTo(cx + g * .55f, cy - g * .25f);
                canvas.drawPath(path, stroke);
                break;
            }
            default:
                label(canvas, control.name(), cx, cy, r * .7f, color);
                break;
        }
    }

    private static int blend(int from, int to, float t) {
        if (t <= 0) return from;
        if (t >= 1) return to;
        return ColorUtils.blendARGB(from, to, t);
    }

    private static int alpha(int color, float share) {
        return ColorUtils.setAlphaComponent(color, Math.max(0, Math.min(255, Math.round(Color.alpha(color) * share))));
    }
}
