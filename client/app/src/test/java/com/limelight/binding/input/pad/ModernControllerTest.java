package com.limelight.binding.input.pad;

import android.app.Application;
import android.content.Context;
import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.graphics.Color;
import android.view.MotionEvent;
import android.view.View;
import android.widget.FrameLayout;

import com.limelight.binding.input.HeldInputState;
import com.limelight.binding.input.StickMath;
import com.limelight.nvstream.input.ControllerPacket;
import com.limelight.nvstream.input.MouseButtonPacket;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.ui.UiNavigationTest;

import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.GraphicsMode;
import org.robolectric.shadows.ShadowLooper;
import org.robolectric.util.ReflectionHelpers;

import java.util.ArrayList;
import java.util.List;

import static org.junit.Assert.*;
import static org.robolectric.Shadows.shadowOf;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 30, application = Application.class, qualifiers = "w800dp-h360dp-land-xhdpi")
public class ModernControllerTest {
    private static final String GAME = "pc-1/app-7";
    private final List<int[]> reports = new ArrayList<>();
    /** Keys from the WASD stick, as "+W" and "-W"; only the WASD stick sends any. */
    private final List<String> keys = new ArrayList<>();
    private final HeldInputState.Sink sink = new HeldInputState.Sink() {
        @Override public void key(int key, boolean down) { keys.add((down ? "+" : "-") + (char) key); }
        @Override public void mouse(int button, boolean down) { fail("Gamepad only"); }
        @Override public void gamepad(int buttons, short lx, short ly, short rx, short ry, byte lt, byte rt) {
            reports.add(new int[] {buttons, lx, ly, rx, ry, lt & 0xFF, rt & 0xFF});
        }
    };
    private FrameLayout frame;
    private PreferenceConfiguration prefs;
    /** The pointer touchpad's output, recorded by the tests below. */
    private final List<int[]> pointerMoves = new ArrayList<>();
    private final List<Integer> pointerButtons = new ArrayList<>();
    private final List<int[]> pointerScrolls = new ArrayList<>();
    private final ModernController.PointerSink pointerSink = new ModernController.PointerSink() {
        @Override public void move(int dx, int dy) { pointerMoves.add(new int[] {dx, dy}); }
        @Override public void button(int button, boolean down) { pointerButtons.add(down ? button : -button); }
        @Override public void scroll(int vertical, int horizontal) { pointerScrolls.add(new int[] {vertical, horizontal}); }
    };

    @Before
    public void setUp() {
        Context app = RuntimeEnvironment.getApplication();
        ModernController.clearLayouts(app);
        app.getSharedPreferences(com.limelight.binding.input.virtual_controller.VirtualControllerConfigurationLoader.OSC_PREFERENCE,
                Context.MODE_PRIVATE).edit().clear().commit();
        UiNavigationTest.Screen screen = Robolectric.buildActivity(UiNavigationTest.Screen.class).setup().visible().get();
        frame = new FrameLayout(screen);
        screen.setContentView(frame, new FrameLayout.LayoutParams(1600, 720));
        prefs = new PreferenceConfiguration();
        prefs.oscStyle = PreferenceConfiguration.OSC_STYLE_MODERN;
        prefs.oscOpacity = 90;
        // HOME follows the guide button setting; on here so every control is covered.
        prefs.showGuideButton = true;
        prefs.showTouchpadButton = true;
        pointerMoves.clear();
        pointerButtons.clear();
        pointerScrolls.clear();
        layout();
    }

