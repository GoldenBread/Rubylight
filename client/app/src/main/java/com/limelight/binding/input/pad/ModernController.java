package com.limelight.binding.input.pad;

import android.content.Context;
import android.content.SharedPreferences;
import android.content.res.Configuration;
import android.graphics.Rect;
import android.os.Handler;
import android.os.Looper;
import android.view.Gravity;
import android.view.View;
import android.widget.FrameLayout;

import com.limelight.binding.input.HeldInputState;
import com.limelight.binding.input.LayoutGamepad;
import com.limelight.binding.input.touch.PointerAcceleration;
import com.limelight.binding.input.virtual_controller.VirtualControllerConfigurationLoader;
import com.limelight.preferences.PreferenceConfiguration;

import org.json.JSONException;
import org.json.JSONObject;

import java.util.EnumMap;
import java.util.HashMap;
import java.util.List;
import java.util.Map;

/**
 * The modern on-screen controller: analog sticks that float or stay put, a D-pad, the face cluster,
 * shoulders, triggers, stick clicks and View, Menu and Home, a WASD stick and a pointer touchpad the layout
 * editor can add, and Xbox, PlayStation or Nintendo glyphs. Controls are separate views in the stream's frame
 * so touches between them reach the stream. Its input is the on-screen gamepad, the same report the classic
 * controller sends (the WASD stick sends keys, the pointer touchpad mouse input), and its layout is kept per
 * orientation for all games or for one game. With the Steam Controller controls setting it adds that
 * controller's trackpads, back buttons and grip touch, sent as the classic controller sends them.
 */
public final class ModernController {
    static final String PREFS = "osc_modern";
    static final String GLOBAL_KEY = PadLayoutKeys.ROOT;
    /** The classic controller's saved layout, read once to carry it over. */
    static final String CLASSIC_PREFS = VirtualControllerConfigurationLoader.OSC_PREFERENCE;
    /** The classic controller's element ids, for the one-time carry-over. */
    private static final int[] CLASSIC_BOXES = {1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17,
            18, 19, 20, 21, 22, 23, 24, 25};
    /** The classic controller's resends: some hosts drop a report that follows another closely. */
    private static final long[] RESEND_MS = {25, 50, 75};

    private final Context context;
    private final FrameLayout frame;
    private final PreferenceConfiguration prefs;
    private final String gameKey;
    private final SharedPreferences store;
    private final LayoutGamepad gamepad;
    /** The WASD stick's keys, Windows virtual-key codes through the sink's {@code key}. */
    private final WasdKeys.Keys keys;
    private final PadPainter painter;
    private final Handler handler = new Handler(Looper.getMainLooper());
    private final Map<PadLayout.Control, PadControlView> views = new EnumMap<>(PadLayout.Control.class);
    private final Runnable resend;
    private final View.OnLayoutChangeListener resize;
    private PadLayout layout;
    private boolean perGame, shown = true, landscape;
    private PadEditor editor;
    private final float[] box = new float[4];
    private TouchSink touchSink;
    private PointerSink pointerSink;

    /** Where the Steam Controller trackpads' touches go: touchpad 0 of the on-screen controls' pad. */
    public interface TouchSink {
        void touch(byte eventType, int pointerId, float x, float y, float pressure);
    }

    /** Where the pointer touchpad's mouse output goes; without one it sends nothing. */
    public interface PointerSink {
        /** Relative pointer movement in pixels. */
        void move(int dx, int dy);
        /** A mouse button from {@link com.limelight.nvstream.input.MouseButtonPacket}. */
        void button(int button, boolean down);
        /** Scroll in high-resolution units, 120 to a notch; either may be zero. */
        void scroll(int vertical, int horizontal);
    }

