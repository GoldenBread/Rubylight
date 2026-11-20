package com.limelight.binding.input.pad;

import android.annotation.SuppressLint;
import android.content.Context;
import android.graphics.Canvas;
import android.graphics.Paint;
import android.graphics.RectF;
import android.view.Gravity;
import android.view.LayoutInflater;
import android.view.MotionEvent;
import android.view.View;
import android.widget.FrameLayout;
import android.widget.TextView;

import com.google.android.material.button.MaterialButton;
import com.google.android.material.materialswitch.MaterialSwitch;
import com.google.android.material.slider.Slider;
import com.limelight.R;
import com.limelight.ui.NavigationDialogBuilder;

import java.util.Locale;

/**
 * Edits the modern controller's layout over a dimmed stream: drag a control to move it, pinch it to
 * resize it, drag a picked control's corner handle to size its width and height on their own up to the whole
 * surface, tap it to pick it for the panel's size, opacity and hide buttons. With nothing picked
 * the panel changes every control; its grip moves it anywhere, and it reopens where it was left. Done keeps the draft, for all games or only this one; Cancel and
 * Back drop it. Hidden controls stay here as faint ghosts so they can be shown again, below every shown control; the
 * WASD stick, left out of the default layout, has its own Add button instead of a ghost.
 */
@SuppressLint("ViewConstructor") // Created by ModernController.
final class PadEditor extends FrameLayout {
    /** Moved controls land on this grid, so rows and mirrored pairs line up without effort. */
    static final float GRID_DP = 8;
    /** The bits of a corner handle: 0 is the top-left one. */
    static final int RIGHT_CORNER = 1, BOTTOM_CORNER = 2;
    private static final float SCALE_STEP = .1f, PANEL_WIDTH_DP = 400, PANEL_MARGIN_DP = 16, SHORT_SURFACE_DP = 480;

    private final ModernController controller;
    private final PadPainter painter;
    private final PadLayout draft;
    private final boolean gameAvailable;
    private boolean perGame;
    private PadLayout.Control selected;
    private final Surface surface;
    private final View panel;
    private final TextView selection, sizeText, hint;
    private final MaterialButton hide, reset, addWasd, addTouchpad, preset, glyphs, pointerMode, split, join;
    private boolean compactButtons, shortSurface;
    private final Slider opacity;
    private final float density;
    private boolean settingOpacity;
    /**
     * Whether the player has placed the panel with its grip, in this orientation now or before. Then it stays where
     * it was put, its top-left corner at {@link #panelX}, {@link #panelY}, shown kept on the surface, and no longer
     * moves out of the picked control's way by itself.
     */
    private boolean panelPlaced;
    private float panelX, panelY, gripRawX, gripRawY, gripFromX, gripFromY;

