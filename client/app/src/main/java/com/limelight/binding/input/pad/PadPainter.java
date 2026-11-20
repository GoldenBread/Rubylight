package com.limelight.binding.input.pad;

import android.content.Context;
import android.graphics.Canvas;
import android.graphics.DashPathEffect;
import android.graphics.Paint;
import android.graphics.Path;
import android.graphics.RadialGradient;
import android.graphics.RectF;
import android.graphics.Shader;
import android.graphics.Typeface;

import androidx.core.content.ContextCompat;

import com.limelight.R;
import com.limelight.binding.input.LayoutStick;
import com.limelight.binding.input.StickMath;
import com.limelight.nvstream.input.ControllerPacket;

/**
 * Draws the modern controls. Resting controls are dark glass with a thin light rim, so they read over
 * bright and dark games alike; a press fills with the app's amber, like every selected thing in
 * Nightjar. The stream view and the layout editor share it, so the editor shows exactly what plays.
 */
final class PadPainter {
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
        /** The pointer touchpad's short mode word ("MOUSE" or "CAM"); empty for other controls and in the editor. */
        String label = "";

        void clear() {
            flags = 0; pressed = false; stickX = 0; stickY = 0; centerX = Float.NaN; centerY = Float.NaN; stickActive = false;
            touchX = Float.NaN; touchY = Float.NaN;
        }
    }

    private static final int GLASS = 0x59000000, GLASS_STRONG = 0x80000000, RIM = 0x66FFFFFF, LABEL = 0xEBFFFFFF;
    private final int accent, onAccent;
    private final float density;
    private final Paint fill = new Paint(Paint.ANTI_ALIAS_FLAG), stroke = new Paint(Paint.ANTI_ALIAS_FLAG),
            text = new Paint(Paint.ANTI_ALIAS_FLAG), dashed = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final RectF rect = new RectF();
    private final Path path = new Path();
    /** The held knob's shadow: a disc this much larger than the knob, dropped this share of its radius. */
    private static final float SHADOW_SCALE = 1.2f, SHADOW_DROP = .12f;
    // A fixed unit radial fade: the canvas is moved and scaled onto the knob instead of the shader,
    // so a frame neither allocates nor rebuilds the native shader.
    private final RadialGradient shadowShader = new RadialGradient(0, 0, 1,
            new int[] {0x66000000, 0x4D000000, 0x00000000}, new float[] {0, .8f, 1}, Shader.TileMode.CLAMP);
    private final Paint shadowPaint = new Paint(Paint.ANTI_ALIAS_FLAG);
    /** The face letters and shoulder names; the layout editor changes it and every view redraws. */
    private PadLayout.Glyph glyph = PadLayout.Glyph.XBOX;
    /** The stick geometry for the last size drawn, recomputed only when a stick's size changes. */
    private float stickSize = Float.NaN, stickBase, stickKnob;
    private final float[] knobAt = new float[2];

    PadPainter(Context context) {
        density = context.getResources().getDisplayMetrics().density;
        accent = ContextCompat.getColor(context, R.color.night_primary);
        onAccent = ContextCompat.getColor(context, R.color.night_on_primary);
        stroke.setStyle(Paint.Style.STROKE);
        stroke.setStrokeWidth(1.5f * density);
        stroke.setStrokeCap(Paint.Cap.ROUND);
        stroke.setStrokeJoin(Paint.Join.ROUND);
        text.setTextAlign(Paint.Align.CENTER);
        text.setTypeface(Typeface.create("sans-serif-medium", Typeface.NORMAL));
        dashed.setStyle(Paint.Style.STROKE);
        dashed.setStrokeWidth(1.5f * density);
        dashed.setPathEffect(new DashPathEffect(new float[] {6 * density, 5 * density}, 0));
        shadowPaint.setShader(shadowShader);
    }

    int accent() {
        return accent;
    }

    void setGlyph(PadLayout.Glyph glyph) {
        this.glyph = glyph;
    }

    /** Draws {@code control} filling the box ({@code left}, {@code top}, {@code width}, {@code height}). */
    void draw(Canvas canvas, PadLayout.Control control, float left, float top, float width, float height, State state) {
        switch (control.shape) {
            case STICK: drawStick(canvas, control, left, top, width, height, state); break;
            case DPAD: drawDpad(canvas, left, top, width, height, state.flags); break;
            case FACE: drawFace(canvas, left, top, width, height, state.flags); break;
            case PILL: drawPill(canvas, control, left, top, width, height, state.pressed); break;
            case PAD: drawTrackpad(canvas, control, left, top, width, height, state); break;
            case TOUCHPAD: drawPointerPad(canvas, left, top, width, height, state); break;
            case FACE_BUTTON: drawFaceButton(canvas, control, left, top, width, height, state.pressed); break;
            case DPAD_ARM: drawDpadArm(canvas, control, left, top, width, height, state.pressed); break;
            default: drawRound(canvas, control, left, top, width, height, state.pressed); break;
        }
    }

    /** How far outside a control's box the editor draws its outline and, at the outline's corners, its handles. */
    static final float OUTLINE_DP = 3;

    /** The editor's outline around a control: amber when selected, dashed white otherwise. */
    void drawOutline(Canvas canvas, float left, float top, float width, float height, boolean selected, boolean hidden) {
        float pad = OUTLINE_DP * density, radius = 12 * density;
        rect.set(left - pad, top - pad, left + width + pad, top + height + pad);
        if (selected) {
            stroke.setColor(accent);
            stroke.setStrokeWidth(2.5f * density);
            canvas.drawRoundRect(rect, radius, radius, stroke);
            stroke.setStrokeWidth(1.5f * density);
            // Corner handles: dragging one sizes the control's width and height (PadEditor).
            fill.setColor(accent);
            float h = 4 * density;
            canvas.drawCircle(rect.left, rect.top, h, fill);
            canvas.drawCircle(rect.right, rect.top, h, fill);
            canvas.drawCircle(rect.left, rect.bottom, h, fill);
            canvas.drawCircle(rect.right, rect.bottom, h, fill);
        } else {
            dashed.setColor(hidden ? 0x66FFFFFF : 0x99FFFFFF);
            canvas.drawRoundRect(rect, radius, radius, dashed);
        }
    }

    private void glass(Canvas canvas, boolean pressed, float cx, float cy, float r) {
        glass(canvas, pressed, cx, cy, r, r);
    }

    /** A glass disc, or an oval filling a box that is not square, so the shape drawn is the shape touched. */
    private void glass(Canvas canvas, boolean pressed, float cx, float cy, float rx, float ry) {
        fill.setColor(pressed ? accent : GLASS);
        if (rx == ry) {
            canvas.drawCircle(cx, cy, rx, fill);
        } else {
            rect.set(cx - rx, cy - ry, cx + rx, cy + ry);
            canvas.drawOval(rect, fill);
        }
        if (!pressed) {
            stroke.setColor(RIM);
            if (rx == ry) {
                canvas.drawCircle(cx, cy, rx, stroke);
            } else {
                rect.set(cx - rx, cy - ry, cx + rx, cy + ry);
                canvas.drawOval(rect, stroke);
            }
        }
    }

    private void label(Canvas canvas, String label, float cx, float cy, float size, boolean pressed) {
        text.setTextSize(size);
        text.setColor(pressed ? onAccent : LABEL);
        canvas.drawText(label, cx, cy - (text.descent() + text.ascent()) / 2, text);
    }

    private void drawStick(Canvas canvas, PadLayout.Control control, float left, float top, float width, float height, State state) {
        drawStickBase(canvas, control, left, top, width, height, state);
        drawStickKnob(canvas, control, left, top, width, height, state);
    }

    /**
     * A stick's base: the glass disc, its rim and, while pushed, the amber arc pointing where. The WASD stick
     * also shows its four keys, a held one in amber.
     */
    void drawStickBase(Canvas canvas, PadLayout.Control control, float left, float top, float width, float height,
                       State state) {
        stickGeometry(Math.min(width, height));
        float base = stickBase;
        float cx = left + width / 2, cy = top + height / 2;
        fill.setColor(state.stickActive ? GLASS_STRONG : GLASS);
        canvas.drawCircle(cx, cy, base, fill);
        stroke.setColor(RIM);
        canvas.drawCircle(cx, cy, base, stroke);
        float magnitude = (float) Math.min(1, Math.hypot(state.stickX, state.stickY));
        if (magnitude > 0) {
            // An amber arc on the rim points where the stick is pushed; it grows with the push.
            float angle = (float) Math.toDegrees(Math.atan2(-state.stickY, state.stickX));
            float sweep = 30 + 50 * magnitude;
            stroke.setColor(accent);
            stroke.setStrokeWidth(4 * density);
            float r = base - 3 * density;
            rect.set(cx - r, cy - r, cx + r, cy + r);
            canvas.drawArc(rect, angle - sweep / 2, sweep, false, stroke);
            stroke.setStrokeWidth(1.5f * density);
        }
        if (control == PadLayout.Control.WASD) {
            float reach = base * .7f, size = base * .26f;
            wasdLabel(canvas, "W", cx, cy - reach, size, (state.flags & LayoutStick.UP) != 0);
            wasdLabel(canvas, "A", cx - reach, cy, size, (state.flags & LayoutStick.LEFT) != 0);
            wasdLabel(canvas, "S", cx, cy + reach, size, (state.flags & LayoutStick.DOWN) != 0);
            wasdLabel(canvas, "D", cx + reach, cy, size, (state.flags & LayoutStick.RIGHT) != 0);
        }
    }

    private void wasdLabel(Canvas canvas, String key, float cx, float cy, float size, boolean held) {
        text.setTextSize(size);
        text.setColor(held ? accent : LABEL);
        canvas.drawText(key, cx, cy - (text.descent() + text.ascent()) / 2, text);
    }

    /**
     * A stick's knob for the control in the box ({@code left}, {@code top}, {@code width},
     * {@code height}). Its centre travels to the base's rim at full deflection, so the canvas must
     * reach {@link #knobOverhang} past the box on every side; while held it lifts on a soft shadow.
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
        if (state.stickActive) {
            // Light from above: the shadow falls a little below the knob and fades out past its edge.
            float shadow = knob * SHADOW_SCALE;
            int saved = canvas.save();
            canvas.translate(kx, ky + knob * SHADOW_DROP);
            canvas.scale(shadow, shadow);
            canvas.drawCircle(0, 0, 1, shadowPaint);
            canvas.restoreToCount(saved);
        }
        boolean clicked = (state.flags & PadInput.buttonFlag(control)) != 0;
        // Held, the knob is solid: it sits on top of the rim it rides over, not behind glass.
        fill.setColor(clicked ? accent : state.stickActive ? 0xFFF2F2F2 : 0x99FFFFFF);
        canvas.drawCircle(kx, ky, knob, fill);
        if (clicked) {
            label(canvas, control == PadLayout.Control.LEFT_STICK ? "L3" : "R3", kx, ky, knob * .7f, true);
        } else {
            // A small ring in the knob, like a thumb grip.
            stroke.setColor(state.stickActive ? 0x33000000 : 0x40000000);
            canvas.drawCircle(kx, ky, knob * .55f, stroke);
        }
    }

    private void stickGeometry(float size) {
        if (size == stickSize) return;
        stickSize = size;
        stickBase = StickMath.baseRadius(size, stroke.getStrokeWidth());
        stickKnob = StickMath.knobRadius(size);
    }

    /** How far past each edge of a stick of this box its knob and the knob's shadow can draw. */
    float knobOverhang(float width, float height) {
        float size = Math.min(width, height), knob = StickMath.knobRadius(size);
        float shadowReach = knob * (SHADOW_SCALE - 1 + SHADOW_DROP);
        return StickMath.overhang(size, stroke.getStrokeWidth(), shadowReach);
    }

    /** How far the finger travels for full deflection on a stick of this size; ModernController uses it as the stick radius. */
    static float travel(float size) {
        return StickMath.travel(size);
    }

    /**
     * The D-pad as one plus whose arms reach the box's edges, so a wide or tall box spreads it as {@link PadInput#dpad}
     * reads it; the arms keep the thickness of the box's shorter side.
     */
    private void drawDpad(Canvas canvas, float left, float top, float width, float height, int flags) {
        float size = Math.min(width, height);
        float arm = size * PadInput.DPAD_ARM, r = size * PadInput.DPAD_CORNER;
        float cx = left + width / 2, cy = top + height / 2;
        // The plus as one rounded shape, then each held arm filled amber on top.
        path.reset();
        path.addRoundRect(cx - arm / 2, top, cx + arm / 2, top + height, r, r, Path.Direction.CW);
        path.addRoundRect(left, cy - arm / 2, left + width, cy + arm / 2, r, r, Path.Direction.CW);
        path.setFillType(Path.FillType.WINDING);
        fill.setColor(GLASS);
        canvas.drawPath(path, fill);
        stroke.setColor(RIM);
        drawPlusOutline(canvas, left, top, width, height, arm, r);
        float reachX = (width - arm) / 2, reachY = (height - arm) / 2;
        armFill(canvas, (flags & ControllerPacket.UP_FLAG) != 0, cx - arm / 2, top, cx + arm / 2, top + reachY + r, r);
        armFill(canvas, (flags & ControllerPacket.DOWN_FLAG) != 0, cx - arm / 2, top + height - reachY - r, cx + arm / 2, top + height, r);
        armFill(canvas, (flags & ControllerPacket.LEFT_FLAG) != 0, left, cy - arm / 2, left + reachX + r, cy + arm / 2, r);
        armFill(canvas, (flags & ControllerPacket.RIGHT_FLAG) != 0, left + width - reachX - r, cy - arm / 2, left + width, cy + arm / 2, r);
        float chevron = arm * .22f, insetX = reachX * .5f, insetY = reachY * .5f;
        chevron(canvas, cx, top + insetY, 0, -1, chevron, (flags & ControllerPacket.UP_FLAG) != 0);
        chevron(canvas, cx, top + height - insetY, 0, 1, chevron, (flags & ControllerPacket.DOWN_FLAG) != 0);
        chevron(canvas, left + insetX, cy, -1, 0, chevron, (flags & ControllerPacket.LEFT_FLAG) != 0);
        chevron(canvas, left + width - insetX, cy, 1, 0, chevron, (flags & ControllerPacket.RIGHT_FLAG) != 0);
    }

    private void drawPlusOutline(Canvas canvas, float left, float top, float width, float height, float arm, float r) {
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
        canvas.drawPath(path, stroke);
    }

    private void armFill(Canvas canvas, boolean held, float l, float t, float r, float b, float radius) {
        if (!held) return;
        fill.setColor(accent);
        canvas.drawRoundRect(l, t, r, b, radius, radius, fill);
    }

    private void chevron(Canvas canvas, float cx, float cy, int dx, int dy, float s, boolean held) {
        path.reset();
        // Tip toward (dx, dy), arms back from it.
        float tipX = cx + dx * s * .6f, tipY = cy + dy * s * .6f;
        float backX = cx - dx * s * .4f, backY = cy - dy * s * .4f;
        path.moveTo(backX - dy * s, backY + dx * s);
        path.lineTo(tipX, tipY);
        path.lineTo(backX + dy * s, backY - dx * s);
        stroke.setColor(held ? onAccent : LABEL);
        stroke.setStrokeWidth(2 * density);
        canvas.drawPath(path, stroke);
        stroke.setStrokeWidth(1.5f * density);
    }

    /**
     * The face cluster: each button in its own part of the box (top, bottom, left, right), spread over a wide or tall
     * box as {@link PadInput#face} reads it, and sized to the box's shorter side so the four never overlap.
     */
    private void drawFace(Canvas canvas, float left, float top, float width, float height, int flags) {
        float r = Math.min(width, height) * PadInput.FACE_RADIUS;
        for (int i = 0; i < PadInput.FACE_CENTERS.length; i++) {
            float cx = left + PadInput.FACE_CENTERS[i][0] * width, cy = top + PadInput.FACE_CENTERS[i][1] * height;
            boolean held = (flags & PadInput.FACE_FLAGS[i]) != 0;
            glass(canvas, held, cx, cy, r);
            label(canvas, PadGlyphs.face(glyph, i), cx, cy, r * .95f, held);
        }
    }

    /** One face button of a split cluster: the cluster's glass button filling its box, with the glyph set's letter. */
    private void drawFaceButton(Canvas canvas, PadLayout.Control control, float left, float top, float width,
                                float height, boolean pressed) {
        float cx = left + width / 2, cy = top + height / 2;
        float rx = width / 2 - stroke.getStrokeWidth(), ry = height / 2 - stroke.getStrokeWidth();
        glass(canvas, pressed, cx, cy, rx, ry);
        label(canvas, PadGlyphs.face(glyph, control.memberIndex()), cx, cy, Math.min(rx, ry) * .95f, pressed);
    }

    /**
     * One direction of a split D-pad: its arm filling the box, rounded as the D-pad's arms are, with its chevron
     * pointing out; amber while held.
     */
    private void drawDpadArm(Canvas canvas, PadLayout.Control control, float left, float top, float width, float height,
                             boolean pressed) {
        boolean vertical = control == PadLayout.Control.DPAD_UP || control == PadLayout.Control.DPAD_DOWN;
        float inset = stroke.getStrokeWidth();
        rect.set(left + inset, top + inset, left + width - inset, top + height - inset);
        float thickness = vertical ? rect.width() : rect.height();
        float radius = Math.min(thickness * PadInput.DPAD_CORNER / PadInput.DPAD_ARM, Math.min(rect.width(), rect.height()) / 2);
        fill.setColor(pressed ? accent : GLASS);
        canvas.drawRoundRect(rect, radius, radius, fill);
        if (!pressed) {
            stroke.setColor(RIM);
            canvas.drawRoundRect(rect, radius, radius, stroke);
        }
        int dx = control == PadLayout.Control.DPAD_LEFT ? -1 : control == PadLayout.Control.DPAD_RIGHT ? 1 : 0;
        int dy = control == PadLayout.Control.DPAD_UP ? -1 : control == PadLayout.Control.DPAD_DOWN ? 1 : 0;
        float size = Math.min(thickness, vertical ? rect.height() : rect.width());
        chevron(canvas, rect.centerX(), rect.centerY(), dx, dy, size * .22f, pressed);
    }

    private void drawPill(Canvas canvas, PadLayout.Control control, float left, float top, float width, float height, boolean pressed) {
        float inset = stroke.getStrokeWidth();
        rect.set(left + inset, top + inset, left + width - inset, top + height - inset);
        // A stadium filling the box, lying or standing: the shorter side is the round end.
        float thickness = Math.min(rect.width(), rect.height());
        float radius = thickness / 2;
        fill.setColor(pressed ? accent : GLASS);
        canvas.drawRoundRect(rect, radius, radius, fill);
        if (!pressed) {
            stroke.setColor(RIM);
            canvas.drawRoundRect(rect, radius, radius, stroke);
        }
        if (control == PadLayout.Control.TOUCHPAD) {
            // Touchpad: a small pad with its click line, like the one on a PlayStation pad.
            float h = thickness * .4f, w = Math.min(rect.width() * .5f, h * 1.8f);
            float l = rect.centerX() - w / 2, t = rect.centerY() - h / 2;
            stroke.setColor(pressed ? onAccent : LABEL);
            rect.set(l, t, l + w, t + h);
            canvas.drawRoundRect(rect, h * .25f, h * .25f, stroke);
            canvas.drawLine(rect.centerX(), t + h * .3f, rect.centerX(), t + h * .7f, stroke);
            return;
        }
        String name = PadGlyphs.shoulder(control, glyph);
        if (name == null) name = pillLabel(control);
        // A long label (the grips') shrinks to stay inside the pill.
        text.setTextSize(thickness * .42f);
        float fit = Math.min(1, rect.width() * .8f / Math.max(1, text.measureText(name)));
        label(canvas, name, rect.centerX(), rect.centerY(), thickness * .42f * fit, pressed);
    }

    static String pillLabel(PadLayout.Control control) {
        switch (control) {
            case LEFT_GRIP: return "L GRIP";
            case RIGHT_GRIP: return "R GRIP";
            default: return control.name();
        }
    }

    /** A Steam Controller trackpad: a rounded glass square with its side's letter, and the finger as an amber dot. */
    private void drawTrackpad(Canvas canvas, PadLayout.Control control, float left, float top, float width, float height,
                              State state) {
        float inset = stroke.getStrokeWidth();
        rect.set(left + inset, top + inset, left + width - inset, top + height - inset);
        float radius = Math.min(rect.width(), rect.height()) * .2f;
        fill.setColor(state.pressed ? GLASS_STRONG : GLASS);
        canvas.drawRoundRect(rect, radius, radius, fill);
        stroke.setColor(state.pressed ? accent : RIM);
        canvas.drawRoundRect(rect, radius, radius, stroke);
        float size = Math.min(rect.width(), rect.height());
        label(canvas, control == PadLayout.Control.LEFT_PAD ? "L" : "R", rect.centerX(), rect.centerY(), size * .22f, false);
        if (state.pressed && !Float.isNaN(state.touchX)) {
            fill.setColor(accent);
            canvas.drawCircle(left + Math.max(0, Math.min(width, state.touchX)), top + Math.max(0, Math.min(height, state.touchY)),
                    size * .09f, fill);
        }
    }

    /**
     * The pointer touchpad: a wide glass pad with a mouse-shaped mark and the mode's short word, so a glance says
     * whether it moves the pointer or the camera; the finger shows as an amber dot while it touches.
     */
    private void drawPointerPad(Canvas canvas, float left, float top, float width, float height, State state) {
        float inset = stroke.getStrokeWidth();
        rect.set(left + inset, top + inset, left + width - inset, top + height - inset);
        float radius = Math.min(rect.width(), rect.height()) * .18f;
        fill.setColor(state.pressed ? GLASS_STRONG : GLASS);
        canvas.drawRoundRect(rect, radius, radius, fill);
        stroke.setColor(state.pressed ? accent : RIM);
        canvas.drawRoundRect(rect, radius, radius, stroke);
        float size = Math.min(rect.width(), rect.height());
        // A small mouse: its body, its two buttons and the wheel between them.
        float mw = size * .30f, mh = size * .42f;
        float ml = rect.centerX() - mw / 2, mt = rect.centerY() - mh / 2;
        stroke.setColor(LABEL);
        stroke.setStrokeWidth(1.5f * density);
        rect.set(ml, mt, ml + mw, mt + mh);
        canvas.drawRoundRect(rect, mw * .45f, mw * .45f, stroke);
        canvas.drawLine(rect.centerX(), mt, rect.centerX(), mt + mh * .38f, stroke);
        canvas.drawLine(rect.centerX(), mt + mh * .62f, rect.centerX(), mt + mh, stroke);
        if (state.label != null && !state.label.isEmpty()) {
            label(canvas, state.label, rect.centerX(), rect.bottom - size * .10f, size * .16f, state.pressed);
        }
        if (state.pressed && !Float.isNaN(state.touchX)) {
            fill.setColor(accent);
            canvas.drawCircle(left + Math.max(0, Math.min(width, state.touchX)), top + Math.max(0, Math.min(height, state.touchY)),
                    size * .10f, fill);
        }
    }

    /** A round button filling its box: a disc in a square one, an oval in a resized one, its mark in the middle. */
    private void drawRound(Canvas canvas, PadLayout.Control control, float left, float top, float width, float height,
                           boolean pressed) {
        float cx = left + width / 2, cy = top + height / 2;
        float rx = width / 2 - stroke.getStrokeWidth(), ry = height / 2 - stroke.getStrokeWidth(), r = Math.min(rx, ry);
        glass(canvas, pressed, cx, cy, rx, ry);
        int color = pressed ? onAccent : LABEL;
        float g = r * .42f;
        stroke.setColor(color);
        fill.setColor(color);
        switch (control) {
            case SELECT: {
                // View: two overlapping windows.
                float w = g * 1.1f, h = g * .85f, o = g * .35f;
                rect.set(cx - w / 2 - o, cy - h / 2 - o, cx + w / 2 - o, cy + h / 2 - o);
                canvas.drawRoundRect(rect, g * .15f, g * .15f, stroke);
                rect.offset(2 * o, 2 * o);
                fill.setColor(pressed ? accent : 0xFF2A2A2A);
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
                label(canvas, control.name(), cx, cy, r * .7f, pressed);
                break;
        }
    }
}