    /**
     * @param gameKey {@link com.limelight.binding.input.TouchLayout#profileKey} of the game, or null when
     *                the stream has none; then only the layout for all games exists.
     */
    public ModernController(Context context, FrameLayout frame, HeldInputState.Sink sink,
                            PreferenceConfiguration prefs, String gameKey) {
        this.context = context;
        this.frame = frame;
        this.prefs = prefs;
        this.gameKey = gameKey == null || gameKey.isEmpty() ? null : gameKey;
        this.store = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE);
        this.gamepad = new LayoutGamepad(sink);
        this.keys = sink::key;
        this.painter = new PadPainter(context);
        this.landscape = landscape();
        resend = gamepad::resend;
        resize = (v, l, t, r, b, ol, ot, or, ob) -> {
            if (r - l != or - ol || b - t != ob - ot) place();
        };
        load();
    }

    private boolean landscape() {
        return context.getResources().getConfiguration().orientation == Configuration.ORIENTATION_LANDSCAPE;
    }

    private void load() {
        // This game's own layout first, in this version or the older one, then the layout shared by all games. A
        // game's own choice always wins, so a shared layout saved later never shadows it.
        String text = store.getString(PadLayoutKeys.key(landscape, gameKey), null);
        perGame = text != null && gameKey != null;
        if (text == null && gameKey != null) {
            text = store.getString(PadLayoutKeys.legacyKey(gameKey), null);
            perGame = text != null;
        }
        if (text == null) text = store.getString(PadLayoutKeys.key(landscape, null), null);
        if (text == null) text = store.getString(PadLayoutKeys.legacyKey(null), null);
        layout = PadLayout.parse(text);
        if (text == null) layout.copyFrom(migrateClassic());
        painter.setGlyph(layout.glyph());
    }

    /**
     * The first time the modern controller is shown with nothing saved, a classic layout the player arranged is
     * carried over once, so switching styles keeps the controls where their thumbs know them.
     */
    private PadLayout migrateClassic() {
        if (store.getBoolean("classic-migrated", false)) return new PadLayout();
        SharedPreferences classic = context.getSharedPreferences(CLASSIC_PREFS, Context.MODE_PRIVATE);
        Map<Integer, float[]> boxes = new HashMap<>();
        for (int id : CLASSIC_BOXES) {
            String json = classic.getString(String.valueOf(id), null);
            if (json == null) continue;
            try {
                JSONObject config = new JSONObject(json);
                boxes.put(id, new float[] {config.getInt("LEFT"), config.getInt("TOP"),
                        config.getInt("WIDTH"), config.getInt("HEIGHT")});
            } catch (JSONException ignored) {
                // A corrupt classic control is simply not carried over.
            }
        }
        if (boxes.isEmpty()) return new PadLayout();
        android.util.DisplayMetrics metrics = context.getResources().getDisplayMetrics();
        // The classic layout is arranged in landscape pixels; a first modern start in portrait must map it with the
        // same dimensions, because the shared key it is stored under is used in both orientations.
        float width = Math.max(metrics.widthPixels, metrics.heightPixels);
        float height = Math.min(metrics.widthPixels, metrics.heightPixels);
        PadLayout migrated = ClassicLayoutMigration.migrate(boxes, width, height, metrics.density);
        if (!migrated.isDefault()) {
            store.edit().putString(PadLayoutKeys.legacyKey(null), migrated.serialize())
                    .putBoolean("classic-migrated", true).apply();
        }
        return migrated;
    }

    /** Sends the Steam Controller trackpads' touches here; without a sink they send nothing. */
    public void setTouchSink(TouchSink sink) {
        touchSink = sink;
    }

    private void touch(byte eventType, int pointerId, float x, float y, float pressure) {
        if (touchSink != null) touchSink.touch(eventType, pointerId, x, y, pressure);
    }

    /** Sends the pointer touchpad's mouse output here; without a sink it sends nothing. */
    public void setPointerSink(PointerSink sink) {
        pointerSink = sink;
        for (PadControlView view : views.values()) view.setPointerSink(sink);
    }

    /** Whether the settings keep this control off whatever the layout says. */
    boolean forcedHidden(PadLayout.Control control) {
        // As on the classic controller, the Steam Controller controls follow their own setting alone.
        if (control.steam) return !prefs.oscSteamControls;
        if (prefs.onlyL3R3) return control != PadLayout.Control.L3 && control != PadLayout.Control.R3;
        return (control == PadLayout.Control.HOME && !prefs.showGuideButton) ||
                (control == PadLayout.Control.TOUCHPAD && !prefs.showTouchpadButton);
    }

    boolean shows(PadLayout layout, PadLayout.Control control) {
        return !forcedHidden(control) && !layout.spec(control).hidden;
    }

    /** Builds the controls again from the saved layout and the settings. */
    public void refreshLayout() {
        releaseInput();
        removeElements();
        load();
        painter.setGlyph(layout.glyph());
        String curve = prefs.trackpadAcceleration ? prefs.trackpadAccelerationCurve : PointerAcceleration.LINEAR;
        for (PadLayout.Control control : PadLayout.Control.values()) {
            if (forcedHidden(control)) continue;
            PadControlView view = new PadControlView(context, control, painter, gamepad, this::changed, keys);
            view.configureStick(prefs.oscFloatingSticks, prefs.oscStickDeadzone / 100f);
            view.setHaptics(prefs.vibrateOsc);
            view.setTouchSink(this::touch);
            view.setPointerSink(pointerSink);
            if (control == PadLayout.Control.POINTER_PAD) {
                view.configurePointer(layout.pointerMode() == PadLayout.PointerMode.CAMERA, curve);
            }
            views.put(control, view);
            frame.addView(view, new FrameLayout.LayoutParams(0, 0, Gravity.TOP | Gravity.START));
        }
        // The sticks' knobs above every control, so a knob pushed past its base covers a neighbour.
        for (PadControlView view : views.values()) {
            View knob = view.knobLayer();
            if (knob != null) frame.addView(knob, new FrameLayout.LayoutParams(0, 0, Gravity.TOP | Gravity.START));
        }
        frame.addOnLayoutChangeListener(resize);
        place();
    }

    /** Puts every control where the layout says on the frame's current size. */
    void place() {
        int width = frame.getWidth(), height = frame.getHeight();
        float density = context.getResources().getDisplayMetrics().density;
        for (Map.Entry<PadLayout.Control, PadControlView> entry : views.entrySet()) {
            PadLayout.Control control = entry.getKey();
            PadControlView view = entry.getValue();
            boolean visible = shown && editor == null && shows(layout, control);
            if (!visible) view.release();
            view.setVisibility(visible ? View.VISIBLE : View.GONE);
            if (width == 0 || height == 0) continue;
            layout.bounds(control, width, height, density, box);
            FrameLayout.LayoutParams params = (FrameLayout.LayoutParams) view.getLayoutParams();
            params.width = Math.round(box[2]);
            params.height = Math.round(box[3]);
            params.leftMargin = Math.round(box[0]);
            params.topMargin = Math.round(box[1]);
            view.setLayoutParams(params);
            View knob = view.knobLayer();
            if (knob != null) {
                // The control's box grown by the knob's overhang on every side; the control keeps the box.
                int overhang = view.knobOverhang(params.width, params.height);
                FrameLayout.LayoutParams knobParams = (FrameLayout.LayoutParams) knob.getLayoutParams();
                knobParams.width = params.width + 2 * overhang;
                knobParams.height = params.height + 2 * overhang;
                knobParams.leftMargin = params.leftMargin - overhang;
                knobParams.topMargin = params.topMargin - overhang;
                knob.setLayoutParams(knobParams);
            }
            view.setRestingAlpha(prefs.oscOpacity / 100f * layout.spec(control).opacity / 100f);
        }
    }

    private void changed() {
        handler.removeCallbacks(resend);
        for (long delay : RESEND_MS) handler.postDelayed(resend, delay);
    }

    public void show() {
        shown = true;
        place();
        // What sits below the top row (the microphone button) follows the controls back, as when they hide.
        if (onLayoutChanged != null) onLayoutChanged.run();
    }

    public void hide() {
        shown = false;
        releaseInput();
        place();
    }

    /**
     * The window turned: loads this orientation's own layout. An editor's unsaved draft is left alone, since its
     * Done writes to the orientation the stream is in when it closes.
     */
    public void onOrientationChanged() {
        boolean now = landscape();
        if (now == landscape) return;
        landscape = now;
        // An open editor keeps its draft; Done saves it to the orientation the stream is in when it closes.
        if (editor != null) return;
        releaseInput();
        load();
        applyAppearance();
        place();
        if (onLayoutChanged != null) onLayoutChanged.run();
    }

    /** Redraws the controls after the glyphs or the pointer touchpad's mode changed. */
    private void applyAppearance() {
        painter.setGlyph(layout.glyph());
        String curve = prefs.trackpadAcceleration ? prefs.trackpadAccelerationCurve : PointerAcceleration.LINEAR;
        for (Map.Entry<PadLayout.Control, PadControlView> entry : views.entrySet()) {
            PadControlView view = entry.getValue();
            if (entry.getKey() == PadLayout.Control.POINTER_PAD) {
                view.configurePointer(layout.pointerMode() == PadLayout.PointerMode.CAMERA, curve);
            }
            view.invalidate();
        }
    }

    public void releaseInput() {
        for (PadControlView view : views.values()) view.release();
        handler.removeCallbacks(resend);
        gamepad.releaseAll();
    }

    public void removeElements() {
        frame.removeOnLayoutChangeListener(resize);
        for (PadControlView view : views.values()) {
            frame.removeView(view);
            if (view.knobLayer() != null) frame.removeView(view.knobLayer());
        }
        views.clear();
    }

    /** Removes the controls and any open editor for good. */
    public void destroy() {
        if (editor != null) finishEditing(false);
        releaseInput();
        removeElements();
    }

    /** The visible controls, in the frame's coordinates, so overlays keep clear of them. */
    public void collectVisibleBounds(List<Rect> out) {
        for (PadControlView view : views.values()) {
            com.limelight.ui.OverlayPlacement.addVisible(view, frame, out);
        }
    }

    /**
     * The bottom of the top row (View, Home, Menu, and the Touchpad below them) in the frame's pixels, so the microphone button can sit
     * below it; NaN when none of them shows.
     */
    public float topRowBottom() {
        int width = frame.getWidth(), height = frame.getHeight();
        if (!shown || editor != null || width == 0 || height == 0) return Float.NaN;
        float density = context.getResources().getDisplayMetrics().density, bottom = Float.NaN;
        for (PadLayout.Control control : new PadLayout.Control[] {PadLayout.Control.SELECT, PadLayout.Control.HOME,
                PadLayout.Control.START, PadLayout.Control.TOUCHPAD}) {
            if (!shows(layout, control)) continue;
            layout.bounds(control, width, height, density, box);
            // Only the middle of the top edge counts: a row moved elsewhere leaves the button its own place.
            if (box[1] > height / 3f || Math.abs(box[0] + box[2] / 2 - width / 2f) > width / 4f) continue;
            bottom = Float.isNaN(bottom) ? box[1] + box[3] : Math.max(bottom, box[1] + box[3]);
        }
        return bottom;
    }

    public boolean isEditing() {
        return editor != null;
    }

    /** Opens the layout editor over the stream; input pauses until it closes. */
    public void startEditing() {
        if (editor != null) return;
        releaseInput();
        editor = new PadEditor(context, this, painter, layout.copy(), perGame, gameKey != null);
        place();
        frame.addView(editor, new FrameLayout.LayoutParams(FrameLayout.LayoutParams.MATCH_PARENT,
                FrameLayout.LayoutParams.MATCH_PARENT));
    }

    /** Closes the editor, keeping its layout when {@code save} is true. */
    public void finishEditing(boolean save) {
        if (editor == null) return;
        PadEditor closing = editor;
        editor = null;
        frame.removeView(closing);
        if (save) save(closing.draft(), closing.perGame());
        else load(); // Cancel after a rotation shows the new orientation's saved layout, not the old one.
        // Cancel also restores the glyphs and pointer mode the editor previewed.
        applyAppearance();
        place();
        if (onLayoutChanged != null) onLayoutChanged.run();
    }

    private Runnable onLayoutChanged;

    /** Runs when the editor closes, so what sits below the top row (the microphone button) can move with it. */
    public void setOnLayoutChanged(Runnable listener) {
        onLayoutChanged = listener;
    }

    void save(PadLayout draft, boolean forThisGame) {
        layout.copyFrom(draft);
        SharedPreferences.Editor edit = store.edit();
        String text = draft.serialize();
        if (forThisGame && gameKey != null) {
            edit.putString(PadLayoutKeys.key(landscape, gameKey), text);
        } else {
            // Saving for all games drops this game's own layout in this orientation so the shared one shows
            // here. The other orientation keeps its own, so saving in portrait never deletes a landscape one.
            edit.putString(PadLayoutKeys.key(landscape, null), text);
            if (gameKey != null) {
                edit.remove(PadLayoutKeys.key(landscape, gameKey));
                // The old single per-game layout is both orientations' fallback: hand it to the other orientation
                // before removing it, so a downgrade-sized layout is not lost there.
                String legacy = store.getString(PadLayoutKeys.legacyKey(gameKey), null);
                String other = PadLayoutKeys.key(!landscape, gameKey);
                if (legacy != null && !store.contains(other)) edit.putString(other, legacy);
                edit.remove(PadLayoutKeys.legacyKey(gameKey));
            }
        }
        edit.apply();
        perGame = forThisGame && gameKey != null;
        applyAppearance();
    }

    /** Forgets every saved modern layout, for all games and for each game; Reset means default, not the old classic layout. */
    public static void clearLayouts(Context context) {
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().clear()
                .putBoolean("classic-migrated", true).apply();
    }

    PadLayout layout() {
        return layout;
    }

    PreferenceConfiguration prefs() {
        return prefs;
    }

    PadControlView view(PadLayout.Control control) {
        return views.get(control);
    }

    FrameLayout frame() {
        return frame;
    }
}