    PadEditor(Context context, ModernController controller, PadPainter painter, PadLayout draft,
              boolean perGame, boolean gameAvailable) {
        // The stream's dark surfaces whatever theme the activity has, like the stream menu.
        super(new androidx.appcompat.view.ContextThemeWrapper(context, R.style.StreamTheme));
        context = getContext();
        this.controller = controller;
        this.painter = painter;
        this.draft = draft;
        this.perGame = perGame && gameAvailable;
        this.gameAvailable = gameAvailable;
        density = context.getResources().getDisplayMetrics().density;
        surface = new Surface(context);
        addView(surface, new LayoutParams(LayoutParams.MATCH_PARENT, LayoutParams.MATCH_PARENT));
        panel = LayoutInflater.from(context).inflate(R.layout.pad_editor_panel, this, false);
        // The stream's glass for a modal panel without a blur: its text never sits on a control or the game.
        float corner = 16 * context.getResources().getDisplayMetrics().density;
        panel.setBackground(com.limelight.ui.GlassPanelDrawable.stream(context,
                new float[] {corner, corner, corner, corner, corner, corner, corner, corner}));
        addView(panel, new LayoutParams(LayoutParams.WRAP_CONTENT, LayoutParams.WRAP_CONTENT, Gravity.CENTER));
        panel.findViewById(R.id.pad_editor_grip).setOnTouchListener((grip, event) -> dragPanel(event));
        // A panel that grows or shrinks (the picked control's rows), or a surface that turns or folds, keeps a
        // placed panel whole on the surface.
        panel.addOnLayoutChangeListener((v, l, t, r, b, ol, ot, or, ob) -> showPlacedPanel());
        int[] placed = controller.panelPosition();
        if (placed != null) placePanel(placed[0], placed[1]);
        selection = panel.findViewById(R.id.pad_editor_selection);
        sizeText = panel.findViewById(R.id.pad_editor_size);
        hide = panel.findViewById(R.id.pad_editor_hide);
        reset = panel.findViewById(R.id.pad_editor_reset);
        addWasd = panel.findViewById(R.id.pad_editor_add_wasd);
        addWasd.setOnClickListener(v -> addWasd());
        addTouchpad = panel.findViewById(R.id.pad_editor_add_touchpad);
        addTouchpad.setOnClickListener(v -> addTouchpad());
        preset = panel.findViewById(R.id.pad_editor_preset);
        preset.setOnClickListener(v -> choosePreset());
        glyphs = panel.findViewById(R.id.pad_editor_glyphs);
        glyphs.setContentDescription(getContext().getString(R.string.pad_edit_glyphs));
        glyphs.setOnClickListener(v -> chooseGlyphs());
        pointerMode = panel.findViewById(R.id.pad_editor_pointer_mode);
        pointerMode.setOnClickListener(v -> togglePointerMode());
        split = panel.findViewById(R.id.pad_editor_split);
        split.setOnClickListener(v -> split());
        join = panel.findViewById(R.id.pad_editor_join);
        join.setOnClickListener(v -> join());
        hint = panel.findViewById(R.id.pad_editor_hint);
        opacity = panel.findViewById(R.id.pad_editor_opacity);
        panel.findViewById(R.id.pad_editor_smaller).setOnClickListener(v -> scaleBy(-SCALE_STEP));
        panel.findViewById(R.id.pad_editor_larger).setOnClickListener(v -> scaleBy(SCALE_STEP));
        hide.setOnClickListener(v -> toggleHidden());
        reset.setOnClickListener(v -> {
            draft.reset();
            painter.setGlyph(draft.glyph());
            select(null);
            changed();
        });
        panel.findViewById(R.id.pad_editor_cancel).setOnClickListener(v -> controller.finishEditing(false));
        panel.findViewById(R.id.pad_editor_done).setOnClickListener(v -> controller.finishEditing(true));
        MaterialSwitch thisGame = panel.findViewById(R.id.pad_editor_this_game);
        thisGame.setVisibility(gameAvailable ? VISIBLE : GONE);
        thisGame.setChecked(this.perGame);
        thisGame.setOnCheckedChangeListener((button, checked) -> this.perGame = checked);
        com.limelight.ui.StepSlider.showValue(opacity, value -> String.format(Locale.getDefault(), "%d %%", value));
        opacity.addOnChangeListener((slider, value, fromUser) -> {
            if (fromUser && !settingOpacity) setOpacity(Math.round(value));
        });
        setClickable(true);
        updatePanel();
    }

    @Override
    protected void onSizeChanged(int w, int h, int oldw, int oldh) {
        super.onSizeChanged(w, h, oldw, oldh);
        // As wide as the design asks on a tablet, never wider than a phone in portrait leaves room for.
        int width = Math.round(Math.min(PANEL_WIDTH_DP * density, w - 2 * PANEL_MARGIN_DP * density));
        // A narrow panel shows Hide and Reset as icons so Cancel and Done stay on the row; a short
        // surface (a phone in landscape) drops the hint so the panel covers fewer controls.
        boolean narrow = width < PANEL_WIDTH_DP * density, shortSurface = h < SHORT_SURFACE_DP * density;
        LayoutParams params = (LayoutParams) panel.getLayoutParams();
        if (params.width != width || narrow != compactButtons || shortSurface != this.shortSurface) {
            params.width = Math.max(0, width);
            post(() -> {
                compactButtons = narrow;
                this.shortSurface = shortSurface;
                updatePanel();
                panel.setLayoutParams(params);
                keepPanelClear();
            });
        }
        if (oldw != 0 && oldh != 0 && (w > h) != (oldw > oldh)) {
            // Turned while editing: the panel goes where it was left in this orientation, or back to the middle.
            post(() -> {
                int[] placed = controller.panelPosition();
                if (placed != null) {
                    placePanel(placed[0], placed[1]);
                } else {
                    unplacePanel();
                }
            });
        } else {
            post(this::showPlacedPanel);
        }
    }

