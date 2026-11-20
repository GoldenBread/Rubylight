package com.limelight.binding.input.pad;

import android.app.Application;
import android.content.Context;
import android.content.SharedPreferences;

import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

import java.util.HashMap;
import java.util.Map;

import static org.junit.Assert.*;

/**
 * The classic controller's saved boxes become this controller's layout once per install, when it is first built
 * after the update that removed the classic controller.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 30, application = Application.class, qualifiers = "w800dp-h360dp-land-xhdpi")
public class ClassicLayoutMigrationTest {
    private static final float WIDTH = 1600, HEIGHT = 720, DENSITY = 2;
    /** The classic right stick, at (1300, 500) on this 1600x720 screen. */
    private static final String RIGHT_STICK = "{\"LEFT\":1200,\"TOP\":400,\"WIDTH\":200,\"HEIGHT\":200}";
    private Context context;
    private SharedPreferences store, classic;

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
        ClassicLayoutMigration.carryOverOnce(context, store);
        assertTrue(store.getBoolean(ClassicLayoutMigration.MIGRATED, false));
        // The older shared key is what both orientations fall back to, for every game without its own layout.
        PadLayout.Spec stick = shared().spec(PadLayout.Control.RIGHT_STICK);
        assertTrue(stick.placed());
        assertEquals(1300f / WIDTH, stick.cx, .01f);
        assertEquals(500f / HEIGHT, stick.cy, .01f);
        assertFalse(shared().spec(PadLayout.Control.HOME).hidden);

        // Exactly once: a later run carries nothing over, whatever the classic preferences hold by then.
        String carried = store.getString(PadLayoutKeys.legacyKey(null), null);
        classic.edit().putString("13", "{\"LEFT\":0,\"TOP\":0,\"WIDTH\":200,\"HEIGHT\":200}").commit();
        store.edit().remove(PadLayoutKeys.legacyKey(null)).commit();
        ClassicLayoutMigration.carryOverOnce(context, store);
        assertFalse(store.contains(PadLayoutKeys.legacyKey(null)));
        assertNotNull(carried);
    }

    @Test public void withNothingClassicSavedItIsDoneAndWritesNothing() {
        ClassicLayoutMigration.carryOverOnce(context, store);
        assertTrue(store.getBoolean(ClassicLayoutMigration.MIGRATED, false));
        assertFalse(store.contains(PadLayoutKeys.legacyKey(null)));
        // A classic layout that appears later is never read.
        classic.edit().putString("13", RIGHT_STICK).commit();
        ClassicLayoutMigration.carryOverOnce(context, store);
        assertFalse(store.contains(PadLayoutKeys.legacyKey(null)));
    }

    @Test public void aLayoutForAllGamesArrangedInThisControllerWins() {
        classic.edit().putString("13", RIGHT_STICK).commit();
        String mine = "v1|FACE,0.7083,0.5556,1.300,100,0";
        store.edit().putString(PadLayoutKeys.legacyKey(null), mine).commit();
        ClassicLayoutMigration.carryOverOnce(context, store);
        assertEquals(mine, store.getString(PadLayoutKeys.legacyKey(null), null));
        assertTrue(store.getBoolean(ClassicLayoutMigration.MIGRATED, false));

        // Both orientations with a layout of their own leave nothing for the classic one to fill.
        store.edit().clear().putString(PadLayoutKeys.key(true, null), mine).putString(PadLayoutKeys.key(false, null), mine)
                .commit();
        ClassicLayoutMigration.carryOverOnce(context, store);
        assertFalse(store.contains(PadLayoutKeys.legacyKey(null)));
        assertEquals(mine, store.getString(PadLayoutKeys.key(true, null), null));
        assertEquals(mine, store.getString(PadLayoutKeys.key(false, null), null));
    }

    @Test public void theClassicLayoutFillsOnlyWhatHasNoLayoutOfItsOwn() {
        classic.edit().putString("13", RIGHT_STICK).commit();
        String landscape = "v1|FACE,0.7083,0.5556,1.300,100,0", game = "v1|DPAD,0.2,0.6,1.000,100,0";
        store.edit().putString(PadLayoutKeys.key(true, null), landscape)
                .putString(PadLayoutKeys.key(true, "pc-1/app-7"), game).commit();
        ClassicLayoutMigration.carryOverOnce(context, store);
        // Portrait had no layout for all games; landscape and the game keep theirs, and they win where they exist.
        assertTrue(shared().spec(PadLayout.Control.RIGHT_STICK).placed());
        assertEquals(landscape, store.getString(PadLayoutKeys.key(true, null), null));
        assertEquals(game, store.getString(PadLayoutKeys.key(true, "pc-1/app-7"), null));
    }

    @Test public void aPlayerWhoPickedThisControllerOrResetTheLayoutGetsNothingClassic() {
        classic.edit().putString("13", RIGHT_STICK).commit();
        ClassicLayoutMigration.skip(context);
        ClassicLayoutMigration.carryOverOnce(context, store);
        assertFalse(store.contains(PadLayoutKeys.legacyKey(null)));

        // Reset sets the same flag, so the defaults stay defaults.
        store.edit().clear().commit();
        ModernController.clearLayouts(context);
        ClassicLayoutMigration.carryOverOnce(context, store);
        assertFalse(store.contains(PadLayoutKeys.legacyKey(null)));
        assertTrue(store.getBoolean(ClassicLayoutMigration.MIGRATED, false));
    }

    @Test public void aCorruptClassicControlIsLeftOut() {
        classic.edit().putString("12", "not json").putString("1", "{\"LEFT\":5}").putInt("14", 3)
                .putString("13", RIGHT_STICK).commit();
        Map<Integer, float[]> boxes = ClassicLayoutMigration.boxes(classic);
        assertEquals(1, boxes.size());
        assertArrayEquals(new float[] {1200, 400, 200, 200}, boxes.get(13), 0);
        ClassicLayoutMigration.carryOverOnce(context, store);
        assertFalse(shared().spec(PadLayout.Control.LEFT_STICK).placed());
        assertTrue(shared().spec(PadLayout.Control.RIGHT_STICK).placed());
    }

    private static Map<Integer, float[]> boxes() {
        Map<Integer, float[]> boxes = new HashMap<>();
        boxes.put(12, new float[] {100, 100, 200, 200});   // left stick
        boxes.put(13, new float[] {1300, 400, 200, 200}); // right stick
        boxes.put(1, new float[] {700, 400, 200, 200});   // D-pad
        boxes.put(6, new float[] {1400, 400, 80, 80});    // A
        boxes.put(7, new float[] {1480, 400, 80, 80});    // B
        boxes.put(8, new float[] {1400, 480, 80, 80});    // X
        boxes.put(9, new float[] {1480, 480, 80, 80});    // Y
        boxes.put(16, new float[] {760, 100, 80, 80});    // Guide
        boxes.put(25, new float[] {1500, 650, 80, 50});   // right grip
        boxes.put(999, new float[] {10, 10, 10, 10});     // unknown, ignored
        return boxes;
    }

    @Test public void classicControlsLandNearTheirOldPlacesAndSizes() {
        PadLayout layout = ClassicLayoutMigration.migrate(boxes(), WIDTH, HEIGHT, DENSITY);
        PadLayout.Spec stick = layout.spec(PadLayout.Control.LEFT_STICK);
        assertEquals(200f / WIDTH, stick.cx, .001f);
        assertEquals(200f / HEIGHT, stick.cy, .001f);
        // 200 px against the 302 px the modern stick takes at this size.
        assertEquals(200f / (PadLayout.baseWidth(PadLayout.Control.LEFT_STICK) * 720), stick.scale(), .01f);

        PadLayout.Spec dpad = layout.spec(PadLayout.Control.DPAD);
        assertEquals(800f / WIDTH, dpad.cx, .001f);
        assertEquals(500f / HEIGHT, dpad.cy, .001f);

        // The four face buttons become one cluster centered on them, as wide as their outermost edges.
        PadLayout.Spec face = layout.spec(PadLayout.Control.FACE);
        assertEquals(1480f / WIDTH, face.cx, .001f);
        assertEquals(480f / HEIGHT, face.cy, .001f);
        assertEquals(160f / (PadLayout.baseWidth(PadLayout.Control.FACE) * 720), face.scale(), .01f);

        assertFalse("a control the modern default leaves out is shown", layout.spec(PadLayout.Control.HOME).hidden);
        assertFalse(layout.spec(PadLayout.Control.RIGHT_GRIP).hidden);
        // The pointer touchpad did not exist then and keeps the modern default (hidden until added).
        assertTrue(layout.spec(PadLayout.Control.POINTER_PAD).hidden);
    }

    @Test public void anEmptyOrBrokenClassicLayoutLeavesTheModernDefaultsAlone() {
        assertTrue(ClassicLayoutMigration.migrate(null, WIDTH, HEIGHT, DENSITY).isDefault());
        assertTrue(ClassicLayoutMigration.migrate(new HashMap<>(), WIDTH, HEIGHT, DENSITY).isDefault());
        Map<Integer, float[]> broken = new HashMap<>();
        broken.put(12, null);
        broken.put(13, new float[] {0, 0, 0, 0});
        assertTrue(ClassicLayoutMigration.migrate(broken, WIDTH, HEIGHT, DENSITY).isDefault());
        assertTrue(ClassicLayoutMigration.migrate(boxes(), 0, 0, DENSITY).isDefault());
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
        assertNull("The face buttons are one cluster, not four controls", ClassicLayoutMigration.control(6));
        assertNull(ClassicLayoutMigration.control(999));
    }
}