    private void layout() {
        frame.measure(View.MeasureSpec.makeMeasureSpec(1600, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(720, View.MeasureSpec.EXACTLY));
        frame.layout(0, 0, 1600, 720);
    }

    private ModernController controller(String game) {
        ModernController controller = new ModernController(frame.getContext(), frame, sink, prefs, game);
        controller.refreshLayout();
        controller.show();
        layout();
        return controller;
    }

    private int[] last() {
        return reports.get(reports.size() - 1);
    }

    @Test public void everyControlIsItsOwnViewInTheStreamFrame() {
        ModernController controller = controller(GAME);
        // Every control, plus each stick's knob layer above them (the WASD stick's too, though it starts hidden); the
        // Steam Controller controls only with their setting.
        assertEquals(PadLayout.Control.values().length - 8 + 3, frame.getChildCount());
        assertNull(controller.view(PadLayout.Control.LEFT_PAD));
        assertEquals(View.GONE, controller.view(PadLayout.Control.WASD).getVisibility());
        PadControlView face = controller.view(PadLayout.Control.FACE);
        assertEquals(View.VISIBLE, face.getVisibility());
        assertTrue(face.getWidth() > 0);
        controller.destroy();
        assertEquals(0, frame.getChildCount());
    }

    @Test public void aFaceTouchSendsItsButtonAndLettingGoReleasesIt() {
        PadControlView face = controller(GAME).view(PadLayout.Control.FACE);
        float size = Math.min(face.getWidth(), face.getHeight());
        face.down(0, .5f * size, .78f * size, 100);
        assertEquals(ControllerPacket.A_FLAG, last()[0]);
        // Sliding to the gap between A and B holds both, as a thumb resting on two buttons does.
        face.move(0, .64f * size, .64f * size);
        assertEquals(ControllerPacket.A_FLAG | ControllerPacket.B_FLAG, last()[0]);
        face.up(0, 200);
        assertEquals(0, last()[0]);
    }

    @Test public void twoFingersOnTheFaceClusterHoldTwoButtons() {
        PadControlView face = controller(GAME).view(PadLayout.Control.FACE);
        float size = Math.min(face.getWidth(), face.getHeight());
        face.down(0, .5f * size, .78f * size, 100);
        face.down(1, .5f * size, .22f * size, 110);
        assertEquals(ControllerPacket.A_FLAG | ControllerPacket.Y_FLAG, last()[0]);
        face.up(0, 150);
        assertEquals(ControllerPacket.Y_FLAG, last()[0]);
    }

    @Test public void aFloatingStickCentersUnderTheThumbAndReportsItsPush() {
        PadControlView stick = controller(GAME).view(PadLayout.Control.LEFT_STICK);
        float size = Math.min(stick.getWidth(), stick.getHeight());
        float travel = PadPainter.travel(size);
        reports.clear();
        stick.down(0, size * .3f, size * .7f, 100);
        // Landing is not a push.
        assertTrue(reports.isEmpty() || (last()[1] == 0 && last()[2] == 0));
        stick.move(0, size * .3f + travel, size * .7f);
        assertEquals(32766, last()[1]);
        assertEquals(0, last()[2]);
        stick.up(0, 400);
        assertEquals(0, last()[1]);
    }

    /** A controller whose saved layout shows the WASD stick, with fixed sticks so a push starts at the drawn center. */
    private ModernController wasdController() {
        prefs.oscFloatingSticks = false;
        ModernController first = controller(GAME);
        PadLayout draft = first.layout().copy();
        draft.spec(PadLayout.Control.WASD).hidden = false;
        first.save(draft, false);
        first.destroy();
        ModernController controller = controller(GAME);
        assertEquals(View.VISIBLE, controller.view(PadLayout.Control.WASD).getVisibility());
        return controller;
    }

    /** Puts a finger on the WASD stick pushed {@code push} of full travel toward {@code degrees} (0 right, 90 up). */
    private static void push(PadControlView stick, boolean down, double degrees, float push) {
        float size = Math.min(stick.getWidth(), stick.getHeight()), travel = PadPainter.travel(size);
        float x = stick.getWidth() / 2f + (float) Math.cos(Math.toRadians(degrees)) * travel * push;
        float y = stick.getHeight() / 2f - (float) Math.sin(Math.toRadians(degrees)) * travel * push;
        if (down) stick.down(0, x, y, 100); else stick.move(0, x, y);
    }

    private void assertEveryKeyLetGo(String when) {
        int downs = 0, ups = 0;
        for (String key : keys) if (key.startsWith("+")) downs++; else ups++;
        assertTrue(when + ": something was held first " + keys, downs > 0);
        assertEquals(when + ": every key sent down is sent up " + keys, downs, ups);
    }

    @Test public void theWasdStickHoldsKeysForEachOfItsEightDirectionsAndNoGamepad() {
        PadControlView stick = wasdController().view(PadLayout.Control.WASD);
        String[] expected = {"D", "WD", "W", "WA", "A", "SA", "S", "SD"};
        reports.clear();
        for (int sector = 0; sector < 8; sector++) {
            keys.clear();
            push(stick, true, sector * 45, 1);
            List<String> want = new ArrayList<>();
            for (char key : expected[sector].toCharArray()) want.add("+" + key);
            assertEquals(sector * 45 + " degrees", want, keys);
            stick.up(0, 500);
            assertEquals(WasdKeys.NONE, stick.wasd().sector());
        }
        for (int[] report : reports) assertEquals("The WASD stick moves no gamepad stick", 0, report[1] | report[2] | report[0]);
    }

    @Test public void theWasdStickUsesTheModernStickDeadzone() {
        prefs.oscStickDeadzone = 20;
        PadControlView stick = wasdController().view(PadLayout.Control.WASD);
        push(stick, true, 90, .15f);
        push(stick, false, 180, .18f);
        assertTrue("Inside the deadzone nothing is held: " + keys, keys.isEmpty());
        push(stick, false, 90, .6f);
        assertEquals(List.of("+W"), keys);
    }

    @Test public void theWasdStickTicksForADirectionAndDifferentlyForADiagonalOnlyOnAChange() {
        prefs.vibrateOsc = true;
        PadControlView stick = wasdController().view(PadLayout.Control.WASD);
        int marker = android.view.HapticFeedbackConstants.LONG_PRESS;
        stick.performHapticFeedback(marker);
        // Landing in the deadzone is quiet: the stick answers keys, not touches.
        push(stick, true, 90, 0);
        assertEquals(marker, shadowOf(stick).lastHapticFeedbackPerformed());
        push(stick, false, 90, 1);
        int direction = shadowOf(stick).lastHapticFeedbackPerformed();
        assertEquals(android.view.HapticFeedbackConstants.CONTEXT_CLICK, direction);
        // Moving on inside W repeats nothing.
        stick.performHapticFeedback(marker);
        push(stick, false, 100, .8f);
        push(stick, false, 80, 1);
        assertEquals(marker, shadowOf(stick).lastHapticFeedbackPerformed());
        push(stick, false, 135, 1);
        int diagonal = shadowOf(stick).lastHapticFeedbackPerformed();
        assertEquals(android.view.HapticFeedbackConstants.VIRTUAL_KEY, diagonal);
        assertNotEquals("A diagonal feels unlike a direction", direction, diagonal);
        push(stick, false, 180, 1);
        assertEquals(direction, shadowOf(stick).lastHapticFeedbackPerformed());
        // Back to the center and lifting are quiet.
        stick.performHapticFeedback(marker);
        push(stick, false, 0, 0);
        stick.up(0, 500);
        assertEquals(marker, shadowOf(stick).lastHapticFeedbackPerformed());
    }

    @Test public void theWasdStickFollowsVibrateOnPress() {
        prefs.vibrateOsc = false;
        PadControlView stick = wasdController().view(PadLayout.Control.WASD);
        int marker = android.view.HapticFeedbackConstants.LONG_PRESS;
        stick.performHapticFeedback(marker);
        push(stick, true, 90, 1);
        push(stick, false, 45, 1);
        assertEquals(List.of("+W", "+D"), keys);
        assertEquals(marker, shadowOf(stick).lastHapticFeedbackPerformed());
    }

    @Test public void theWasdStickLetsGoOfItsKeysOnEveryWayOut() {
        ModernController controller = wasdController();
        PadControlView stick = controller.view(PadLayout.Control.WASD);

        push(stick, true, 45, 1);
        stick.up(0, 500);
        assertEveryKeyLetGo("Lift");

        keys.clear();
        push(stick, true, 135, 1);
        long now = android.os.SystemClock.uptimeMillis();
        MotionEvent cancel = MotionEvent.obtain(now, now, MotionEvent.ACTION_CANCEL, 0, 0, 0);
        stick.onTouchEvent(cancel);
        cancel.recycle();
        assertEveryKeyLetGo("Cancel");

        keys.clear();
        push(stick, true, 225, 1);
        controller.releaseInput();
        assertEveryKeyLetGo("Focus loss");

        keys.clear();
        push(stick, true, 315, 1);
        controller.hide();
        assertEveryKeyLetGo("Stream end or picture-in-picture");
        controller.show();

        keys.clear();
        push(stick, true, 90, 1);
        controller.startEditing();
        assertEveryKeyLetGo("Editing");
        PadEditor editor = (PadEditor) frame.getChildAt(frame.getChildCount() - 1);
        editor.select(PadLayout.Control.WASD);
        editor.toggleHidden();
        controller.finishEditing(true);
        assertEquals("Hiding it in the editor removes it", View.GONE, stick.getVisibility());

        // Shown again, then removed with the controller.
        PadLayout draft = controller.layout().copy();
        draft.spec(PadLayout.Control.WASD).hidden = false;
        controller.save(draft, false);
        controller.refreshLayout();
        layout();
        stick = controller.view(PadLayout.Control.WASD);
        keys.clear();
        push(stick, true, 270, 1);
        controller.refreshLayout();
        assertEveryKeyLetGo("Rebuilt from the settings");

        stick = controller.view(PadLayout.Control.WASD);
        layout();
        keys.clear();
        push(stick, true, 180, 1);
        controller.destroy();
        assertEveryKeyLetGo("Removed");
    }

    @Test public void theEditorAddsTheWasdStickWithItsOwnButton() {
        ModernController controller = controller(GAME);
        assertEquals(View.GONE, controller.view(PadLayout.Control.WASD).getVisibility());
        controller.startEditing();
        PadEditor editor = (PadEditor) frame.getChildAt(frame.getChildCount() - 1);
        View add = editor.findViewById(com.limelight.R.id.pad_editor_add_wasd);
        assertEquals(View.VISIBLE, add.getVisibility());
        assertTrue(add.performClick());
        assertEquals(PadLayout.Control.WASD, editor.selected());
        assertFalse(editor.draft().spec(PadLayout.Control.WASD).hidden);
        assertEquals("Added, the button goes", View.GONE, add.getVisibility());
        editor.toggleHidden();
        assertEquals("Hidden again, it comes back", View.VISIBLE, add.getVisibility());
        editor.toggleHidden();
        controller.finishEditing(true);
        layout();
        assertEquals(View.VISIBLE, controller.view(PadLayout.Control.WASD).getVisibility());
        assertFalse(controller("pc-1/app-8").layout().spec(PadLayout.Control.WASD).hidden);
    }

    /** Left out, the WASD stick has no ghost: a tap where it would sit is empty space and drops the pick. */
    @Test public void theEditorsEmptyMiddleDeselectsUntilTheWasdStickIsAdded() {
        ModernController controller = controller(GAME);
        controller.startEditing();
        layout();
        PadEditor editor = (PadEditor) frame.getChildAt(frame.getChildCount() - 1);
        View surface = editor.getChildAt(0);
        float density = frame.getContext().getResources().getDisplayMetrics().density;
        float[] box = new float[4];
        editor.draft().bounds(PadLayout.Control.WASD, surface.getWidth(), surface.getHeight(), density, box);
        float x = box[0] + box[2] / 2, y = box[1] + box[3] / 2;

        editor.select(PadLayout.Control.FACE);
        tap(surface, x, y);
        assertNull("Empty space deselects", editor.selected());

        // A hidden control that has a ghost still answers, but never over a shown one.
        editor.select(PadLayout.Control.HOME);
        editor.toggleHidden();
        editor.draft().bounds(PadLayout.Control.HOME, surface.getWidth(), surface.getHeight(), density, box);
        tap(surface, box[0] + box[2] / 2, box[1] + box[3] / 2);
        assertEquals(PadLayout.Control.HOME, editor.selected());

        editor.addWasd();
        editor.select(null);
        tap(surface, x, y);
        assertEquals("Added, it is picked like any control", PadLayout.Control.WASD, editor.selected());
        controller.finishEditing(false);
    }

    /**
     * The picked control's corner handles size its width and height independently, the opposite corner staying put,
     * and the editor's Done keeps the shape.
     */
    @Test public void aCornerHandleSizesWidthAndHeightIndependently() {
        ModernController controller = controller(GAME);
        controller.startEditing();
        layout();
        PadEditor editor = (PadEditor) frame.getChildAt(frame.getChildCount() - 1);
        View surface = editor.getChildAt(0);
        float density = frame.getContext().getResources().getDisplayMetrics().density;
        float outline = PadPainter.OUTLINE_DP * density;
        editor.select(PadLayout.Control.FACE);
        float[] before = editor.draft().bounds(PadLayout.Control.FACE, surface.getWidth(), surface.getHeight(), density,
                new float[4]);

        // The top-left handle, dragged 200 px to the left: wider, as tall as before, the bottom-right corner kept.
        drag(surface, before[0] - outline, before[1] - outline, before[0] - outline - 200, before[1] - outline);
        assertEquals("The pick stays", PadLayout.Control.FACE, editor.selected());
        float[] wide = editor.draft().bounds(PadLayout.Control.FACE, surface.getWidth(), surface.getHeight(), density,
                new float[4]);
        assertEquals("The right edge stays", before[0] + before[2], wide[0] + wide[2], .5f);
        assertEquals("The bottom edge stays", before[1] + before[3], wide[1] + wide[3], .5f);
        assertEquals("200 px wider, on the 8 dp grid", before[2] + 200, wide[2], PadEditor.GRID_DP * density / 2 + .5f);
        assertEquals("As tall, give or take the grid", before[3], wide[3], PadEditor.GRID_DP * density / 2 + .5f);
        PadLayout.Spec spec = editor.draft().spec(PadLayout.Control.FACE);
        assertTrue(spec.widthScale + " x " + spec.heightScale, spec.widthScale > spec.heightScale + .5f);

        // The same handle dragged down: only the height changes.
        float widthScale = spec.widthScale;
        drag(surface, wide[0] - outline, wide[1] - outline, wide[0] - outline, wide[1] - outline + 100);
        float[] low = editor.draft().bounds(PadLayout.Control.FACE, surface.getWidth(), surface.getHeight(), density,
                new float[4]);
        assertEquals(widthScale, spec.widthScale, 1e-4);
        assertEquals(wide[3] - 100, low[3], PadEditor.GRID_DP * density / 2 + .5f);
        assertEquals("The bottom edge still stays", before[1] + before[3], low[1] + low[3], .5f);

        // A handle cannot shrink a control below a full touch, nor drag it past the surface.
        drag(surface, low[0] - outline, low[1] - outline, low[0] + 2000, low[1] + 2000);
        float[] least = editor.draft().bounds(PadLayout.Control.FACE, surface.getWidth(), surface.getHeight(), density,
                new float[4]);
        assertTrue(least[2] >= PadLayout.MIN_TOUCH_DP * density - .5f && least[3] >= PadLayout.MIN_TOUCH_DP * density - .5f);
        drag(surface, least[0] - outline, least[1] - outline, -500, -500);
        float[] most = editor.draft().bounds(PadLayout.Control.FACE, surface.getWidth(), surface.getHeight(), density,
                new float[4]);
        assertEquals("Up to the surface's corner", 0, most[0], .5f);
        assertEquals(0, most[1], .5f);
        assertEquals(before[0] + before[2], most[0] + most[2], .5f);

        PadLayout.Spec kept = new PadLayout().spec(PadLayout.Control.FACE);
        kept.copyFrom(spec);
        controller.finishEditing(true);
        PadLayout.Spec saved = controller(GAME).layout().spec(PadLayout.Control.FACE);
        assertEquals(kept.widthScale, saved.widthScale, 1e-3);
        assertEquals(kept.heightScale, saved.heightScale, 1e-3);
    }

    /** A controller whose saved layout has the face cluster or the D-pad split into its members. */
    private ModernController splitController(PadLayout.Control cluster) {
        ModernController first = controller(GAME);
        PadLayout draft = first.layout().copy();
        draft.split(cluster, frame.getWidth(), frame.getHeight(), frame.getResources().getDisplayMetrics().density);
        first.save(draft, false);
        first.destroy();
        return controller(GAME);
    }

    /** A point of the frame in {@code view}'s own coordinates, as a finger on it reports. */
    private static float[] in(View view, float frameX, float frameY) {
        return new float[] {frameX - view.getLeft(), frameY - view.getTop()};
    }

    /**
     * Split, A, B, X and Y are controls of their own, and a finger slides across them as it does on the cluster: from
     * A to B it lets go of A and presses B, though the finger still belongs to A's view, and where two boxes overlap it
     * holds both. Each shows itself pressed while held.
     */
    @Test public void aSplitFaceButtonSlidesToTheNextAndAnOverlapHoldsBoth() {
        ModernController controller = splitController(PadLayout.Control.FACE);
        assertEquals(View.GONE, controller.view(PadLayout.Control.FACE).getVisibility());
        for (PadLayout.Control member : PadLayout.members(PadLayout.Control.FACE)) {
            assertEquals(member.name(), View.VISIBLE, controller.view(member).getVisibility());
        }
        PadControlView a = controller.view(PadLayout.Control.A), b = controller.view(PadLayout.Control.B);
        a.down(0, a.getWidth() / 2f, a.getHeight() / 2f, 100);
        assertEquals(ControllerPacket.A_FLAG, last()[0]);
        assertTrue(a.state().pressed);

        float[] onB = in(a, b.getLeft() + b.getWidth() / 2f, b.getTop() + b.getHeight() / 2f);
        a.move(0, onB[0], onB[1]);
        assertEquals("Slid onto B", ControllerPacket.B_FLAG, last()[0]);
        assertFalse(a.state().pressed);
        assertTrue(b.state().pressed);

        int overlapLeft = Math.max(a.getLeft(), b.getLeft()), overlapTop = Math.max(a.getTop(), b.getTop());
        assertTrue("A's and B's boxes overlap", overlapLeft < Math.min(a.getRight(), b.getRight()) &&
                overlapTop < Math.min(a.getBottom(), b.getBottom()));
        float[] onBoth = in(a, overlapLeft + 1, overlapTop + 1);
        a.move(0, onBoth[0], onBoth[1]);
        assertEquals("Where they overlap, both", ControllerPacket.A_FLAG | ControllerPacket.B_FLAG, last()[0]);

        // A second finger on Y holds Y as well; the first lifting lets go of only its own.
        PadControlView y = controller.view(PadLayout.Control.Y);
        y.down(1, y.getWidth() / 2f, y.getHeight() / 2f, 150);
        assertEquals(ControllerPacket.A_FLAG | ControllerPacket.B_FLAG | ControllerPacket.Y_FLAG, last()[0]);
        a.up(0, 300);
        assertEquals(ControllerPacket.Y_FLAG, last()[0]);
        assertFalse(a.state().pressed || b.state().pressed);
        // Slid off every member, nothing.
        float[] away = in(y, y.getLeft() - 300, y.getTop() - 300);
        y.move(1, away[0], away[1]);
        assertEquals(0, last()[0]);
        y.up(1, 400);

        // A member a finger slid onto lets go when it hides, and a hidden member takes no input.
        a.down(2, a.getWidth() / 2f, a.getHeight() / 2f, 500);
        a.move(2, onB[0], onB[1]);
        assertEquals(ControllerPacket.B_FLAG, last()[0]);
        controller.layout().spec(PadLayout.Control.B).hidden = true;
        controller.place();
        assertEquals(View.GONE, b.getVisibility());
        assertEquals(0, last()[0]);
        a.move(2, onB[0], onB[1]);
        assertEquals(0, last()[0]);
        a.up(2, 600);
        controller.destroy();
    }

    /** Split, the D-pad's arms are controls of their own; where two neighbours overlap a finger holds the diagonal. */
    @Test public void aSplitDpadHoldsADiagonalWhereTwoArmsOverlap() {
        ModernController controller = splitController(PadLayout.Control.DPAD);
        PadControlView up = controller.view(PadLayout.Control.DPAD_UP), right = controller.view(PadLayout.Control.DPAD_RIGHT);
        up.down(0, up.getWidth() / 2f, up.getHeight() / 2f, 100);
        assertEquals(ControllerPacket.UP_FLAG, last()[0]);
        int overlapLeft = Math.max(up.getLeft(), right.getLeft()), overlapTop = Math.max(up.getTop(), right.getTop());
        assertTrue("Up's and right's boxes overlap", overlapLeft < Math.min(up.getRight(), right.getRight()) &&
                overlapTop < Math.min(up.getBottom(), right.getBottom()));
        float[] corner = in(up, overlapLeft + 1, overlapTop + 1);
        up.move(0, corner[0], corner[1]);
        assertEquals("The diagonal", ControllerPacket.UP_FLAG | ControllerPacket.RIGHT_FLAG, last()[0]);
        float[] onRight = in(up, right.getLeft() + right.getWidth() - 4, right.getTop() + right.getHeight() / 2f);
        up.move(0, onRight[0], onRight[1]);
        assertEquals(ControllerPacket.RIGHT_FLAG, last()[0]);
        // The middle, between the four arms, holds nothing.
        float density = frame.getResources().getDisplayMetrics().density;
        float[] pad = controller.layout().bounds(PadLayout.Control.DPAD, frame.getWidth(), frame.getHeight(), density,
                new float[4]);
        float[] middle = in(up, pad[0] + pad[2] / 2, pad[1] + pad[3] / 2);
        up.move(0, middle[0], middle[1]);
        assertEquals(0, last()[0]);
        up.up(0, 300);
        controller.destroy();
    }

    /**
     * The editor's Split turns the picked face cluster into its four buttons, picked and named in the glyph set, and
     * Join turns them back; Done keeps either, and the stream shows the cluster or its members, never both.
     */
    @Test public void theEditorSplitsAClusterAndJoinsItAgain() {
        ModernController controller = controller(GAME);
        controller.startEditing();
        settle();
        PadEditor editor = (PadEditor) frame.getChildAt(frame.getChildCount() - 1);
        View split = editor.findViewById(com.limelight.R.id.pad_editor_split);
        View join = editor.findViewById(com.limelight.R.id.pad_editor_join);
        assertEquals(View.GONE, split.getVisibility());
        editor.select(PadLayout.Control.LB);
        assertEquals("Only the clusters split", View.GONE, split.getVisibility());
        editor.select(PadLayout.Control.FACE);
        assertEquals(View.VISIBLE, split.getVisibility());
        assertEquals(View.GONE, join.getVisibility());
        assertTrue(split.performClick());
        assertTrue(editor.draft().isSplit(PadLayout.Control.FACE));
        assertEquals("The first member is picked", PadLayout.Control.A, editor.selected());
        assertEquals(View.GONE, split.getVisibility());
        assertEquals(View.VISIBLE, join.getVisibility());
        android.widget.TextView name = editor.findViewById(com.limelight.R.id.pad_editor_selection);
        assertEquals("A button", name.getText().toString());
        editor.draft().setGlyph(PadLayout.Glyph.PLAYSTATION);
        editor.select(PadLayout.Control.B);
        assertEquals("◯ button", name.getText().toString());
        editor.draft().setGlyph(PadLayout.Glyph.XBOX);
        controller.finishEditing(true);
        layout();
        assertEquals(View.GONE, controller.view(PadLayout.Control.FACE).getVisibility());
        assertEquals(View.VISIBLE, controller.view(PadLayout.Control.X).getVisibility());

        controller.startEditing();
        settle();
        editor = (PadEditor) frame.getChildAt(frame.getChildCount() - 1);
        editor.select(PadLayout.Control.Y);
        join = editor.findViewById(com.limelight.R.id.pad_editor_join);
        assertTrue(join.performClick());
        assertEquals(PadLayout.Control.FACE, editor.selected());
        assertFalse(editor.draft().isSplit(PadLayout.Control.FACE));
        controller.finishEditing(true);
        layout();
        assertEquals(View.VISIBLE, controller.view(PadLayout.Control.FACE).getVisibility());
        for (PadLayout.Control member : PadLayout.members(PadLayout.Control.FACE)) {
            assertEquals(member.name(), View.GONE, controller.view(member).getVisibility());
        }
        controller.destroy();
    }

    /** Lays the frame out and lets the editor's posted panel updates run, as the next frames would. */
    private void settle() {
        for (int pass = 0; pass < 3; pass++) {
            ShadowLooper.idleMainLooper();
            layout();
        }
    }

    /**
     * The editor's panel moves by its grip anywhere on the screen, then no longer moves out of a picked control's way
     * by itself, and the editor reopens with it where it was left.
     */
    @Test public void theEditorPanelMovesByItsGripAndReopensWhereItWasLeft() {
        ModernController controller = controller(GAME);
        controller.startEditing();
        settle();
        PadEditor editor = (PadEditor) frame.getChildAt(frame.getChildCount() - 1);
        View panel = editor.panel();
        View grip = editor.findViewById(com.limelight.R.id.pad_editor_grip);
        assertEquals(frame.getContext().getString(com.limelight.R.string.pad_edit_move_panel), grip.getContentDescription());
        float x = panel.getX(), y = panel.getY();
        assertTrue("The panel laid out in the middle", panel.getWidth() > 0 && x >= 300);

        drag(grip, 100, 10, 100 - 300, 10 + 20);
        assertEquals(x - 300, panel.getX(), 1);
        assertEquals(y + 20, panel.getY(), 1);
        settle();
        assertEquals("Laid out again, it stays", x - 300, panel.getX(), 1);
        assertEquals(y + 20, panel.getY(), 1);
        // The left stick sits under it now; picking it leaves the placed panel alone.
        editor.select(PadLayout.Control.LEFT_STICK);
        settle();
        assertEquals(x - 300, panel.getX(), 1);
        assertEquals(y + 20, panel.getY(), 1);

        // Dragged far past an edge, it stops there, whole.
        drag(grip, 100, 10, 100 - 5000, 10 - 5000);
        assertEquals(0, panel.getX(), 1);
        assertEquals(0, panel.getY(), 1);
        drag(grip, 100, 10, 100 + 300, 10 + 40);
        settle();
        float leftX = panel.getX(), leftY = panel.getY();
        assertEquals(300, leftX, 1);
        assertEquals(40, leftY, 1);

        // Cancel or Done, the editor opens again with the panel where it was left.
        controller.finishEditing(false);
        controller.startEditing();
        settle();
        PadEditor again = (PadEditor) frame.getChildAt(frame.getChildCount() - 1);
        assertNotSame(editor, again);
        assertEquals(leftX, again.panel().getX(), 1);
        assertEquals(leftY, again.panel().getY(), 1);
        controller.finishEditing(true);
    }

    /** A remembered place that no longer fits (a larger screen, a fold) shows the panel whole at the nearest edge. */
    @Test public void aRememberedPanelPlaceOffTheScreenKeepsThePanelOnIt() {
        RuntimeEnvironment.getApplication().getSharedPreferences(ModernController.PREFS, Context.MODE_PRIVATE).edit()
                .putString(PadLayoutKeys.panelKey(true), "5000,-300").commit();
        ModernController controller = controller(GAME);
        controller.startEditing();
        settle();
        View panel = ((PadEditor) frame.getChildAt(frame.getChildCount() - 1)).panel();
        assertTrue(panel.getWidth() > 0);
        assertEquals(frame.getWidth() - panel.getWidth(), panel.getX(), 1);
        assertEquals(0, panel.getY(), 1);
        assertTrue(panel.getY() + panel.getHeight() <= frame.getHeight());
        controller.finishEditing(false);
    }

    private static void drag(View view, float fromX, float fromY, float toX, float toY) {
        long now = android.os.SystemClock.uptimeMillis();
        MotionEvent down = MotionEvent.obtain(now, now, MotionEvent.ACTION_DOWN, fromX, fromY, 0);
        view.dispatchTouchEvent(down);
        down.recycle();
        for (int step = 1; step <= 4; step++) {
            float t = step / 4f;
            MotionEvent move = MotionEvent.obtain(now, now + 16 * step, MotionEvent.ACTION_MOVE,
                    fromX + (toX - fromX) * t, fromY + (toY - fromY) * t, 0);
            view.dispatchTouchEvent(move);
            move.recycle();
        }
        MotionEvent up = MotionEvent.obtain(now, now + 100, MotionEvent.ACTION_UP, toX, toY, 0);
        view.dispatchTouchEvent(up);
        up.recycle();
    }

    private static void tap(View view, float x, float y) {
        long now = android.os.SystemClock.uptimeMillis();
        MotionEvent down = MotionEvent.obtain(now, now, MotionEvent.ACTION_DOWN, x, y, 0);
        view.dispatchTouchEvent(down);
        down.recycle();
        MotionEvent up = MotionEvent.obtain(now, now + 40, MotionEvent.ACTION_UP, x, y, 0);
        view.dispatchTouchEvent(up);
        up.recycle();
    }

    @Test public void theWasdStickHiddenWhileHeldLetsGo() {
        ModernController controller = wasdController();
        PadControlView stick = controller.view(PadLayout.Control.WASD);
        push(stick, true, 0, 1);
        controller.layout().spec(PadLayout.Control.WASD).hidden = true;
        controller.place();
        assertEquals(View.GONE, stick.getVisibility());
        assertEveryKeyLetGo("Hidden");
    }

    @Test public void aFixedStickPushesFromItsDrawnCenter() {
        prefs.oscFloatingSticks = false;
        PadControlView stick = controller(GAME).view(PadLayout.Control.RIGHT_STICK);
        float size = Math.min(stick.getWidth(), stick.getHeight());
        stick.down(0, stick.getWidth() / 2f, stick.getHeight() / 2f - PadPainter.travel(size), 100);
        assertEquals(0, last()[3]);
        assertEquals(32766, last()[4]);
    }

    @Test @GraphicsMode(GraphicsMode.Mode.NATIVE)
    public void aFullyPushedKnobSitsOnTheRimAndDrawsPastTheControlWithoutTakingTouches() {
        prefs.oscFloatingSticks = false;
        ModernController controller = controller(GAME);
        PadControlView stick = controller.view(PadLayout.Control.LEFT_STICK);
        View knob = stick.knobLayer();
        assertNotNull(knob);
        assertNull(controller.view(PadLayout.Control.FACE).knobLayer());

        // The control keeps the layout's box, so saved layouts place and touch exactly as before.
        float density = frame.getResources().getDisplayMetrics().density;
        float[] box = new float[4];
        controller.layout().bounds(PadLayout.Control.LEFT_STICK, 1600, 720, density, box);
        assertEquals(Math.round(box[0]), stick.getLeft());
        assertEquals(Math.round(box[1]), stick.getTop());
        assertEquals(Math.round(box[2]), stick.getWidth());
        assertEquals(Math.round(box[3]), stick.getHeight());

        // The knob's layer is that box grown on every side by enough for a knob centred on the rim.
        int w = stick.getWidth(), h = stick.getHeight();
        float size = Math.min(w, h), stroke = 1.5f * density;
        float base = StickMath.baseRadius(size, stroke), radius = StickMath.knobRadius(size);
        int overhang = stick.knobOverhang(w, h);
        assertTrue("overhang " + overhang, overhang >= base + radius - size / 2);
        assertEquals(stick.getLeft() - overhang, knob.getLeft());
        assertEquals(stick.getTop() - overhang, knob.getTop());
        assertEquals(stick.getRight() + overhang, knob.getRight());
        assertEquals(stick.getBottom() + overhang, knob.getBottom());

        // Pushed fully right (and past the rim): the same full value as before, the knob's centre on the rim.
        stick.down(0, w / 2f, h / 2f, 100);
        stick.move(0, w / 2f + 3 * PadPainter.travel(size), h / 2f);
        assertEquals(32766, last()[1]);
        assertEquals(0, last()[2]);
        PadPainter.State state = stick.state();
        float kx = StickMath.knobX(w / 2f, h / 2f, w / 2f, h / 2f, state.stickX, state.stickY, base);
        assertEquals(w / 2f + base, kx, .01f);

        // Half the knob lies outside the control's box, and the layer paints it there.
        Bitmap pixels = Bitmap.createBitmap(knob.getWidth(), knob.getHeight(), Bitmap.Config.ARGB_8888);
        knob.draw(new Canvas(pixels));
        int outside = Math.round(overhang + w / 2f + base + radius * .6f);
        assertTrue(outside > overhang + w);
        assertTrue("knob not drawn past the control", Color.alpha(pixels.getPixel(outside, Math.round(overhang + h / 2f))) > 0);
        assertEquals(0, Color.alpha(pixels.getPixel(Math.round(overhang + w / 2f - base + radius), Math.round(overhang + h / 2f))));

        // A finger landing on the knob's overhang, outside the control, never reaches the stick.
        stick.up(0, 400);
        MotionEvent down = MotionEvent.obtain(1000, 1000, MotionEvent.ACTION_DOWN, knob.getLeft() + overhang / 2f,
                stick.getTop() + h / 2f, 0);
        assertFalse(knob.dispatchTouchEvent(MotionEvent.obtain(down)));
        frame.dispatchTouchEvent(down);
        assertFalse(stick.state().stickActive);
        down.recycle();
    }

    @Test public void tappingAStickThenTouchingItAgainHoldsItsClick() {
        PadControlView stick = controller(GAME).view(PadLayout.Control.LEFT_STICK);
        float c = stick.getWidth() / 2f;
        stick.down(0, c, c, 1000);
        stick.up(0, 1080);
        stick.down(0, c, c, 1200);
        assertEquals(ControllerPacket.LS_CLK_FLAG, last()[0] & ControllerPacket.LS_CLK_FLAG);
        stick.up(0, 1500);
        assertEquals(0, last()[0]);
        // A third touch is a new stick touch, not another click.
        stick.down(0, c, c, 1600);
        assertEquals(0, last()[0] & ControllerPacket.LS_CLK_FLAG);
    }

    @Test public void triggersPullFullyAndShouldersHoldTheirButton() {
        ModernController controller = controller(GAME);
        controller.view(PadLayout.Control.RT).down(0, 5, 5, 100);
        assertEquals(255, last()[6]);
        controller.view(PadLayout.Control.LB).down(1, 5, 5, 110);
        assertEquals(ControllerPacket.LB_FLAG, last()[0]);
        assertEquals(255, last()[6]);
        controller.releaseInput();
        assertArrayEquals(new int[7], last());
    }

    @Test public void resendsFollowAChangeLikeTheClassicController() {
        ModernController controller = controller(GAME);
        controller.view(PadLayout.Control.START).down(0, 5, 5, 100);
        int sent = reports.size();
        ShadowLooper.idleMainLooper(100, java.util.concurrent.TimeUnit.MILLISECONDS);
        assertEquals(sent + 3, reports.size());
        assertEquals(ControllerPacket.PLAY_FLAG, last()[0]);
    }

    @Test public void onlyStickClicksLeavesJustL3AndR3() {
        prefs.onlyL3R3 = true;
        ModernController controller = controller(GAME);
        assertEquals(2, frame.getChildCount());
        assertNotNull(controller.view(PadLayout.Control.L3));
        assertNull(controller.view(PadLayout.Control.FACE));
    }

    @Test public void homeShowsOnlyWithTheGuideButtonSetting() {
        prefs.showGuideButton = false;
        assertNull(controller(GAME).view(PadLayout.Control.HOME));
    }

    @Test public void theTouchpadClicksThePadsTouchpadAndShowsOnlyWithItsSetting() {
        PadControlView touchpad = controller(GAME).view(PadLayout.Control.TOUCHPAD);
        assertEquals(View.VISIBLE, touchpad.getVisibility());
        touchpad.down(0, touchpad.getWidth() / 2f, touchpad.getHeight() / 2f, 100);
        assertEquals(ControllerPacket.TOUCHPAD_FLAG, last()[0]);
        touchpad.up(0, 200);
        assertEquals(0, last()[0]);
        frame.removeAllViews();
        prefs.showTouchpadButton = false;
        assertNull("Off by default, so the default layout stays as it was", controller(GAME).view(PadLayout.Control.TOUCHPAD));
        assertFalse(new PreferenceConfiguration().showTouchpadButton);
    }

    @Test public void aLayoutSavedForOneGameStaysWithThatGame() {
        ModernController first = controller(GAME);
        PadLayout draft = first.layout().copy();
        draft.setScale(PadLayout.Control.FACE, 1.5f);
        first.save(draft, true);
        first.destroy();
        assertEquals(1.5f, controller(GAME).layout().spec(PadLayout.Control.FACE).scale(), 1e-3);
        assertTrue("Another game keeps the shared layout", controller("pc-1/app-8").layout().isDefault());
    }

    @Test public void savingForAllGamesDropsThisGamesOwnLayout() {
        ModernController controller = controller(GAME);
        PadLayout own = controller.layout().copy();
        own.setScale(PadLayout.Control.FACE, 1.5f);
        controller.save(own, true);
        PadLayout shared = controller.layout().copy();
        shared.setScale(PadLayout.Control.FACE, .7f);
        controller.save(shared, false);
        controller.destroy();
        assertEquals(.7f, controller(GAME).layout().spec(PadLayout.Control.FACE).scale(), 1e-3);
        assertEquals(.7f, controller("pc-1/app-8").layout().spec(PadLayout.Control.FACE).scale(), 1e-3);
        assertFalse(RuntimeEnvironment.getApplication().getSharedPreferences(ModernController.PREFS, Context.MODE_PRIVATE)
                .contains(PadLayoutKeys.key(true, GAME)));
    }

    @Test public void aStreamWithoutAGameSavesOnlyTheSharedLayout() {
        ModernController controller = controller(null);
        PadLayout draft = controller.layout().copy();
        draft.spec(PadLayout.Control.LB).hidden = true;
        controller.save(draft, true);
        assertTrue(controller("any").layout().spec(PadLayout.Control.LB).hidden);
    }

    @Test public void editingPausesInputAndCancelKeepsTheOldLayout() {
        ModernController controller = controller(GAME);
        PadControlView face = controller.view(PadLayout.Control.FACE);
        face.down(0, face.getWidth() / 2f, face.getHeight() * .78f, 100);
        controller.startEditing();
        assertTrue(controller.isEditing());
        assertEquals("Editing lets go of what was held", 0, last()[0]);
        assertEquals(View.GONE, face.getVisibility());
        controller.finishEditing(false);
        assertFalse(controller.isEditing());
        assertEquals(View.VISIBLE, face.getVisibility());
        assertTrue(controller.layout().isDefault());
    }

    @Test public void theEditorsDoneKeepsTheDraft() {
        ModernController controller = controller(GAME);
        controller.startEditing();
        PadEditor editor = (PadEditor) frame.getChildAt(frame.getChildCount() - 1);
        editor.select(PadLayout.Control.DPAD);
        editor.scaleBy(.1f);
        editor.toggleHidden();
        controller.finishEditing(true);
        assertEquals(1.1f, controller.layout().spec(PadLayout.Control.DPAD).scale(), 1e-3);
        assertEquals(View.GONE, controller.view(PadLayout.Control.DPAD).getVisibility());
        controller.destroy();
        assertTrue("A game without its own layout saves for every game, so another game sees it",
                controller("pc-1/app-8").layout().spec(PadLayout.Control.DPAD).hidden);
    }

    @Test public void theEditorWithNothingPickedChangesEveryControl() {
        ModernController controller = controller(GAME);
        controller.startEditing();
        PadEditor editor = (PadEditor) frame.getChildAt(frame.getChildCount() - 1);
        editor.setOpacity(50);
        editor.scaleBy(-.1f);
        for (PadLayout.Control control : PadLayout.Control.values()) {
            // The Steam Controller controls are off without their setting, so the editor leaves them alone.
            if (control.steam) continue;
            assertEquals(control.name(), 50, editor.draft().spec(control).opacity);
            assertEquals(control.name(), .9f, editor.draft().spec(control).scale(), 1e-3);
        }
        editor.toggleHidden();
        assertFalse("Hide needs a picked control", editor.draft().spec(PadLayout.Control.FACE).hidden);
    }

    @Test public void theTopRowLeavesRoomForTheMicrophoneButton() {
        ModernController controller = controller(GAME);
        float bottom = controller.topRowBottom();
        float density = frame.getResources().getDisplayMetrics().density;
        // The Touchpad sits below View, Home and Menu, so the microphone goes below it.
        float[] box = controller.layout().bounds(PadLayout.Control.TOUCHPAD, 1600, 720, density, new float[4]);
        assertEquals(box[1] + box[3], bottom, 1);
        float[] menu = controller.layout().bounds(PadLayout.Control.START, 1600, 720, density, new float[4]);
        assertTrue(menu[1] + menu[3] < box[1]);
        // The microphone's default place clears the Touchpad pill, in landscape and in portrait.
        float size = com.limelight.ui.MicButton.SIZE_DP * density;
        float[] mic = com.limelight.ui.MicButton.Placement.center(Float.NaN, Float.NaN, 1600, 720, size,
                12 * density, bottom);
        assertTrue(mic[1] - size / 2 >= box[1] + box[3]);
        float[] portrait = controller.layout().bounds(PadLayout.Control.TOUCHPAD, 720, 1600, density, new float[4]);
        assertTrue("Counted as the top row", portrait[1] <= 1600 / 3f);
        mic = com.limelight.ui.MicButton.Placement.center(Float.NaN, Float.NaN, 720, 1600, size,
                12 * density, portrait[1] + portrait[3]);
        assertTrue(mic[1] - size / 2 >= portrait[1] + portrait[3]);
        controller.hide();
        assertTrue(Float.isNaN(controller.topRowBottom()));
    }

    @Test public void closingTheEditorTellsTheMicrophoneButtonToMove() {
        ModernController controller = controller(GAME);
        int[] runs = {0};
        controller.setOnLayoutChanged(() -> runs[0]++);
        controller.startEditing();
        controller.finishEditing(false);
        controller.startEditing();
        controller.finishEditing(true);
        assertEquals("Cancel and Done both report", 2, runs[0]);
        controller.finishEditing(true);
        assertEquals("Nothing to close, nothing reported", 2, runs[0]);
    }

    /** A portrait phone: Hide and Reset turn into 48 dp icons so Cancel and Done stay whole on the row. */
    @Test @Config(qualifiers = "w360dp-h800dp-port-xhdpi")
    public void aNarrowEditorKeepsEveryButtonWholeAndTouchable() {
        frame.getLayoutParams().width = 720;
        frame.getLayoutParams().height = 1600;
        ModernController controller = new ModernController(frame.getContext(), frame, sink, prefs, GAME);
        controller.refreshLayout();
        controller.startEditing();
        for (int pass = 0; pass < 2; pass++) {
            ShadowLooper.idleMainLooper();
            frame.measure(View.MeasureSpec.makeMeasureSpec(720, View.MeasureSpec.EXACTLY),
                    View.MeasureSpec.makeMeasureSpec(1600, View.MeasureSpec.EXACTLY));
            frame.layout(0, 0, 720, 1600);
        }
        PadEditor editor = (PadEditor) frame.getChildAt(frame.getChildCount() - 1);
        editor.select(PadLayout.Control.DPAD);
        ShadowLooper.idleMainLooper();
        float density = frame.getResources().getDisplayMetrics().density;
        View row = (View) editor.findViewById(com.limelight.R.id.pad_editor_done).getParent();
        assertTrue("The editor laid out", row.getWidth() > 0);
        int[] ids = {com.limelight.R.id.pad_editor_hide, com.limelight.R.id.pad_editor_reset,
                com.limelight.R.id.pad_editor_cancel, com.limelight.R.id.pad_editor_done};
        for (int id : ids) {
            com.google.android.material.button.MaterialButton button = editor.findViewById(id);
            String name = frame.getResources().getResourceEntryName(id);
            assertTrue(name + " is " + button.getWidth() / density + " dp wide", button.getWidth() >= 48 * density - 1);
            assertTrue(name + " is " + button.getHeight() / density + " dp tall", button.getHeight() >= 48 * density - 1);
            assertTrue(name + " ends inside the panel", button.getRight() <= row.getWidth());
        }
        com.google.android.material.button.MaterialButton hide = editor.findViewById(com.limelight.R.id.pad_editor_hide);
        assertEquals("Icon alone", 0, hide.getText().length());
        assertEquals("Still named for TalkBack", frame.getContext().getString(com.limelight.R.string.pad_edit_hide),
                hide.getContentDescription());
        controller.destroy();
    }

    /** A layout edited in portrait is stored for portrait; landscape keeps its own. */
    @Test @Config(qualifiers = "w360dp-h800dp-port-xhdpi")
    public void portraitAndLandscapeKeepTheirOwnLayouts() {
        frame.getLayoutParams().width = 720;
        frame.getLayoutParams().height = 1600;
        ModernController controller = new ModernController(frame.getContext(), frame, sink, prefs, GAME);
        controller.refreshLayout();
        controller.show();
        frame.measure(View.MeasureSpec.makeMeasureSpec(720, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(1600, View.MeasureSpec.EXACTLY));
        frame.layout(0, 0, 720, 1600);
        PadLayout draft = controller.layout().copy();
        draft.setScale(PadLayout.Control.FACE, 1.4f);
        controller.save(draft, false);
        android.content.SharedPreferences store = RuntimeEnvironment.getApplication()
                .getSharedPreferences(ModernController.PREFS, Context.MODE_PRIVATE);
        assertTrue("Saved for portrait", store.contains(PadLayoutKeys.key(false, null)));
        assertFalse("Not for landscape", store.contains(PadLayoutKeys.key(true, null)));
        controller.destroy();
    }

    /** A layout saved by the version before separate orientations loads in both. */
    @Test public void anOlderSharedLayoutStillLoads() {
        RuntimeEnvironment.getApplication().getSharedPreferences(ModernController.PREFS, Context.MODE_PRIVATE)
                .edit().putString(PadLayoutKeys.legacyKey(null), "v1|FACE,0.7083,0.5556,1.300,100,0").commit();
        ModernController controller = controller(GAME);
        assertEquals(.7083f, controller.layout().spec(PadLayout.Control.FACE).cx, 1e-4);
        assertEquals(1.3f, controller.layout().spec(PadLayout.Control.FACE).scale(), 1e-3);
    }

    /** The first modern stream after a classic one keeps the classic controls' places. */
    @Test public void aClassicLayoutIsCarriedOverOnce() {
        RuntimeEnvironment.getApplication().getSharedPreferences(
                        com.limelight.binding.input.virtual_controller.VirtualControllerConfigurationLoader.OSC_PREFERENCE,
                        Context.MODE_PRIVATE)
                .edit().putString("13", "{\"LEFT\":1200,\"TOP\":400,\"WIDTH\":200,\"HEIGHT\":200}").commit();
        // A stream that never saw the modern controller: the one-time carry-over is still due.
        RuntimeEnvironment.getApplication().getSharedPreferences(ModernController.PREFS, Context.MODE_PRIVATE)
                .edit().remove("classic-migrated").commit();
        ModernController controller = controller(GAME);
        PadLayout.Spec stick = controller.layout().spec(PadLayout.Control.RIGHT_STICK);
        assertTrue("The classic right stick's place is carried over", stick.placed());
        assertEquals(1300f / 1600f, stick.cx, .01f);
        assertTrue(RuntimeEnvironment.getApplication().getSharedPreferences(ModernController.PREFS, Context.MODE_PRIVATE)
                .contains(PadLayoutKeys.legacyKey(null)));
        controller.destroy();
        // The carried layout is used again, not re-migrated from whatever the classic controller holds later.
        assertTrue(controller(GAME).layout().spec(PadLayout.Control.RIGHT_STICK).placed());
    }

    @Test public void theEditorAddsThePointerTouchpadAndSetsItsMode() {
        ModernController controller = controller(GAME);
        assertEquals(View.GONE, controller.view(PadLayout.Control.POINTER_PAD).getVisibility());
        controller.startEditing();
        PadEditor editor = (PadEditor) frame.getChildAt(frame.getChildCount() - 1);
        View add = editor.findViewById(com.limelight.R.id.pad_editor_add_touchpad);
        assertEquals(View.VISIBLE, add.getVisibility());
        assertTrue(add.performClick());
        assertEquals(PadLayout.Control.POINTER_PAD, editor.selected());
        assertFalse(editor.draft().spec(PadLayout.Control.POINTER_PAD).hidden);
        View mode = editor.findViewById(com.limelight.R.id.pad_editor_pointer_mode);
        assertEquals(View.VISIBLE, mode.getVisibility());
        assertTrue(mode.performClick());
        assertEquals(PadLayout.PointerMode.CAMERA, editor.draft().pointerMode());
        com.google.android.material.button.MaterialButton modeButton =
                editor.findViewById(com.limelight.R.id.pad_editor_pointer_mode);
        assertEquals("The button names the mode now", "Camera", modeButton.getText().toString());
        assertEquals("and its description the mode pressing gives", "Mouse", modeButton.getContentDescription().toString());
        assertTrue(mode.performClick());
        assertEquals(PadLayout.PointerMode.MOUSE, editor.draft().pointerMode());
        controller.finishEditing(true);
        layout();
        assertEquals(View.VISIBLE, controller.view(PadLayout.Control.POINTER_PAD).getVisibility());
    }

    /** The pointer touchpad is built from a saved layout with its mode, and it sends through the pointer sink. */
    private ModernController pointerController(boolean camera) {
        ModernController first = controller(GAME);
        PadLayout draft = first.layout().copy();
        draft.spec(PadLayout.Control.POINTER_PAD).hidden = false;
        draft.setPointerMode(camera ? PadLayout.PointerMode.CAMERA : PadLayout.PointerMode.MOUSE);
        first.save(draft, false);
        first.destroy();
        ModernController controller = controller(GAME);
        controller.setPointerSink(pointerSink);
        return controller;
    }

    @Test public void aPointerTouchpadDragMovesTheMouseAndATapClicks() {
        prefs.trackpadAcceleration = true;
        pointerMoves.clear();
        pointerButtons.clear();
        PadControlView pad = pointerController(false).view(PadLayout.Control.POINTER_PAD);
        assertEquals(View.VISIBLE, pad.getVisibility());
        assertEquals("The pad says what it does", "MOUSE", pad.state().label);
        float x = pad.getWidth() / 2f, y = pad.getHeight() / 2f;

        pad.down(0, x, y, 1000);
        // A gesture starts at the slow end, so acceleration shows from the second move on.
        pad.move(0, x + 20, y, 1_010_000_000L);
        pad.move(0, x + 40, y, 1_020_000_000L);
        assertEquals("Both moves reach the pointer", 2, pointerMoves.size());
        int first = pointerMoves.get(0)[0], second = pointerMoves.get(1)[0];
        assertTrue("A faster move is accelerated: " + first + " then " + second, second > first);
        assertTrue("and passes the finger's own 20 px", second > 20);
        pad.up(0, 1200);
        assertTrue("A drag is not a click", pointerButtons.isEmpty());

        pad.down(0, x, y, 2000);
        pad.up(0, 2080);
        assertEquals(2, pointerButtons.size());
        assertEquals(Integer.valueOf(MouseButtonPacket.BUTTON_LEFT), pointerButtons.get(0));
        assertEquals(Integer.valueOf(-MouseButtonPacket.BUTTON_LEFT), pointerButtons.get(1));
    }

    @Test public void twoFingersOnThePointerTouchpadTapRightAndDragToScroll() {
        pointerButtons.clear();
        pointerScrolls.clear();
        PadControlView pad = pointerController(false).view(PadLayout.Control.POINTER_PAD);
        float x = pad.getWidth() / 2f, y = pad.getHeight() / 2f;

        pad.down(0, x, y, 1000);
        pad.down(1, x + 30, y, 1050);
        pad.up(1, 1100);
        pad.up(0, 1150);
        assertEquals("Two fingers tap the right button", 2, pointerButtons.size());
        assertEquals(Integer.valueOf(MouseButtonPacket.BUTTON_RIGHT), pointerButtons.get(0));

        pointerButtons.clear();
        pad.down(0, x, y, 2000);
        pad.down(1, x + 30, y, 2050);
        pad.move(1, x + 30, y + 80, 2_100_000_000L);
        pad.up(1, 2200);
        pad.up(0, 2250);
        assertFalse("Two fingers dragging scroll", pointerScrolls.isEmpty());
        assertTrue(pointerButtons.isEmpty());
    }

    @Test public void thePointerTouchpadAsACameraPushesTheRightStickAndLetsGo() {
        ModernController controller = pointerController(true);
        PadControlView pad = controller.view(PadLayout.Control.POINTER_PAD);
        assertEquals("The pad says it is the camera", "CAM", pad.state().label);
        float x = pad.getWidth() / 2f, y = pad.getHeight() / 2f;
        pad.down(0, x, y, 1000);
        // The camera starts at rest like the pointer: its speed needs an interval behind it.
        pad.move(0, x + 50, y, 1_005_000_000L);
        pad.move(0, x + 100, y, 1_010_000_000L);
        assertTrue("A fast drag turns right", last()[3] > 0);
        assertEquals(0, last()[4]);
        pad.release();
        assertEquals("Letting go centers the camera", 0, last()[3] | last()[4]);
    }

    /** Trackpad speed reaches the touchpad's Camera mode: the same drag turns further at 200 % than at 100 %. */
    @Test public void thePointerTouchpadAsACameraFollowsTrackpadSpeed() {
        int[] turned = new int[2];
        int[] speeds = {100, 200};
        for (int i = 0; i < speeds.length; i++) {
            prefs.trackpadSpeed = speeds[i];
            ModernController controller = pointerController(true);
            PadControlView pad = controller.view(PadLayout.Control.POINTER_PAD);
            float x = pad.getWidth() / 2f, y = pad.getHeight() / 2f;
            pad.down(0, x, y, 1000);
            pad.move(0, x + 4, y, 1_010_000_000L);
            pad.move(0, x + 8, y, 1_020_000_000L);
            turned[i] = last()[3];
            pad.release();
            controller.destroy();
        }
        assertTrue("A drag turns the camera", turned[0] > 0);
        assertTrue("and turns it further at 200 %: " + turned[0] + " vs " + turned[1], turned[1] > turned[0]);
    }

    /**
     * Opacity 0 is a player's choice of controls nobody sees: a press still plays but shows nothing, neither the
     * control, a stick's knob, the touchpad's finger nor the pressed amber. Above 0 a faint control still answers.
     */
    @Test public void atOpacityZeroAPressStaysInvisibleAndAboveItAFaintControlStillAnswers() {
        prefs.oscOpacity = 0;
        ModernController controller = pointerController(false);
        PadControlView button = controller.view(PadLayout.Control.LB);
        PadControlView face = controller.view(PadLayout.Control.FACE);
        PadControlView stick = controller.view(PadLayout.Control.LEFT_STICK);
        PadControlView pad = controller.view(PadLayout.Control.POINTER_PAD);
        for (PadControlView view : new PadControlView[] {button, face, stick, pad}) {
            assertEquals(view.control + " at rest", 0, view.getAlpha(), 0);
        }
        button.down(0, 5, 5, 100);
        face.down(1, face.getWidth() / 2f, face.getHeight() * .78f, 100);
        float size = Math.min(stick.getWidth(), stick.getHeight());
        stick.down(2, stick.getWidth() / 2f, stick.getHeight() / 2f, 100);
        stick.move(2, stick.getWidth() / 2f + PadPainter.travel(size), stick.getHeight() / 2f);
        pad.down(3, pad.getWidth() / 2f, pad.getHeight() / 2f, 100);
        pad.move(3, pad.getWidth() / 2f + 20, pad.getHeight() / 2f, 200_000_000L);
        assertEquals("Pressed, the buttons still play", ControllerPacket.LB_FLAG | ControllerPacket.A_FLAG,
                last()[0] & (ControllerPacket.LB_FLAG | ControllerPacket.A_FLAG));
        assertTrue("and so does the stick", last()[1] > 0);
        assertFalse("The touchpad follows its finger", Float.isNaN(pad.state().touchX));
        for (PadControlView view : new PadControlView[] {button, face, stick, pad}) {
            assertEquals(view.control + " pressed", 0, view.getAlpha(), 0);
        }
        assertEquals("The knob layer too", 0, stick.knobLayer().getAlpha(), 0);
        controller.releaseInput();

        prefs.oscOpacity = 20;
        controller.place();
        assertEquals(.2f * PadLayout.MAX_OPACITY / 100f, button.getAlpha(), 1e-4);
        button.down(0, 5, 5, 100);
        stick.down(2, stick.getWidth() / 2f, stick.getHeight() / 2f, 100);
        assertEquals("A press raises a faint control", PadControlView.PRESSED_ALPHA, button.getAlpha(), 1e-4);
        assertEquals(PadControlView.PRESSED_ALPHA, stick.knobLayer().getAlpha(), 1e-4);
        button.up(0, 200);
        assertEquals("and letting go lowers it again", .2f, button.getAlpha(), 1e-4);
        controller.destroy();
    }

    /** H3: this game's own layout, in the old single key, keeps winning after the player saves one for all games. */
    @Test public void aGamesOwnLayoutWinsOverASharedOneSavedLater() {
        android.content.SharedPreferences store = RuntimeEnvironment.getApplication()
                .getSharedPreferences(ModernController.PREFS, Context.MODE_PRIVATE);
        store.edit()
                .putString(PadLayoutKeys.legacyKey(GAME), "v1|FACE,0.7083,0.5556,1.300,100,0")
                .putString(PadLayoutKeys.key(true, null), "v1|FACE,0.3000,0.4000,0.700,100,0")
                .commit();
        ModernController controller = controller(GAME);
        assertEquals("The game's own layout wins", 1.3f,
                controller.layout().spec(PadLayout.Control.FACE).scale(), 1e-3);
        assertEquals(.7083f, controller.layout().spec(PadLayout.Control.FACE).cx, 1e-4);
        assertEquals("A game without its own layout gets the shared one", .7f,
                controller("pc-1/app-8").layout().spec(PadLayout.Control.FACE).scale(), 1e-3);
    }

    /** M5: saving "for all games" in portrait must not delete the game's landscape layout. */
    @Test @Config(qualifiers = "w360dp-h800dp-port-xhdpi")
    public void savingForAllGamesKeepsTheOtherOrientationsPerGameLayout() {
        android.content.SharedPreferences store = RuntimeEnvironment.getApplication()
                .getSharedPreferences(ModernController.PREFS, Context.MODE_PRIVATE);
        store.edit().putString(PadLayoutKeys.key(true, GAME), "v1|FACE,0.7083,0.5556,1.300,100,0").commit();
        frame.getLayoutParams().width = 720;
        frame.getLayoutParams().height = 1600;
        ModernController controller = new ModernController(frame.getContext(), frame, sink, prefs, GAME);
        controller.refreshLayout();
        frame.measure(View.MeasureSpec.makeMeasureSpec(720, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(1600, View.MeasureSpec.EXACTLY));
        frame.layout(0, 0, 720, 1600);
        PadLayout draft = controller.layout().copy();
        draft.setScale(PadLayout.Control.FACE, .7f);
        controller.save(draft, false);
        assertTrue("Saved for all games in portrait", store.contains(PadLayoutKeys.key(false, null)));
        assertTrue("The game's landscape layout stays", store.contains(PadLayoutKeys.key(true, GAME)));
        assertFalse("This orientation's own per-game layout goes", store.contains(PadLayoutKeys.key(false, GAME)));
        controller.destroy();
    }

    /** M6: a scroll is never a right click, even when the first finger lifts before the scroll starts. */
    @Test public void aScrollNeverRightClicksAfterTheFirstFingerLifts() {
        pointerButtons.clear();
        pointerScrolls.clear();
        PadControlView pad = pointerController(false).view(PadLayout.Control.POINTER_PAD);
        float x = pad.getWidth() / 2f, y = pad.getHeight() / 2f;

        pad.down(0, x, y, 1000);
        pad.down(1, x + 30, y, 1020);
        pad.up(0, 1080);
        pad.move(1, x + 30, y + 80, 1_100_000_000L);
        pad.up(1, 1200);
        assertFalse("The second finger scrolls", pointerScrolls.isEmpty());
        assertTrue("and the scroll does not right-click: " + pointerButtons, pointerButtons.isEmpty());
    }

    /** M6: a finger that stays after its partner lifts is not a two-finger tap. */
    @Test public void aLingeringFingerIsNotATwoFingerTap() {
        pointerButtons.clear();
        PadControlView pad = pointerController(false).view(PadLayout.Control.POINTER_PAD);
        float x = pad.getWidth() / 2f, y = pad.getHeight() / 2f;

        pad.down(0, x, y, 1000);
        pad.down(1, x + 30, y, 1010);
        pad.up(0, 1090);
        pad.up(1, 1400);
        assertTrue("A late lift is not a tap: " + pointerButtons, pointerButtons.isEmpty());
    }

    /** M3: other controls report once per event; only the pointer touchpad needs every historical sample. */
    @Test public void historicalSamplesChangeOnlyThePointerTouchpad() {
        ModernController controller = pointerController(false);
        PadControlView pad = controller.view(PadLayout.Control.POINTER_PAD);
        float x = pad.getWidth() / 2f, y = pad.getHeight() / 2f;
        pad.down(0, x, y, 1000);
        pointerMoves.clear();
        MotionEvent drag = MotionEvent.obtain(1000, 1010, MotionEvent.ACTION_MOVE, x + 60, y, 0);
        drag.addBatch(1005, x + 30, y, 1f, 1f, 0);
        assertEquals("The event carries a historical sample", 1, drag.getHistorySize());
        pad.onTouchEvent(drag);
        drag.recycle();
        assertTrue("The pointer pad uses each sample's own time: " + pointerMoves.size(), pointerMoves.size() >= 2);

        PadControlView stick = controller.view(PadLayout.Control.LEFT_STICK);
        float size = Math.min(stick.getWidth(), stick.getHeight()), travel = PadPainter.travel(size);
        float cx = stick.getWidth() / 2f, cy = stick.getHeight() / 2f;
        long now = android.os.SystemClock.uptimeMillis();
        MotionEvent down = MotionEvent.obtain(now, now, MotionEvent.ACTION_DOWN, cx, cy, 0);
        stick.onTouchEvent(down);
        down.recycle();
        int before = reports.size();
        MotionEvent move = MotionEvent.obtain(now, now + 10, MotionEvent.ACTION_MOVE, cx + travel, cy, 0);
        move.addBatch(now + 5, cx + travel / 2, cy, 1f, 1f, 0);
        stick.onTouchEvent(move);
        move.recycle();
        assertEquals("A stick reports once for the event, as it always has", 1, reports.size() - before);
    }

    /** L4: showing the controls again tells the microphone button to move back below the top row. */
    @Test public void showingTheControlsAgainTellsTheMicrophoneButtonToMove() {
        ModernController controller = controller(GAME);
        int[] runs = {0};
        controller.setOnLayoutChanged(() -> runs[0]++);
        controller.hide();
        controller.show();
        assertEquals(1, runs[0]);
    }

    /** M3: the old single per-game layout is both orientations' fallback, so saving for all games keeps it for both. */
    @Test public void savingForAllGamesHandsTheGamesOldLayoutToTheOtherOrientation() {
        android.content.SharedPreferences store = RuntimeEnvironment.getApplication()
                .getSharedPreferences(ModernController.PREFS, Context.MODE_PRIVATE);
        String legacy = "v1|FACE,0.7083,0.5556,1.300,100,0";
        store.edit().putString(PadLayoutKeys.legacyKey(GAME), legacy).commit();
        ModernController controller = controller(GAME);
        assertEquals("The game's old layout is its own", 1.3f, controller.layout().spec(PadLayout.Control.FACE).scale(), 1e-3);
        PadLayout shared = controller.layout().copy();
        shared.setScale(PadLayout.Control.FACE, .7f);
        controller.save(shared, false);
        controller.destroy();
        assertFalse("The old shared key goes", store.contains(PadLayoutKeys.legacyKey(GAME)));
        assertEquals("The other orientation keeps the game's own layout", legacy,
                store.getString(PadLayoutKeys.key(false, GAME), null));
        // Portrait finds that layout instead of the shared one saved in landscape.
        RuntimeEnvironment.setQualifiers("w360dp-h800dp-port-xhdpi");
        frame.getLayoutParams().width = 720;
        frame.getLayoutParams().height = 1600;
        ModernController portrait = new ModernController(frame.getContext(), frame, sink, prefs, GAME);
        assertEquals(1.3f, portrait.layout().spec(PadLayout.Control.FACE).scale(), 1e-3);
        portrait.destroy();
    }

    /**
     * L2: the first modern start after a classic layout is carried over maps it with landscape dimensions, whatever
     * orientation it starts in, because the shared layout it becomes is used in both.
     */
    @Test @Config(qualifiers = "w360dp-h800dp-port-xhdpi")
    public void aClassicLayoutCarriedOverInPortraitUsesLandscapeDimensions() {
        RuntimeEnvironment.getApplication().getSharedPreferences(
                        com.limelight.binding.input.virtual_controller.VirtualControllerConfigurationLoader.OSC_PREFERENCE,
                        Context.MODE_PRIVATE)
                .edit().putString("13", "{\"LEFT\":1200,\"TOP\":400,\"WIDTH\":200,\"HEIGHT\":200}").commit();
        RuntimeEnvironment.getApplication().getSharedPreferences(ModernController.PREFS, Context.MODE_PRIVATE)
                .edit().remove("classic-migrated").commit();
        ModernController controller = controller(GAME);
        PadLayout.Spec stick = controller.layout().spec(PadLayout.Control.RIGHT_STICK);
        // This display is 720x1600; the classic stick sat on its 1600x720 landscape side at (1300, 500).
        assertEquals(1300f / 1600f, stick.cx, .01f);
        assertEquals(500f / 720f, stick.cy, .01f);
        controller.destroy();
    }

    /** L1: Trackpad acceleration off is linear for the pointer touchpad, as it is for the screen's own touchpad. */
    @Test public void withTrackpadAccelerationOffThePointerTouchpadPassesTheFingerThrough() {
        prefs.trackpadAcceleration = false;
        pointerMoves.clear();
        PadControlView pad = pointerController(false).view(PadLayout.Control.POINTER_PAD);
        float x = pad.getWidth() / 2f, y = pad.getHeight() / 2f;
        pad.down(0, x, y, 1000);
        pad.move(0, x + 20, y, 1_010_000_000L);
        assertEquals("The finger's own 20 px, not the medium curve's 12", 20, pointerMoves.get(0)[0]);
        pad.release();
    }

    /**
     * M2: every sample is stamped with its own event time. A late frame (a dispatch delay) must not read as a slow
     * finger, so the speed comes from the 5 ms between the two samples. Robolectric's MotionEvent reports whole
     * milliseconds, so this runs below Android 14; the nanosecond times above it are the same times, wider.
     */
    @Test
    public void thePointerTouchpadsSpeedFollowsEventTimesThroughDispatchJitter() {
        PadControlView pad = pointerController(false).view(PadLayout.Control.POINTER_PAD);
        float x = pad.getWidth() / 2f, y = pad.getHeight() / 2f;
        pad.down(0, x, y, 1000);
        PadPointerMotion motion = ReflectionHelpers.getField(pad, "pointerMotion");
        MotionEvent drag = MotionEvent.obtain(1000, 1005, MotionEvent.ACTION_MOVE, x + 30, y, 0);
        drag.addBatch(1010, x + 60, y, 1f, 1f, 0);
        assertEquals("The event carries one historical sample", 1, drag.getHistorySize());
        pad.onTouchEvent(drag);
        drag.recycle();
        // The historical sample starts the speed; the current one adds 30 px over 5 ms: 15 dp over 5 ms at density 2.
        assertEquals(3000f, motion.speed(), 1f);
        pad.release();
    }
}