    /** The grip: dragging it moves the panel anywhere on the surface; letting go remembers where, per orientation. */
    private boolean dragPanel(MotionEvent event) {
        switch (event.getActionMasked()) {
            case MotionEvent.ACTION_DOWN:
                // Raw coordinates: the grip moves with the panel, so its own would chase the finger.
                gripRawX = event.getRawX();
                gripRawY = event.getRawY();
                gripFromX = panel.getX();
                gripFromY = panel.getY();
                return true;
            case MotionEvent.ACTION_MOVE:
                placePanel(gripFromX + event.getRawX() - gripRawX, gripFromY + event.getRawY() - gripRawY);
                return true;
            case MotionEvent.ACTION_UP:
            case MotionEvent.ACTION_CANCEL:
                if (panelPlaced) controller.savePanelPosition(Math.round(panelX), Math.round(panelY));
                return true;
            default:
                return false;
        }
    }

    /** Puts the panel's top-left corner at ({@code x}, {@code y}), kept whole on the surface, and leaves it there. */
    private void placePanel(float x, float y) {
        LayoutParams params = (LayoutParams) panel.getLayoutParams();
        if (!panelPlaced || params.gravity != (Gravity.TOP | Gravity.START)) {
            // The layout puts it in the top-left corner; its translation carries the place, so a drag lays out nothing.
            params.gravity = Gravity.TOP | Gravity.START;
            params.leftMargin = params.topMargin = params.rightMargin = params.bottomMargin = 0;
            panel.setLayoutParams(params);
        }
        panelPlaced = true;
        panelX = Math.max(0, Math.min(maxPanelX(), x));
        panelY = Math.max(0, Math.min(maxPanelY(), y));
        showPlacedPanel();
    }

    /** Shows a placed panel where it was put, moved only as far as it takes to keep it whole on the surface. */
    private void showPlacedPanel() {
        if (!panelPlaced) return;
        float x = Math.max(0, Math.min(maxPanelX(), panelX)), y = Math.max(0, Math.min(maxPanelY(), panelY));
        panel.setTranslationX(x - panel.getLeft());
        panel.setTranslationY(y - panel.getTop());
    }

    /** Back to the panel's own place in the middle, moving out of the picked control's way by itself. */
    private void unplacePanel() {
        panelPlaced = false;
        panel.setTranslationX(0);
        panel.setTranslationY(0);
        LayoutParams params = (LayoutParams) panel.getLayoutParams();
        params.gravity = Gravity.CENTER;
        params.leftMargin = params.topMargin = params.rightMargin = params.bottomMargin = 0;
        panel.setLayoutParams(params);
        keepPanelClear();
    }

    private float maxPanelX() {
        return getWidth() > 0 && panel.getWidth() > 0 ? Math.max(0, getWidth() - panel.getWidth()) : Float.MAX_VALUE;
    }

    private float maxPanelY() {
        return getHeight() > 0 && panel.getHeight() > 0 ? Math.max(0, getHeight() - panel.getHeight()) : Float.MAX_VALUE;
    }

    /** The panel, for tests. */
    View panel() {
        return panel;
    }

    /** What the surface draws a control with at rest, for tests. */
    PadPainter.State previewState(PadLayout.Control control) {
        return surface.previewState(control);
    }

    PadLayout draft() {
        return draft;
    }

    boolean perGame() {
        return perGame && gameAvailable;
    }

    PadLayout.Control selected() {
        return selected;
    }

    void select(PadLayout.Control control) {
        selected = control;
        updatePanel();
        surface.invalidate();
        post(this::keepPanelClear);
    }

    /** The controls the editor shows: everything but what the settings keep off. */
    private boolean editable(PadLayout.Control control) {
        return !controller.forcedHidden(control);
    }

    /**
     * Whether the surface draws the control and a tap can pick it: a control the default layout leaves out has no
     * ghost while hidden. A cluster and its members never show together: a split cluster has no ghost under its
     * members, and a member is off the surface while its cluster shows.
     */
    private boolean onSurface(PadLayout.Control control) {
        if (!editable(control) || (control.hiddenByDefault && draft.spec(control).hidden)) return false;
        PadLayout.Control cluster = control.cluster();
        if (cluster != null) return draft.spec(cluster).hidden;
        return !draft.isSplit(control);
    }

