package com.limelight.binding.input.pad;

import android.app.Application;
import android.content.Context;
import android.view.View;
import android.widget.FrameLayout;

import com.limelight.binding.input.HeldInputState;
import com.limelight.binding.input.SteamControls;
import com.limelight.nvstream.input.ControllerPacket;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.ui.UiNavigationTest;

import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowLooper;

import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.TimeUnit;

import static org.junit.Assert.*;

/**
 * nightjar#106: the modern on-screen controller offers the Steam Controller's trackpads, back buttons and grip touch
 * with their setting, sent as the classic controller sends them.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 30, application = Application.class, qualifiers = "w800dp-h360dp-land-xhdpi")
public class ModernSteamControlsTest {
    private static final PadLayout.Control[] STEAM = {PadLayout.Control.LEFT_PAD, PadLayout.Control.RIGHT_PAD,
            PadLayout.Control.L4, PadLayout.Control.L5, PadLayout.Control.R4, PadLayout.Control.R5,
            PadLayout.Control.LEFT_GRIP, PadLayout.Control.RIGHT_GRIP};
    private final List<Integer> buttons = new ArrayList<>();
    /** Trackpad touches as {type, pointer, x * 1000, y * 1000}. */
    private final List<int[]> touches = new ArrayList<>();
    /** Buttons and touches in the order they went out: "b<flags>" and "t<type>". */
    private final List<String> events = new ArrayList<>();
    private final HeldInputState.Sink sink = new HeldInputState.Sink() {
        @Override public void key(int key, boolean down) { fail("Gamepad only"); }
        @Override public void mouse(int button, boolean down) { fail("Gamepad only"); }
        @Override public void gamepad(int b, short lx, short ly, short rx, short ry, byte lt, byte rt) { buttons.add(b); events.add("b" + b); }
    };
    private FrameLayout frame;
    private PreferenceConfiguration prefs;

    @Before
    public void setUp() {
        Context app = RuntimeEnvironment.getApplication();
        ModernController.clearLayouts(app);
        UiNavigationTest.Screen screen = Robolectric.buildActivity(UiNavigationTest.Screen.class).setup().visible().get();
        frame = new FrameLayout(screen);
        screen.setContentView(frame, new FrameLayout.LayoutParams(1600, 720));
        prefs = new PreferenceConfiguration();
        prefs.oscSteamControls = true;
        // The controls take their sizes from the frame's.
        layout();
    }

    private void layout() {
        frame.measure(View.MeasureSpec.makeMeasureSpec(1600, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(720, View.MeasureSpec.EXACTLY));
        frame.layout(0, 0, 1600, 720);
    }

    private ModernController controller() {
        ModernController controller = new ModernController(frame.getContext(), frame, sink, prefs, null);
        controller.setTouchSink((type, pointer, x, y, pressure) ->
                { touches.add(new int[] {type, pointer, Math.round(x * 1000), Math.round(y * 1000)}); events.add("t" + type); });
        controller.refreshLayout();
        controller.show();
        layout();
        return controller;
    }

    private int last() {
        return buttons.get(buttons.size() - 1);
    }

    @Test public void theControlsShowOnlyWithTheirSetting() {
        ModernController controller = controller();
        for (PadLayout.Control control : STEAM) {
            assertTrue(control.steam);
            assertEquals(control.name(), View.VISIBLE, controller.view(control).getVisibility());
        }
        controller.destroy();

        prefs.oscSteamControls = false;
        controller = controller();
        for (PadLayout.Control control : STEAM) {
            assertTrue(control.name(), controller.forcedHidden(control));
            assertNull(control.name(), controller.view(control));
        }
        controller.destroy();

        // Like the classic controller's, they follow their own setting even with only L3 and R3 shown.
        prefs.oscSteamControls = true;
        prefs.onlyL3R3 = true;
        controller = controller();
        assertFalse(controller.forcedHidden(PadLayout.Control.LEFT_PAD));
        assertTrue(controller.forcedHidden(PadLayout.Control.FACE));
    }

    @Test public void theBackButtonsHoldTheirPaddleFlags() {
        ModernController controller = controller();
        int[][] expected = {{PadLayout.Control.L4.ordinal(), SteamControls.L4_FLAG},
                {PadLayout.Control.L5.ordinal(), SteamControls.L5_FLAG},
                {PadLayout.Control.R4.ordinal(), SteamControls.R4_FLAG},
                {PadLayout.Control.R5.ordinal(), SteamControls.R5_FLAG}};
        for (int[] pair : expected) {
            PadControlView view = controller.view(PadLayout.Control.values()[pair[0]]);
            view.down(0, 10, 10, 100);
            assertEquals(view.control.name(), pair[1], last());
            view.up(0, 150);
            assertEquals(0, last());
        }
    }

    @Test public void aTouchedStickHoldsItsTouchFlagUntilTheFingerLifts() {
        ModernController controller = controller();
        PadControlView left = controller.view(PadLayout.Control.LEFT_STICK);
        PadControlView right = controller.view(PadLayout.Control.RIGHT_STICK);
        left.down(0, left.getWidth() / 2f, left.getHeight() / 2f, 1000);
        assertEquals(SteamControls.LEFT_STICK_TOUCH_FLAG, last());
        right.down(1, right.getWidth() / 2f, right.getHeight() / 2f, 1000);
        assertEquals(SteamControls.LEFT_STICK_TOUCH_FLAG | SteamControls.RIGHT_STICK_TOUCH_FLAG, last());
        right.up(1, 1050);
        assertEquals(SteamControls.LEFT_STICK_TOUCH_FLAG, last());
        left.up(0, 1050);
        assertEquals(0, last());

        // A tap and a touch held is the stick's click; the touch stays with it.
        left.down(0, left.getWidth() / 2f, left.getHeight() / 2f, 2000);
        left.up(0, 2050);
        left.down(0, left.getWidth() / 2f, left.getHeight() / 2f, 2150);
        assertEquals(ControllerPacket.LS_CLK_FLAG | SteamControls.LEFT_STICK_TOUCH_FLAG, last());
        left.up(0, 2300);
        assertEquals(0, last());
        controller.destroy();

        // Without the Steam controls a touched stick sends nothing but where it points.
        prefs.oscSteamControls = false;
        buttons.clear();
        controller = controller();
        left = controller.view(PadLayout.Control.LEFT_STICK);
        left.down(0, left.getWidth() / 2f, left.getHeight() / 2f, 3000);
        assertTrue(buttons.isEmpty() || last() == 0);
        left.up(0, 3050);
        controller.destroy();
        assertEquals(0, PadInput.stickTouchFlag(PadLayout.Control.WASD));
        assertEquals(0, PadInput.stickTouchFlag(PadLayout.Control.L3));
    }

    @Test public void aGripHoldsFromOneTapToTheNextAndLetsGoWhenTheControlsDo() {
        ModernController controller = controller();
        PadControlView grip = controller.view(PadLayout.Control.LEFT_GRIP);
        grip.down(0, 10, 10, 100);
        grip.up(0, 150);
        assertEquals(SteamControls.LEFT_GRIP_TOUCH_FLAG, last());
        assertTrue(grip.state().pressed);

        // The other grip joins it.
        PadControlView right = controller.view(PadLayout.Control.RIGHT_GRIP);
        right.down(1, 10, 10, 200);
        right.up(1, 250);
        assertEquals(SteamControls.LEFT_GRIP_TOUCH_FLAG | SteamControls.RIGHT_GRIP_TOUCH_FLAG, last());

        grip.down(0, 10, 10, 300);
        grip.up(0, 350);
        assertEquals(SteamControls.RIGHT_GRIP_TOUCH_FLAG, last());

        // Hiding the controls lets go of everything, a held grip included.
        controller.hide();
        assertEquals(0, last());
        assertFalse(right.state().pressed);
    }

    @Test public void aTrackpadSendsItsFingerToItsHalfOfTouchpadZero() {
        ModernController controller = controller();
        PadControlView left = controller.view(PadLayout.Control.LEFT_PAD);
        float w = left.getWidth(), h = left.getHeight();
        assertTrue(w > 0 && h > 0);
        left.down(0, w / 2, h / 4, 100);
        int[] down = touches.get(touches.size() - 1);
        assertEquals(MoonBridge.LI_TOUCH_EVENT_DOWN, down[0]);
        assertEquals(SteamControls.pointerId(SteamControls.LEFT_PAD), down[1]);
        assertEquals(Math.round(SteamControls.touchX(SteamControls.LEFT_PAD, .5f) * 1000), down[2]);
        assertEquals(250, down[3]);
        // A trackpad is no button: the gamepad report stays as it was.
        assertTrue(buttons.isEmpty() || last() == 0);

        left.move(0, w, h);
        int[] move = touches.get(touches.size() - 1);
        assertEquals(MoonBridge.LI_TOUCH_EVENT_MOVE, move[0]);
        assertEquals(Math.round(SteamControls.touchX(SteamControls.LEFT_PAD, 1) * 1000), move[2]);
        assertEquals(1000, move[3]);
        left.up(0, 200);
        assertEquals(MoonBridge.LI_TOUCH_EVENT_UP, touches.get(touches.size() - 1)[0]);

        // The right pad is the right half; a second finger on a pad is ignored, and the controls letting go cancel the touch.
        PadControlView right = controller.view(PadLayout.Control.RIGHT_PAD);
        right.down(3, 0, 0, 300);
        int[] rightDown = touches.get(touches.size() - 1);
        assertEquals(SteamControls.pointerId(SteamControls.RIGHT_PAD), rightDown[1]);
        assertEquals(500, rightDown[2]);
        int count = touches.size();
        right.down(4, w / 2, h / 2, 310);
        assertEquals(count, touches.size());
        controller.releaseInput();
        int[] cancel = touches.get(touches.size() - 1);
        assertEquals(MoonBridge.LI_TOUCH_EVENT_CANCEL, cancel[0]);
        assertEquals(SteamControls.pointerId(SteamControls.RIGHT_PAD), cancel[1]);
        assertFalse(right.state().pressed);
    }

    /**
     * A tap on a trackpad presses and releases the pad's own click button, by the pointer touchpad's
     * tap thresholds: short, and the finger stayed near where it landed. A drag or a lingering finger
     * moves the pad and clicks nothing.
     */
    @Test public void aTapOnATrackpadClicksItsPadAndADragOrALingeringFingerDoesNot() {
        ModernController controller = controller();
        PadControlView left = controller.view(PadLayout.Control.LEFT_PAD);
        float w = left.getWidth(), h = left.getHeight();

        // A tap: the click presses with the finger still on the pad, holds a moment, releases, and
        // only then does the touch end.
        int count = buttons.size();
        left.down(0, w / 2, h / 4, 100);
        assertEquals(count, buttons.size());
        events.clear();
        left.up(0, 200);
        assertEquals("The press goes out with the touch still down",
                java.util.Arrays.asList("b" + SteamControls.LEFT_PAD_CLICK_FLAG), events);
        ShadowLooper.idleMainLooper(PadControlView.PAD_CLICK_HOLD_MS, TimeUnit.MILLISECONDS);
        assertEquals(java.util.Arrays.asList("b" + SteamControls.LEFT_PAD_CLICK_FLAG, "b0",
                "t" + MoonBridge.LI_TOUCH_EVENT_UP), events);
        assertEquals(MoonBridge.LI_TOUCH_EVENT_UP, touches.get(touches.size() - 1)[0]);
        assertEquals(0, last());

        // The right pad clicks its own button.
        PadControlView right = controller.view(PadLayout.Control.RIGHT_PAD);
        right.down(1, 5, 5, 300);
        right.up(1, 400);
        assertEquals(SteamControls.RIGHT_PAD_CLICK_FLAG, last());
        ShadowLooper.idleMainLooper(PadControlView.PAD_CLICK_HOLD_MS, TimeUnit.MILLISECONDS);
        assertEquals(SteamControls.RIGHT_PAD_CLICK_FLAG, buttons.get(buttons.size() - 2));
        assertEquals(0, last());
        assertEquals(MoonBridge.LI_TOUCH_EVENT_UP, touches.get(touches.size() - 1)[0]);

        // A drag: the finger crosses the pad within tap time, but too far.
        count = buttons.size();
        left.down(0, 10, 10, 1000);
        left.move(0, w, h);
        left.move(0, w, h / 2);
        left.up(0, 1100);
        assertEquals("A drag sends no click", count, buttons.size());

        // A long press: the finger stays put but lingers past the tap time.
        left.down(0, w / 2, h / 2, 2000);
        left.up(0, 2000 + PadInput.TOUCHPAD_TAP_MS + 50);
        assertEquals("A lingering finger sends no click", count, buttons.size());

        // A new touch within the hold ends the click first, in order.
        events.clear();
        left.down(0, w / 2, h / 2, 2500);
        left.up(0, 2550);
        left.down(0, w / 2, h / 2, 2560);
        assertEquals(java.util.Arrays.asList("t" + MoonBridge.LI_TOUCH_EVENT_DOWN,
                "b" + SteamControls.LEFT_PAD_CLICK_FLAG, "b0", "t" + MoonBridge.LI_TOUCH_EVENT_UP,
                "t" + MoonBridge.LI_TOUCH_EVENT_DOWN), events);
        left.up(0, 3000 - 10);
        ShadowLooper.idleMainLooper(PadControlView.PAD_CLICK_HOLD_MS, TimeUnit.MILLISECONDS);
        count = buttons.size();

        // Input taken away mid-touch is no tap either: the touch is cancelled, nothing clicks.
        left.down(0, w / 2, h / 2, 3000);
        controller.releaseInput();
        assertEquals(MoonBridge.LI_TOUCH_EVENT_CANCEL, touches.get(touches.size() - 1)[0]);
        assertEquals(count, buttons.size());
        controller.destroy();

        // Without the Steam controls there is no trackpad to tap, so nothing can click: their
        // setting alone puts the pads, and their clicks, on the pad's SC26 arrival.
        prefs.oscSteamControls = false;
        buttons.clear();
        controller = controller();
        assertNull(controller.view(PadLayout.Control.LEFT_PAD));
        assertNull(controller.view(PadLayout.Control.RIGHT_PAD));
        assertTrue(controller.forcedHidden(PadLayout.Control.LEFT_PAD));
        controller.destroy();
    }

    @Test public void theEditorPicksAndHidesThemLikeAnyControl() {
        ModernController controller = controller();
        controller.startEditing();
        PadEditor editor = (PadEditor) frame.getChildAt(frame.getChildCount() - 1);
        editor.setOpacity(40);
        assertEquals(40, editor.draft().spec(PadLayout.Control.RIGHT_PAD).opacity);
        editor.select(PadLayout.Control.RIGHT_GRIP);
        editor.toggleHidden();
        controller.finishEditing(true);
        assertEquals(View.GONE, controller.view(PadLayout.Control.RIGHT_GRIP).getVisibility());
        assertTrue(PadLayout.parse(controller.layout().serialize()).spec(PadLayout.Control.RIGHT_GRIP).hidden);
        assertEquals("Right grip", PadEditor.name(frame.getContext(), PadLayout.Control.RIGHT_GRIP));
    }

    @Test public void aLayoutSavedBeforeTheSteamControlsLoadsWithThemAtTheirDefaults() {
        // As 5.0.x saved it: a moved face cluster, a hidden Home and the WASD stick shown.
        PadLayout layout = PadLayout.parse("v1|FACE,0.7083,0.5556,1.300,100,0|HOME,-,-,1.000,100,1|WASD,-,-,1.000,100,0");
        assertEquals(.7083f, layout.spec(PadLayout.Control.FACE).cx, 1e-4);
        assertTrue(layout.spec(PadLayout.Control.HOME).hidden);
        assertFalse(layout.spec(PadLayout.Control.WASD).hidden);
        for (PadLayout.Control control : STEAM) {
            assertTrue(control.name(), layout.spec(control).isDefault());
            assertFalse(control.name(), layout.spec(control).hidden);
        }
        // A moved trackpad saves as an entry of its own, which an older build skips as an unknown control.
        layout.moveTo(PadLayout.Control.LEFT_PAD, 400, 300, 1600, 720, 2);
        String text = layout.serialize();
        assertTrue(text, text.contains("|LEFT_PAD,"));
        PadLayout again = PadLayout.parse(text);
        assertEquals(layout.spec(PadLayout.Control.LEFT_PAD).cx, again.spec(PadLayout.Control.LEFT_PAD).cx, 1e-3);
        assertEquals(.7083f, again.spec(PadLayout.Control.FACE).cx, 1e-4);
    }

    /** Phones both ways up and tablets: the controls lie on the surface and keep off the default controls and each other. */
    @Test public void theDefaultPlacesKeepClearOfEveryDefaultControl() {
        float[][] surfaces = {{2400, 1080, 2.75f}, {2340, 1080, 2.75f}, {1600, 720, 2}, {1920, 1080, 2.625f},
                {1280, 800, 2}, {1080, 2400, 2.75f}, {720, 1600, 2}, {2560, 1600, 2}, {2000, 1200, 2}, {1600, 2560, 2}};
        PadLayout layout = new PadLayout();
        float[] a = new float[4], b = new float[4];
        PadLayout.Control[] controls = PadLayout.Control.values();
        for (float[] surface : surfaces) {
            String where = surface[0] + "x" + surface[1];
            for (PadLayout.Control control : STEAM) {
                layout.bounds(control, surface[0], surface[1], surface[2], a);
                assertTrue(control + " is a full touch on " + where, Math.min(a[2], a[3]) >= 48 * surface[2] - .01f);
                for (PadLayout.Control other : controls) {
                    if (other == control || other.hiddenByDefault) continue;
                    layout.bounds(other, surface[0], surface[1], surface[2], b);
                    boolean overlap = a[0] < b[0] + b[2] && b[0] < a[0] + a[2] && a[1] < b[1] + b[3] && b[1] < a[1] + a[3];
                    assertFalse(control + " overlaps " + other + " on " + where, overlap);
                }
            }
        }
    }
}
