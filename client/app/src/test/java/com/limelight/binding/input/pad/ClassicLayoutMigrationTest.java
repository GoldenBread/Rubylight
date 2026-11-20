package com.limelight.binding.input.pad;

import android.app.Application;
import android.content.Context;
import android.content.SharedPreferences;
import android.view.View;
import android.widget.FrameLayout;

import com.limelight.binding.input.HeldInputState;
import com.limelight.nvstream.input.ControllerPacket;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.ui.UiNavigationTest;

import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;

import static org.junit.Assert.*;

/**
 * The classic controller's saved boxes become this controller's layout once per install, when it is first built
 * after the update that removed the classic controller.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 30, application = Application.class, qualifiers = "w800dp-h360dp-land-xhdpi",
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class)
public class ClassicLayoutMigrationTest {
    private static final float WIDTH = 1600, HEIGHT = 720, DENSITY = 2;
    /** The classic right stick, at (1300, 500) on this 1600x720 screen. */
    private static final String RIGHT_STICK = "{\"LEFT\":1200,\"TOP\":400,\"WIDTH\":200,\"HEIGHT\":200}";
    private static final PadLayout.Control[] MEMBERS = PadLayout.members(PadLayout.Control.FACE);
    private Context context;
    private SharedPreferences store, classic;
    private final List<Integer> buttons = new ArrayList<>();

    @Before
    public void setUp() {
        context = RuntimeEnvironment.getApplication();
        store = context.getSharedPreferences(ModernController.PREFS, Context.MODE_PRIVATE);
        classic = context.getSharedPreferences(ClassicLayoutMigration.CLASSIC_PREFS, Context.MODE_PRIVATE);
        store.edit().clear().commit();
        classic.edit().clear().commit();
    }

    private PadLayout shared() {
        return PadLayout.parse(store.getString(PadLayoutKeys.legacyKey(null), null));
    }

    @Test public void theClassicLayoutBecomesTheLayoutForAllGamesOnce() {
        classic.edit().putString("13", RIGHT_STICK).putString("16", "{\"LEFT\":760,\"TOP\":100,\"WIDTH\":80,\"HEIGHT\":80}")
                .commit();
        ClassicLayoutMigration.carryOverOnce(context, store, false);
        assertTrue(store.getBoolean(ClassicLayoutMigration.MIGRATED, false));
        // The older shared key is what both orientations fall back to, for every game without its own layout.
        PadLayout.Spec stick = shared().spec(PadLayout.Control.RIGHT_STICK);
        assertTrue(stick.placed());
        assertEquals(1300f / WIDTH, stick.cx, .01f);
        assertEquals(500f / HEIGHT, stick.cy, .01f);
        assertFalse(shared().spec(PadLayout.Control.HOME).hidden);

        // Exactly once: a later run carries nothing over, whatever the classic preferences hold by then.
        classic.edit().putString("13", "{\"LEFT\":0,\"TOP\":0,\"WIDTH\":200,\"HEIGHT\":200}").commit();
        store.edit().remove(PadLayoutKeys.legacyKey(null)).commit();
        ClassicLayoutMigration.carryOverOnce(context, store, false);
        assertFalse(store.contains(PadLayoutKeys.legacyKey(null)));
    }

    @Test public void withNothingClassicSavedItIsDoneAndWritesNothing() {
        ClassicLayoutMigration.carryOverOnce(context, store, true);
        assertTrue(store.getBoolean(ClassicLayoutMigration.MIGRATED, false));
        assertFalse(store.contains(PadLayoutKeys.legacyKey(null)));
        // A classic layout that appears later is never read.
        classic.edit().putString("13", RIGHT_STICK).commit();
        ClassicLayoutMigration.carryOverOnce(context, store, false);
        assertFalse(store.contains(PadLayoutKeys.legacyKey(null)));
    }

    @Test public void aLayoutForAllGamesArrangedInThisControllerWins() {
        classic.edit().putString("13", RIGHT_STICK).commit();
        String mine = "v1|FACE,0.7083,0.5556,1.300,100,0";
        store.edit().putString(PadLayoutKeys.legacyKey(null), mine).commit();
        ClassicLayoutMigration.carryOverOnce(context, store, false);
        assertEquals(mine, store.getString(PadLayoutKeys.legacyKey(null), null));
        assertTrue(store.getBoolean(ClassicLayoutMigration.MIGRATED, false));

        // Both orientations with a layout of their own leave nothing for the classic one to fill.
        store.edit().clear().putString(PadLayoutKeys.key(true, null), mine).putString(PadLayoutKeys.key(false, null), mine)
                .commit();
        ClassicLayoutMigration.carryOverOnce(context, store, false);
        assertFalse(store.contains(PadLayoutKeys.legacyKey(null)));
        assertEquals(mine, store.getString(PadLayoutKeys.key(true, null), null));
        assertEquals(mine, store.getString(PadLayoutKeys.key(false, null), null));
    }

    @Test public void theClassicLayoutFillsOnlyWhatHasNoLayoutOfItsOwn() {
        classic.edit().putString("13", RIGHT_STICK).commit();
        String landscape = "v1|FACE,0.7083,0.5556,1.300,100,0", game = "v1|DPAD,0.2,0.6,1.000,100,0";
        store.edit().putString(PadLayoutKeys.key(true, null), landscape)
                .putString(PadLayoutKeys.key(true, "pc-1/app-7"), game).commit();
        ClassicLayoutMigration.carryOverOnce(context, store, false);
        // Portrait had no layout for all games; landscape and the game keep theirs, and they win where they exist.
        assertTrue(shared().spec(PadLayout.Control.RIGHT_STICK).placed());
        assertEquals(landscape, store.getString(PadLayoutKeys.key(true, null), null));
        assertEquals(game, store.getString(PadLayoutKeys.key(true, "pc-1/app-7"), null));
    }

    @Test public void aPlayerWhoPickedThisControllerOrResetTheLayoutGetsNothingClassic() {
        classic.edit().putString("13", RIGHT_STICK).commit();
        ClassicLayoutMigration.skip(context);
        ClassicLayoutMigration.carryOverOnce(context, store, false);
        assertFalse(store.contains(PadLayoutKeys.legacyKey(null)));

        // Reset sets the same flag, so the defaults stay defaults.
        store.edit().clear().commit();
        ModernController.clearLayouts(context);
        ClassicLayoutMigration.carryOverOnce(context, store, false);
        assertFalse(store.contains(PadLayoutKeys.legacyKey(null)));
        assertTrue(store.getBoolean(ClassicLayoutMigration.MIGRATED, false));
    }

    @Test public void aCorruptClassicControlIsLeftOut() {
        classic.edit().putString("12", "not json").putString("1", "{\"LEFT\":5}").putInt("14", 3)
                .putString("13", RIGHT_STICK).commit();
        Map<Integer, float[]> boxes = ClassicLayoutMigration.boxes(classic);
        assertEquals(1, boxes.size());
        assertArrayEquals(new float[] {1200, 400, 200, 200, 100}, boxes.get(13), 0);
        ClassicLayoutMigration.carryOverOnce(context, store, false);
        assertFalse(shared().spec(PadLayout.Control.LEFT_STICK).placed());
        assertTrue(shared().spec(PadLayout.Control.RIGHT_STICK).placed());
    }

    /** An older build's carry-over, from the first time the player tried this controller: the stick at the left. */
    private static final String EARLIER = "v1|RIGHT_STICK,0.1000,0.5000,1.000,100,0";

    private void retire(String style) {
        android.preference.PreferenceManager.getDefaultSharedPreferences(context).edit()
                .putString("list_osc_style", style).commit();
        PreferenceConfiguration.readPreferences(context);
    }

    @Test public void aPlayerWhoWentBackToTheClassicControllerGetsTheLayoutTheyKeptArranging() {
        // They tried this controller once, which carried the classic layout over and set the flag, then went back
        // to the classic controller and moved its right stick to (1300, 500).
        store.edit().putBoolean(ClassicLayoutMigration.MIGRATED, true)
                .putString(PadLayoutKeys.legacyKey(null), EARLIER).commit();
        classic.edit().putString("13", RIGHT_STICK).commit();
        retire("classic");
        ClassicLayoutMigration.carryOverOnce(context, store, false);
        PadLayout.Spec stick = shared().spec(PadLayout.Control.RIGHT_STICK);
        assertEquals(1300f / WIDTH, stick.cx, .01f);
        assertEquals(500f / HEIGHT, stick.cy, .01f);
        assertTrue(store.getBoolean(ClassicLayoutMigration.MIGRATED, false));
        assertFalse(store.contains(ClassicLayoutMigration.CLASSIC_STYLE));

        // Still only once: the classic layout moved later is never read.
        String carried = store.getString(PadLayoutKeys.legacyKey(null), null);
        classic.edit().putString("13", "{\"LEFT\":0,\"TOP\":0,\"WIDTH\":200,\"HEIGHT\":200}").commit();
        ClassicLayoutMigration.carryOverOnce(context, store, false);
        assertEquals(carried, store.getString(PadLayoutKeys.legacyKey(null), null));
    }

    @Test public void goingBackToTheClassicControllerKeepsWhatThePlayerArrangedHere() {
        store.edit().putBoolean(ClassicLayoutMigration.MIGRATED, true)
                .putString(PadLayoutKeys.legacyKey(null), EARLIER).commit();
        classic.edit().putString("13", RIGHT_STICK).commit();
        // A layout arranged in this controller for both orientations: the earlier carry-over can reach nobody, and
        // what the player arranged stays.
        String mine = "v1|FACE,0.7083,0.5556,1.300,100,0";
        store.edit().putString(PadLayoutKeys.key(true, null), mine).putString(PadLayoutKeys.key(false, null), mine)
                .commit();
        retire("classic");
        ClassicLayoutMigration.carryOverOnce(context, store, false);
        assertEquals(mine, store.getString(PadLayoutKeys.key(true, null), null));
        assertEquals(mine, store.getString(PadLayoutKeys.key(false, null), null));
        assertEquals(EARLIER, store.getString(PadLayoutKeys.legacyKey(null), null));

        // A player who stayed with this controller keeps the earlier carry-over: it is the layout they play with.
        store.edit().clear().putBoolean(ClassicLayoutMigration.MIGRATED, true)
                .putString(PadLayoutKeys.legacyKey(null), EARLIER).commit();
        retire("modern");
        ClassicLayoutMigration.carryOverOnce(context, store, false);
        assertEquals(EARLIER, store.getString(PadLayoutKeys.legacyKey(null), null));
    }

    @Test public void aClassicPlayerWithTheNintendoLayoutKeepsBAtTheBottomOfTheDefaults() {
        // Nothing classic was arranged: the classic defaults sent B from the bottom, A from the right, Y from the
        // left and X from the top.
        retire("classic");
        ClassicLayoutMigration.carryOverOnce(context, store, true);
        PadLayout layout = shared();
        assertTrue(layout.isSplit(PadLayout.Control.FACE));
        PadLayout plain = new PadLayout();
        plain.split(PadLayout.Control.FACE, WIDTH, HEIGHT, DENSITY);
        assertBox("bottom sends B", box(plain, PadLayout.Control.A), box(layout, PadLayout.Control.B));
        assertBox("right sends A", box(plain, PadLayout.Control.B), box(layout, PadLayout.Control.A));
        assertBox("left sends Y", box(plain, PadLayout.Control.X), box(layout, PadLayout.Control.Y));
        assertBox("top sends X", box(plain, PadLayout.Control.Y), box(layout, PadLayout.Control.X));
        assertEquals(PadLayout.Glyph.XBOX, layout.glyph());
        assertFalse(store.contains(ClassicLayoutMigration.CLASSIC_STYLE));
    }

    @Test public void theNintendoLayoutLeavesTheDefaultsOfEveryoneElseAlone() {
        // A player who had picked this controller: it never swapped its buttons on screen.
        retire("modern");
        ClassicLayoutMigration.carryOverOnce(context, store, true);
        assertFalse(store.contains(PadLayoutKeys.legacyKey(null)));
        // No stored style, as on a fresh install: nothing tells a classic player, so the defaults stay.
        store.edit().clear().commit();
        ClassicLayoutMigration.carryOverOnce(context, store, true);
        assertFalse(store.contains(PadLayoutKeys.legacyKey(null)));
        // A classic player without the setting keeps the plain defaults.
        store.edit().clear().commit();
        retire("classic");
        ClassicLayoutMigration.carryOverOnce(context, store, false);
        assertFalse(store.contains(PadLayoutKeys.legacyKey(null)));
    }

    @Test public void aMigratedNintendoDefaultSendsBFromTheBottom() {
        retire("classic");
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.flipFaceButtons = true;
        ModernController controller = controller(frame(), prefs);
        PadControlView bottom = controller.view(PadLayout.Control.B);
        PadLayout plain = new PadLayout();
        plain.split(PadLayout.Control.FACE, WIDTH, HEIGHT, DENSITY);
        float[] place = box(plain, PadLayout.Control.A);
        assertEquals(place[0] + place[2] / 2, bottom.getLeft() + bottom.getWidth() / 2f, 2);
        assertEquals(place[1] + place[3] / 2, bottom.getTop() + bottom.getHeight() / 2f, 2);
        bottom.down(0, bottom.getWidth() / 2f, bottom.getHeight() / 2f, 100);
        assertEquals(ControllerPacket.B_FLAG, (int) buttons.get(buttons.size() - 1));
        bottom.up(0, 200);
        controller.destroy();
    }

    @Test public void aClassicPlayerWhoResetTheClassicLayoutGetsTheDefaults() {
        store.edit().putBoolean(ClassicLayoutMigration.MIGRATED, true)
                .putString(PadLayoutKeys.legacyKey(null), EARLIER).commit();
        retire("classic");
        ClassicLayoutMigration.carryOverOnce(context, store, false);
        assertFalse(store.contains(PadLayoutKeys.legacyKey(null)));
        assertTrue(store.getBoolean(ClassicLayoutMigration.MIGRATED, false));
    }

    /**
     * The classic controller's default layout on this 1600x720 screen, as it saved it: its 128x72 grid scaled to the
     * height (ten pixels a cell) and the right-hand controls moved by the 320 pixels this screen is wider than 16:9.
     */
    private static Map<Integer, float[]> classicDefault() {
        Map<Integer, float[]> boxes = new HashMap<>();
        boxes.put(1, new float[] {40, 410, 300, 300});    // D-pad
        boxes.put(6, new float[] {1380, 210, 100, 100});  // A, bottom
        boxes.put(7, new float[] {1480, 110, 100, 100});  // B, right
        boxes.put(8, new float[] {1280, 110, 100, 100});  // X, left
        boxes.put(9, new float[] {1380, 10, 100, 100});   // Y, top
        boxes.put(2, new float[] {10, 310, 120, 90});     // LT
        boxes.put(3, new float[] {1470, 310, 120, 90});   // RT
        boxes.put(4, new float[] {240, 310, 120, 90});    // LB
        boxes.put(5, new float[] {1240, 310, 120, 90});   // RB
        boxes.put(12, new float[] {60, 40, 260, 260});    // left stick
        boxes.put(13, new float[] {1300, 420, 260, 260}); // right stick
        boxes.put(10, new float[] {340, 640, 120, 70});   // Back
        boxes.put(11, new float[] {1150, 640, 120, 70});  // Start
        boxes.put(16, new float[] {810, 640, 120, 70});   // Guide
        return boxes;
    }

    private static float[] box(PadLayout layout, PadLayout.Control control) {
        return layout.bounds(control, WIDTH, HEIGHT, DENSITY, new float[4]);
    }

    private static void assertBox(String what, float[] expected, float[] actual) {
        for (int i = 0; i < 4; i++) assertEquals(what + " [" + i + "]", expected[i], actual[i], 1.5f);
    }

    private static void assertCenter(String what, float[] classic, float[] actual) {
        assertEquals(what, classic[0] + classic[2] / 2, actual[0] + actual[2] / 2, 1.5f);
        assertEquals(what, classic[1] + classic[3] / 2, actual[1] + actual[3] / 2, 1.5f);
    }

    @Test public void theClassicDefaultLayoutBecomesTheClusterWhereItsButtonsWere() {
        Map<Integer, float[]> classic = classicDefault();
        PadLayout layout = ClassicLayoutMigration.migrate(classic, WIDTH, HEIGHT, DENSITY);
        // Its four face buttons still stand in their diamond: one cluster over the box they span.
        assertTrue(layout.shown(PadLayout.Control.FACE));
        assertFalse(layout.isSplit(PadLayout.Control.FACE));
        assertBox("face", new float[] {1280, 10, 300, 300}, box(layout, PadLayout.Control.FACE));
        assertBox("D-pad", classic.get(1), box(layout, PadLayout.Control.DPAD));
        assertBox("left stick", classic.get(12), box(layout, PadLayout.Control.LEFT_STICK));
        assertBox("right stick", classic.get(13), box(layout, PadLayout.Control.RIGHT_STICK));
        // The small classic buttons grow around their place to this controller's full touch (48 dp, 72 dp pills),
        // kept on the screen: each new box holds the classic one.
        for (int id : new int[] {2, 3, 4, 5, 10, 11, 16}) {
            PadLayout.Control control = ClassicLayoutMigration.control(id);
            float[] was = classic.get(id), now = box(layout, control);
            assertTrue(control.name(), now[0] <= was[0] + .5f && now[1] <= was[1] + .5f);
            assertTrue(control.name(), now[0] + now[2] >= was[0] + was[2] - .5f);
            assertTrue(control.name(), now[1] + now[3] >= was[1] + was[3] - .5f);
            assertTrue(control.name(), now[2] < was[2] * 1.5f);
        }
        assertFalse("a control the default leaves out is shown", layout.spec(PadLayout.Control.HOME).hidden);
        // The pointer touchpad did not exist then and keeps the default (hidden until added).
        assertTrue(layout.spec(PadLayout.Control.POINTER_PAD).hidden);
        for (PadLayout.Control control : PadLayout.Control.values()) {
            assertEquals(control.name(), PadLayout.MAX_OPACITY, layout.spec(control).opacity);
        }
    }

    @Test public void eachClassicBoxKeepsItsOwnWidthAndHeight() {
        Map<Integer, float[]> boxes = new HashMap<>();
        boxes.put(12, new float[] {0, 120, 700, 600});    // left stick, wider than tall
        boxes.put(1, new float[] {720, 500, 400, 160});   // D-pad, flat
        boxes.put(17, new float[] {700, 20, 200, 300});   // Touchpad button, taller than wide
        PadLayout layout = ClassicLayoutMigration.migrate(boxes, WIDTH, HEIGHT, DENSITY);
        for (Map.Entry<Integer, float[]> entry : boxes.entrySet()) {
            PadLayout.Control control = ClassicLayoutMigration.control(entry.getKey());
            assertBox(control.name(), entry.getValue(), box(layout, control));
        }
        PadLayout.Spec stick = layout.spec(PadLayout.Control.LEFT_STICK);
        assertNotEquals(stick.widthScale, stick.heightScale, .01f);
        // A box past the screen's edge or size comes back onto it, at most the whole screen.
        boxes.clear();
        boxes.put(13, new float[] {-100, 0, 2000, 900});
        assertBox("whole screen", new float[] {0, 0, WIDTH, HEIGHT},
                box(ClassicLayoutMigration.migrate(boxes, WIDTH, HEIGHT, DENSITY), PadLayout.Control.RIGHT_STICK));
    }

    @Test public void eachControlKeepsItsOwnOpacity() {
        Map<Integer, float[]> boxes = classicDefault();
        boxes.put(12, new float[] {60, 40, 260, 260, 40});
        boxes.put(1, new float[] {40, 410, 300, 300, 5});
        PadLayout layout = ClassicLayoutMigration.migrate(boxes, WIDTH, HEIGHT, DENSITY);
        assertEquals(40, layout.spec(PadLayout.Control.LEFT_STICK).opacity);
        assertEquals("never below this controller's floor", PadLayout.MIN_OPACITY,
                layout.spec(PadLayout.Control.DPAD).opacity);
        assertEquals("saved before opacity: opaque", PadLayout.MAX_OPACITY,
                layout.spec(PadLayout.Control.RIGHT_STICK).opacity);

        // The saved preferences carry the opacity too.
        classic.edit().putString("12", "{\"LEFT\":60,\"TOP\":40,\"WIDTH\":260,\"HEIGHT\":260,\"OPACITY\":35}")
                .putString("13", RIGHT_STICK).commit();
        Map<Integer, float[]> saved = ClassicLayoutMigration.boxes(classic);
        assertEquals(35, saved.get(12)[ClassicLayoutMigration.OPACITY], 0);
        assertEquals(100, saved.get(13)[ClassicLayoutMigration.OPACITY], 0);
    }

    @Test public void faceButtonsMovedApartBecomeTheClustersMembersWhereThePlayerPutThem() {
        Map<Integer, float[]> boxes = classicDefault();
        boxes.put(6, new float[] {1400, 400, 80, 80});     // A, moved down beside the right stick
        boxes.put(7, new float[] {1480, 400, 80, 80, 60}); // B, next to it and fainter
        PadLayout layout = ClassicLayoutMigration.migrate(boxes, WIDTH, HEIGHT, DENSITY);
        assertTrue(layout.isSplit(PadLayout.Control.FACE));
        assertFalse(layout.shown(PadLayout.Control.FACE));
        int[] ids = {6, 7, 8, 9};
        for (int i = 0; i < ids.length; i++) {
            assertTrue(MEMBERS[i].name(), layout.shown(MEMBERS[i]));
            assertCenter(MEMBERS[i].name(), boxes.get(ids[i]), box(layout, MEMBERS[i]));
        }
        assertEquals(60, layout.spec(PadLayout.Control.B).opacity);
        assertEquals(PadLayout.MAX_OPACITY, layout.spec(PadLayout.Control.A).opacity);

        // In their diamond but at different opacities they stay four, each as faint as it was.
        boxes = classicDefault();
        boxes.put(9, new float[] {1380, 10, 100, 100, 50});
        layout = ClassicLayoutMigration.migrate(boxes, WIDTH, HEIGHT, DENSITY);
        assertTrue(layout.isSplit(PadLayout.Control.FACE));
        assertEquals(50, layout.spec(PadLayout.Control.Y).opacity);

        // A button the classic layout did not save shows over its place in the default cluster.
        boxes = classicDefault();
        boxes.remove(9);
        layout = ClassicLayoutMigration.migrate(boxes, WIDTH, HEIGHT, DENSITY);
        assertTrue(layout.shown(PadLayout.Control.Y));
        assertFalse(layout.spec(PadLayout.Control.Y).placed());
        assertTrue(layout.spec(PadLayout.Control.A).placed());
    }

    @Test public void theDiamondAllowsAQuarterButtonAndNoMore() {
        float[][] diamond = {{1380, 210, 100, 100}, {1480, 110, 100, 100}, {1280, 110, 100, 100}, {1380, 10, 100, 100}};
        assertTrue(ClassicLayoutMigration.classicDiamond(diamond));
        // The classic grid's rounding and a nudge stay the cluster.
        diamond[1] = new float[] {1490, 115, 100, 100};
        assertTrue(ClassicLayoutMigration.classicDiamond(diamond));
        // Half a button off, a button resized apart, or a row of four does not.
        diamond[1] = new float[] {1530, 110, 100, 100};
        assertFalse(ClassicLayoutMigration.classicDiamond(diamond));
        diamond[1] = new float[] {1480, 110, 160, 100};
        assertFalse(ClassicLayoutMigration.classicDiamond(diamond));
        float[][] row = {{1000, 300, 100, 100}, {1100, 300, 100, 100}, {1200, 300, 100, 100}, {1300, 300, 100, 100}};
        assertFalse(ClassicLayoutMigration.classicDiamond(row));
    }

    @Test public void withTheNintendoLayoutEachPlaceKeepsTheButtonItSent() {
        // Classic sent B from the bottom button, A from the right one, Y from the left and X from the top.
        PadLayout layout = ClassicLayoutMigration.migrate(classicDefault(), WIDTH, HEIGHT, DENSITY, true);
        assertTrue(layout.isSplit(PadLayout.Control.FACE));
        Map<Integer, float[]> classic = classicDefault();
        assertCenter("bottom sends B", classic.get(6), box(layout, PadLayout.Control.B));
        assertCenter("right sends A", classic.get(7), box(layout, PadLayout.Control.A));
        assertCenter("left sends Y", classic.get(8), box(layout, PadLayout.Control.Y));
        assertCenter("top sends X", classic.get(9), box(layout, PadLayout.Control.X));
        // The glyphs stay Xbox, so each button still shows the letter it sends, as on the classic controller.
        assertEquals(PadLayout.Glyph.XBOX, layout.glyph());
    }

    private FrameLayout frame() {
        UiNavigationTest.Screen screen = Robolectric.buildActivity(UiNavigationTest.Screen.class).setup().visible().get();
        FrameLayout frame = new FrameLayout(screen);
        screen.setContentView(frame, new FrameLayout.LayoutParams((int) WIDTH, (int) HEIGHT));
        layout(frame);
        return frame;
    }

    private static void layout(FrameLayout frame) {
        frame.measure(View.MeasureSpec.makeMeasureSpec((int) WIDTH, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec((int) HEIGHT, View.MeasureSpec.EXACTLY));
        frame.layout(0, 0, (int) WIDTH, (int) HEIGHT);
    }

    private ModernController controller(FrameLayout frame, PreferenceConfiguration prefs) {
        ModernController controller = new ModernController(frame.getContext(), frame, new HeldInputState.Sink() {
            @Override public void key(int key, boolean down) { }
            @Override public void mouse(int button, boolean down) { }
            @Override public void gamepad(int b, short lx, short ly, short rx, short ry, byte lt, byte rt) {
                buttons.add(b);
            }
        }, prefs, null);
        controller.refreshLayout();
        controller.show();
        layout(frame);
        return controller;
    }

    private void putClassic(int id, int left, int top, int width, int height, int opacity) {
        classic.edit().putString(String.valueOf(id), "{\"LEFT\":" + left + ",\"TOP\":" + top + ",\"WIDTH\":" + width +
                ",\"HEIGHT\":" + height + ",\"OPACITY\":" + opacity + "}").commit();
    }

    /** The reported layout: controls stretched over the screen at opacity 0, so a stray touch never reaches the stream. */
    @Test public void aFullScreenClassicLayoutAtOpacityZeroComesOutTheSame() {
        int[][] classicBoxes = {
                {1, 0, 0, 640, 240},       // D-pad over the top left
                {12, 0, 240, 640, 480},    // left stick under it
                {10, 640, 0, 160, 720},    // Back and Start, the middle column
                {11, 800, 0, 160, 720},
                {8, 960, 0, 320, 180},     // X, Y, A and B as a grid over the top right
                {9, 1280, 0, 320, 180},
                {6, 960, 180, 320, 180},
                {7, 1280, 180, 320, 180},
                {13, 960, 360, 640, 360},  // right stick under them
        };
        for (int[] b : classicBoxes) putClassic(b[0], b[1], b[2], b[3], b[4], 10);
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.oscOpacity = 0;
        ModernController controller = controller(frame(), prefs);
        assertTrue(controller.layout().isSplit(PadLayout.Control.FACE));
        for (int[] b : classicBoxes) {
            PadLayout.Control control = ClassicLayoutMigration.control(b[0]);
            if (control == null) control = MEMBERS[b[0] - 6];
            View view = controller.view(control);
            assertEquals(control.name(), View.VISIBLE, view.getVisibility());
            assertBox(control.name(), new float[] {b[1], b[2], b[3], b[4]},
                    new float[] {view.getLeft(), view.getTop(), view.getWidth(), view.getHeight()});
            assertEquals(control.name(), 10, controller.layout().spec(control).opacity);
            assertEquals(control.name(), 0f, view.getAlpha(), 0);
        }
        controller.destroy();
    }

    @Test public void aMigratedNintendoLayoutSendsFromEachPlaceWhatTheClassicButtonSent() {
        for (Map.Entry<Integer, float[]> entry : classicDefault().entrySet()) {
            float[] b = entry.getValue();
            putClassic(entry.getKey(), (int) b[0], (int) b[1], (int) b[2], (int) b[3], 100);
        }
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.flipFaceButtons = true;
        ModernController controller = controller(frame(), prefs);
        // The classic bottom button (1380, 210, 100, 100) sent B: its place still does.
        PadControlView bottom = controller.view(PadLayout.Control.B);
        assertEquals(1380 + 50, bottom.getLeft() + bottom.getWidth() / 2, 2);
        assertEquals(210 + 50, bottom.getTop() + bottom.getHeight() / 2, 2);
        bottom.down(0, bottom.getWidth() / 2f, bottom.getHeight() / 2f, 100);
        assertEquals(ControllerPacket.B_FLAG, (int) buttons.get(buttons.size() - 1));
        bottom.up(0, 200);
        assertEquals(0, (int) buttons.get(buttons.size() - 1));
        controller.destroy();

        // Without the setting the same classic layout is the plain cluster: its bottom sends A.
        store.edit().clear().commit();
        prefs.flipFaceButtons = false;
        controller = controller(frame(), prefs);
        assertFalse(controller.layout().isSplit(PadLayout.Control.FACE));
        controller.destroy();
    }

    @Test public void anEmptyOrBrokenClassicLayoutLeavesTheModernDefaultsAlone() {
        assertTrue(ClassicLayoutMigration.migrate(null, WIDTH, HEIGHT, DENSITY).isDefault());
        assertTrue(ClassicLayoutMigration.migrate(new HashMap<>(), WIDTH, HEIGHT, DENSITY).isDefault());
        Map<Integer, float[]> broken = new HashMap<>();
        broken.put(12, null);
        broken.put(13, new float[] {0, 0, 0, 0});
        broken.put(999, new float[] {10, 10, 10, 10});
        assertTrue(ClassicLayoutMigration.migrate(broken, WIDTH, HEIGHT, DENSITY).isDefault());
        assertTrue(ClassicLayoutMigration.migrate(classicDefault(), 0, 0, DENSITY).isDefault());
    }

    @Test public void eachClassicElementMapsToTheControlItWas() {
        assertEquals(PadLayout.Control.LEFT_STICK, ClassicLayoutMigration.control(12));
        assertEquals(PadLayout.Control.RIGHT_STICK, ClassicLayoutMigration.control(13));
        assertEquals(PadLayout.Control.L3, ClassicLayoutMigration.control(14));
        assertEquals(PadLayout.Control.R3, ClassicLayoutMigration.control(15));
        assertEquals(PadLayout.Control.SELECT, ClassicLayoutMigration.control(10));
        assertEquals(PadLayout.Control.START, ClassicLayoutMigration.control(11));
        assertEquals(PadLayout.Control.TOUCHPAD, ClassicLayoutMigration.control(17));
        assertEquals(PadLayout.Control.LEFT_PAD, ClassicLayoutMigration.control(22));
        assertEquals(PadLayout.Control.RIGHT_PAD, ClassicLayoutMigration.control(23));
        assertNull("The face buttons become the cluster or its members", ClassicLayoutMigration.control(6));
        assertNull(ClassicLayoutMigration.control(999));
    }
}