    /**
     * The plus and minus buttons: the picked control, or every control, grows or shrinks by a step in both directions,
     * keeping its shape, up to the whole surface.
     */
    void scaleBy(float step) {
        float width = surfaceWidth(), height = surfaceHeight();
        for (PadLayout.Control control : PadLayout.Control.values()) {
            if (!editable(control) || (selected != null && control != selected)) continue;
            PadLayout.Spec spec = draft.spec(control);
            // Round the larger side to the step so repeated taps land on 50 %, 60 %, ... rather than drifting.
            float from = spec.scale(), to = Math.round((from + step) / SCALE_STEP) * SCALE_STEP;
            draft.scaleFrom(control, spec.widthScale, spec.heightScale, to / from, width, height, density);
        }
        changed();
    }

    /** The surface's size: the editor's own once laid out, before that the stream frame it fills. */
    private float surfaceWidth() {
        return surface.getWidth() > 0 ? surface.getWidth() : controller.frame().getWidth();
    }

    private float surfaceHeight() {
        return surface.getHeight() > 0 ? surface.getHeight() : controller.frame().getHeight();
    }

    void setOpacity(int value) {
        for (PadLayout.Control control : PadLayout.Control.values()) {
            if (!editable(control) || (selected != null && control != selected)) continue;
            draft.setOpacity(control, value);
        }
        surface.invalidate();
    }

    void toggleHidden() {
        if (selected == null) return;
        PadLayout.Spec spec = draft.spec(selected);
        spec.hidden = !spec.hidden;
        changed();
    }

    /**
     * Splits the picked face cluster or D-pad into four controls of their own, where its buttons were and at their
     * sizes, and picks the first, so Join is at hand.
     */
    void split() {
        if (selected == null || PadLayout.members(selected).length == 0) return;
        PadLayout.Control cluster = selected;
        draft.split(cluster, surfaceWidth(), surfaceHeight(), density);
        select(PadLayout.members(cluster)[0]);
        changed();
    }

    /** Joins the picked member's cluster again, where and as it was, and picks it. */
    void join() {
        if (selected == null || selected.cluster() == null) return;
        PadLayout.Control cluster = selected.cluster();
        draft.join(cluster);
        select(cluster);
        changed();
    }

    /** Puts the WASD stick in the layout and picks it, so the panel moves off it and it can be placed at once. */
    void addWasd() {
        draft.spec(PadLayout.Control.WASD).hidden = false;
        select(PadLayout.Control.WASD);
    }

    /** Puts the pointer touchpad in the layout and picks it, ready to move, resize and set its mode. */
    void addTouchpad() {
        draft.spec(PadLayout.Control.POINTER_PAD).hidden = false;
        select(PadLayout.Control.POINTER_PAD);
    }

    /** A preset replaces the layout with its selection of controls; Done keeps it, Cancel drops it. */
    void choosePreset() {
        new NavigationDialogBuilder(getContext()).setTitle(R.string.pad_edit_preset)
                .setItems(R.array.pad_preset_names, (dialog, which) -> {
                    draft.applyPreset(PadLayout.Preset.values()[which]);
                    painter.setGlyph(draft.glyph());
                    select(null);
                    changed();
                }).setNegativeButton(android.R.string.cancel, null).create().show();
    }

    /** The face letters and shoulder names of one controller family. */
    void chooseGlyphs() {
        new NavigationDialogBuilder(getContext()).setTitle(R.string.pad_edit_glyphs)
                .setItems(R.array.pad_glyph_names, (dialog, which) -> {
                    draft.setGlyph(PadLayout.Glyph.values()[which]);
                    painter.setGlyph(draft.glyph());
                    changed();
                }).setNegativeButton(android.R.string.cancel, null).create().show();
    }

    /** Switches the pointer touchpad between moving the pointer and moving the camera. */
    void togglePointerMode() {
        draft.setPointerMode(draft.pointerMode() == PadLayout.PointerMode.MOUSE ?
                PadLayout.PointerMode.CAMERA : PadLayout.PointerMode.MOUSE);
        changed();
    }

    private void changed() {
        updatePanel();
        surface.invalidate();
    }

    private PadLayout.Spec reference() {
        if (selected != null) return draft.spec(selected);
        for (PadLayout.Control control : PadLayout.Control.values()) {
            if (editable(control)) return draft.spec(control);
        }
        return draft.spec(PadLayout.Control.L3);
    }

