package com.limelight.binding.input.pad;

import org.junit.Test;

import java.util.HashMap;
import java.util.Map;

import static org.junit.Assert.*;

/** The classic controller's saved boxes become a modern layout once, on the first switch of style. */
public class ClassicLayoutMigrationTest {
    private static final float WIDTH = 1600, HEIGHT = 720, DENSITY = 2;

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