    private void updatePanel() {
        PadLayout.Spec spec = reference();
        String name = selected == null ? getContext().getString(R.string.pad_edit_all) :
                name(getContext(), selected, draft.glyph());
        if (selected != null && spec.hidden) name = getContext().getString(R.string.pad_edit_hidden, name);
        selection.setText(name);
        sizeText.setText(sizeLabel(spec));
        settingOpacity = true;
        opacity.setValue(Math.max(PadLayout.MIN_OPACITY, Math.min(PadLayout.MAX_OPACITY, Math.round(spec.opacity / 5f) * 5)));
        settingOpacity = false;
        hide.setEnabled(selected != null);
        boolean showing = selected != null && spec.hidden;
        hide.setIconResource(showing ? R.drawable.ic_visibility : R.drawable.ic_visibility_off);
        label(hide, getContext().getString(showing ? R.string.pad_edit_show : R.string.pad_edit_hide));
        label(reset, getContext().getString(R.string.pad_edit_reset));
        boolean wasdOut = editable(PadLayout.Control.WASD) && draft.spec(PadLayout.Control.WASD).hidden;
        addWasd.setVisibility(wasdOut ? VISIBLE : GONE);
        boolean touchpadOut = editable(PadLayout.Control.POINTER_PAD) && draft.spec(PadLayout.Control.POINTER_PAD).hidden;
        addTouchpad.setVisibility(touchpadOut ? VISIBLE : GONE);
        boolean pointerPad = selected == PadLayout.Control.POINTER_PAD;
        pointerMode.setVisibility(pointerPad ? VISIBLE : GONE);
        // The pointer touchpad's line says what Camera does, so the mode is found; a short surface shows only that.
        hint.setText(pointerPad ? R.string.pad_edit_hint_camera : R.string.pad_edit_hint);
        hint.setVisibility(pointerPad || !shortSurface ? VISIBLE : GONE);
        // Split for a shown face cluster or D-pad; Join for one of the members that stand in for it.
        split.setVisibility(selected != null && PadLayout.members(selected).length > 0 && !spec.hidden ? VISIBLE : GONE);
        join.setVisibility(selected != null && selected.cluster() != null ? VISIBLE : GONE);
        boolean camera = draft.pointerMode() == PadLayout.PointerMode.CAMERA;
        // The text names what the touchpad does now; the description names what pressing gives, since it switches.
        pointerMode.setText(getContext().getString(camera ? R.string.pad_touchpad_mode_camera : R.string.pad_touchpad_mode_mouse));
        pointerMode.setContentDescription(getContext().getString(camera ? R.string.pad_touchpad_mode_mouse :
                R.string.pad_touchpad_mode_camera));
        glyphs.setText(glyphName(getContext(), draft.glyph()));
        preset.setText(getContext().getString(R.string.pad_edit_preset));
    }

    /** "120 %", or "150 × 80 %" for a control whose width and height differ. */
    static String sizeLabel(PadLayout.Spec spec) {
        int w = Math.round(spec.widthScale * 100), h = Math.round(spec.heightScale * 100);
        return w == h ? String.format(Locale.getDefault(), "%d %%", w) :
                String.format(Locale.getDefault(), "%d \u00d7 %d %%", w, h);
    }

    private static String glyphName(Context context, PadLayout.Glyph glyph) {
        switch (glyph) {
            case PLAYSTATION: return context.getString(R.string.pad_glyph_playstation);
            case NINTENDO: return context.getString(R.string.pad_glyph_nintendo);
            default: return context.getString(R.string.pad_glyph_xbox);
        }
    }

    /** A labelled button, or on a narrow panel its icon alone with the label as description and tooltip. */
    private void label(MaterialButton button, String text) {
        // The style's own padding and minimum width, kept so a panel that widens again restores them.
        if (button.getTag(R.id.pad_editor_hide) == null) {
            button.setTag(R.id.pad_editor_hide, new int[] {button.getPaddingStart(), button.getPaddingEnd(), button.getMinWidth()});
        }
        int[] wide = (int[]) button.getTag(R.id.pad_editor_hide);
        int touch = Math.round(PadLayout.MIN_TOUCH_DP * density);
        // Icon alone: a 48 dp square with the icon in its middle, so four buttons fit a phone's row.
        int side = compactButtons ? Math.max(0, (touch - button.getIconSize()) / 2) : 0;
        button.setText(compactButtons ? null : text);
        button.setContentDescription(compactButtons ? text : null);
        button.setIconPadding(compactButtons ? 0 : Math.round(8 * density));
        button.setIconGravity(compactButtons ? MaterialButton.ICON_GRAVITY_TEXT_START : MaterialButton.ICON_GRAVITY_START);
        button.setPaddingRelative(compactButtons ? side : wide[0], button.getPaddingTop(),
                compactButtons ? side : wide[1], button.getPaddingBottom());
        button.setMinWidth(compactButtons ? touch : wide[2]);
        button.setMinimumWidth(compactButtons ? touch : wide[2]);
        androidx.appcompat.widget.TooltipCompat.setTooltipText(button, compactButtons ? text : null);
    }

    /** The control's name for people, with Xbox's face letters. */
    static String name(Context context, PadLayout.Control control) {
        return name(context, control, PadLayout.Glyph.XBOX);
    }

    /** The control's name for people: its label, or what it is when it has none; a face button's in its glyph set. */
    static String name(Context context, PadLayout.Control control, PadLayout.Glyph glyph) {
        if (control.shape == PadLayout.Shape.FACE_BUTTON) {
            return context.getString(R.string.pad_face_button, PadGlyphs.face(glyph, control.memberIndex()));
        }
        switch (control) {
            case DPAD_UP: return context.getString(R.string.pad_dpad_up);
            case DPAD_DOWN: return context.getString(R.string.pad_dpad_down);
            case DPAD_LEFT: return context.getString(R.string.pad_dpad_left);
            case DPAD_RIGHT: return context.getString(R.string.pad_dpad_right);
            case LEFT_STICK: return context.getString(R.string.pad_left_stick);
            case RIGHT_STICK: return context.getString(R.string.pad_right_stick);
            case DPAD: return context.getString(R.string.pad_dpad);
            case FACE: return context.getString(R.string.pad_face);
            case SELECT: return context.getString(R.string.pad_view);
            case START: return context.getString(R.string.pad_menu);
            case HOME: return context.getString(R.string.pad_home);
            case TOUCHPAD: return context.getString(R.string.pad_touchpad);
            case POINTER_PAD: return context.getString(R.string.pad_pointer_touchpad);
            case WASD: return context.getString(R.string.pad_wasd);
            case LEFT_PAD: return context.getString(R.string.pad_left_trackpad);
            case RIGHT_PAD: return context.getString(R.string.pad_right_trackpad);
            case LEFT_GRIP: return context.getString(R.string.pad_left_grip);
            case RIGHT_GRIP: return context.getString(R.string.pad_right_grip);
            default: return control.name();
        }
    }

    /** Moves the panel to the other half when the picked control sits under it, until the player places it. */
    private void keepPanelClear() {
        if (selected == null || panel.getWidth() == 0 || panelPlaced) return;
        float[] box = draft.bounds(selected, getWidth(), getHeight(), density, new float[4]);
        RectF control = new RectF(box[0], box[1], box[0] + box[2], box[1] + box[3]);
        RectF under = new RectF(panel.getLeft(), panel.getTop(), panel.getRight(), panel.getBottom());
        if (!RectF.intersects(control, under)) return;
        LayoutParams params = (LayoutParams) panel.getLayoutParams();
        boolean controlHigh = control.centerY() < getHeight() / 2f;
        params.gravity = Gravity.CENTER_HORIZONTAL | (controlHigh ? Gravity.BOTTOM : Gravity.TOP);
        params.topMargin = params.bottomMargin = Math.round(16 * density);
        panel.setLayoutParams(params);
    }

    /** Draws the dimmed stream, the grid, every control and the outlines, and takes the editing touches. */
    private final class Surface extends View {
        private final Paint scrim = new Paint(), grid = new Paint();
        private final float[] box = new float[4];
        private final PadPainter.State idle = new PadPainter.State(), pointerIdle = new PadPainter.State();
        private float offsetX, offsetY, pinchDistance, pinchWidth, pinchHeight, anchorX, anchorY;
        private boolean dragging, pinching;
        /** The corner handle being dragged ({@link #corner}'s bits), or -1. */
        private int resizing = -1;

        Surface(Context context) {
            super(context);
            scrim.setColor(0x99000000);
            grid.setColor(0x14FFFFFF);
            grid.setStrokeWidth(Math.max(1, density));
        }

        @Override
        protected void onDraw(Canvas canvas) {
            float width = getWidth(), height = getHeight();
            canvas.drawRect(0, 0, width, height, scrim);
            float step = GRID_DP * 3 * density;
            for (float x = step; x < width; x += step) canvas.drawLine(x, 0, x, height, grid);
            for (float y = step; y < height; y += step) canvas.drawLine(0, y, width, y, grid);
            float base = controller.prefs().oscOpacity / 100f;
            for (PadLayout.Control control : PadLayout.Control.values()) {
                if (!onSurface(control)) continue;
                PadLayout.Spec spec = draft.spec(control);
                draft.bounds(control, width, height, density, box);
                // Show the real opacity, but never so faint that a control cannot be found.
                float alpha = spec.hidden ? .3f : Math.max(.35f, base * spec.opacity / 100f);
                int saved = canvas.saveLayerAlpha(box[0], box[1], box[0] + box[2], box[1] + box[3],
                        Math.round(alpha * 255));
                painter.draw(canvas, control, box[0], box[1], box[2], box[3], previewState(control));
                canvas.restoreToCount(saved);
                painter.drawOutline(canvas, box[0], box[1], box[2], box[3], control == selected, spec.hidden);
            }
        }

        /** What a control shows at rest here: the pointer touchpad its mode's word, as it does in play. */
        PadPainter.State previewState(PadLayout.Control control) {
            if (control != PadLayout.Control.POINTER_PAD) return idle;
            pointerIdle.label = PadPainter.pointerLabel(draft.pointerMode() == PadLayout.PointerMode.CAMERA);
            return pointerIdle;
        }

        private PadLayout.Control hit(float x, float y) {
            PadLayout.Control[] controls = PadLayout.Control.values();
            float slop = 8 * density;
            // A shown control wins over a hidden one's ghost; among each, the last drawn is on top, so it wins.
            for (boolean ghosts : new boolean[] {false, true}) {
                for (int i = controls.length - 1; i >= 0; i--) {
                    if (!onSurface(controls[i]) || draft.spec(controls[i]).hidden != ghosts) continue;
                    draft.bounds(controls[i], getWidth(), getHeight(), density, box);
                    if (x >= box[0] - slop && x <= box[0] + box[2] + slop && y >= box[1] - slop && y <= box[1] + box[3] + slop) {
                        return controls[i];
                    }
                }
            }
            return null;
        }

        /**
         * The picked control's corner handle under ({@code x}, {@code y}), as {@link #RIGHT_CORNER} and
         * {@link #BOTTOM_CORNER} bits, or -1. Each handle takes a 48 dp touch around the corner it is drawn on;
         * inside the control only a corner's quarter counts, so a small control's middle still moves it.
         */
        private int corner(float x, float y) {
            if (selected == null) return -1;
            draft.bounds(selected, getWidth(), getHeight(), density, box);
            float reach = PadLayout.MIN_TOUCH_DP / 2 * density, outline = PadPainter.OUTLINE_DP * density;
            float insideX = Math.min(reach, box[2] / 4), insideY = Math.min(reach, box[3] / 4);
            boolean inside = x > box[0] && x < box[0] + box[2] && y > box[1] && y < box[1] + box[3];
            int best = -1;
            float bestDistance = Float.MAX_VALUE;
            for (int corner = 0; corner < 4; corner++) {
                boolean right = (corner & RIGHT_CORNER) != 0, bottom = (corner & BOTTOM_CORNER) != 0;
                float cornerX = right ? box[0] + box[2] : box[0], cornerY = bottom ? box[1] + box[3] : box[1];
                float handleX = cornerX + (right ? outline : -outline), handleY = cornerY + (bottom ? outline : -outline);
                if (Math.abs(x - handleX) > reach || Math.abs(y - handleY) > reach) continue;
                if (inside && (Math.abs(x - cornerX) > insideX || Math.abs(y - cornerY) > insideY)) continue;
                float distance = (float) Math.hypot(x - handleX, y - handleY);
                if (distance < bestDistance) {
                    best = corner;
                    bestDistance = distance;
                }
            }
            return best;
        }

        @SuppressLint("ClickableViewAccessibility") // Editing gestures; the panel's buttons are the accessible path.
        @Override
        public boolean onTouchEvent(MotionEvent event) {
            switch (event.getActionMasked()) {
                case MotionEvent.ACTION_DOWN: {
                    pinching = false;
                    // A corner handle of the picked control wins over whatever lies under it.
                    resizing = corner(event.getX(), event.getY());
                    if (resizing >= 0) {
                        dragging = false;
                        draft.bounds(selected, getWidth(), getHeight(), density, box);
                        boolean right = (resizing & RIGHT_CORNER) != 0, bottom = (resizing & BOTTOM_CORNER) != 0;
                        // The dragged corner follows the finger from where it took hold; the opposite one stays.
                        offsetX = (right ? box[0] + box[2] : box[0]) - event.getX();
                        offsetY = (bottom ? box[1] + box[3] : box[1]) - event.getY();
                        anchorX = right ? box[0] : box[0] + box[2];
                        anchorY = bottom ? box[1] : box[1] + box[3];
                        break;
                    }
                    PadLayout.Control control = hit(event.getX(), event.getY());
                    select(control);
                    dragging = control != null;
                    if (dragging) {
                        draft.bounds(control, getWidth(), getHeight(), density, box);
                        offsetX = box[0] + box[2] / 2 - event.getX();
                        offsetY = box[1] + box[3] / 2 - event.getY();
                    }
                    break;
                }
                case MotionEvent.ACTION_POINTER_DOWN:
                    if (selected != null && event.getPointerCount() == 2) {
                        pinching = true;
                        dragging = false;
                        resizing = -1;
                        pinchDistance = distance(event);
                        pinchWidth = draft.spec(selected).widthScale;
                        pinchHeight = draft.spec(selected).heightScale;
                    }
                    break;
                case MotionEvent.ACTION_MOVE:
                    if (pinching && event.getPointerCount() >= 2 && pinchDistance > 0) {
                        draft.scaleFrom(selected, pinchWidth, pinchHeight, distance(event) / pinchDistance, getWidth(),
                                getHeight(), density);
                        changed();
                    } else if (resizing >= 0 && selected != null) {
                        resize(event.getX() + offsetX, event.getY() + offsetY);
                    } else if (dragging && selected != null) {
                        float grid = GRID_DP * density;
                        float x = Math.round((event.getX() + offsetX) / grid) * grid;
                        float y = Math.round((event.getY() + offsetY) / grid) * grid;
                        draft.moveTo(selected, x, y, getWidth(), getHeight(), density);
                        invalidate();
                    }
                    break;
                case MotionEvent.ACTION_POINTER_UP:
                    // The pinch ends; the remaining finger does not jump the control.
                    pinching = false;
                    break;
                case MotionEvent.ACTION_UP:
                case MotionEvent.ACTION_CANCEL:
                    dragging = false;
                    pinching = false;
                    resizing = -1;
                    keepPanelClear();
                    updatePanel();
                    break;
                default:
                    break;
            }
            return true;
        }

        /**
         * Puts the dragged corner at ({@code x}, {@code y}), on the grid or flush with the surface's edge, and sizes
         * the picked control's width and height between it and the opposite corner, which stays: never smaller than
         * a full touch, never past the surface.
         */
        private void resize(float x, float y) {
            float width = getWidth(), height = getHeight();
            float u = PadLayout.unit(width, height, density);
            float leastWide = PadLayout.width(selected, u, density, PadLayout.MIN_SCALE);
            float leastHigh = PadLayout.height(selected, u, density, PadLayout.MIN_SCALE);
            x = snapEdge(x, width);
            y = snapEdge(y, height);
            float left, right, top, bottom;
            if ((resizing & RIGHT_CORNER) != 0) {
                left = anchorX;
                right = Math.max(anchorX + leastWide, Math.min(width, x));
            } else {
                right = anchorX;
                left = Math.min(anchorX - leastWide, Math.max(0, x));
            }
            if ((resizing & BOTTOM_CORNER) != 0) {
                top = anchorY;
                bottom = Math.max(anchorY + leastHigh, Math.min(height, y));
            } else {
                bottom = anchorY;
                top = Math.min(anchorY - leastHigh, Math.max(0, y));
            }
            draft.setBounds(selected, left, top, right - left, bottom - top, width, height, density);
            changed();
        }

        /** A dragged edge lands on the grid, or flush with the surface's edge when that is nearer. */
        private float snapEdge(float position, float size) {
            float grid = GRID_DP * density;
            float snapped = Math.round(position / grid) * grid;
            return Math.abs(size - position) < Math.abs(snapped - position) ? size : snapped;
        }

        private float distance(MotionEvent event) {
            return (float) Math.hypot(event.getX(0) - event.getX(1), event.getY(0) - event.getY(1));
        }
    }
}
