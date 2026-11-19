package com.limelight.binding.input.pad;

import org.junit.Test;

import static org.junit.Assert.*;

public class PadLayoutTest {
    private static final float DENSITY = 2.75f;

    @Test public void anUneditedLayoutSavesAsTheVersionAlone() {
        PadLayout layout = new PadLayout();
        assertEquals("v1", layout.serialize());
        assertTrue(PadLayout.parse("v1").isDefault());
        assertTrue("A layout the short-lived v2 build wrote still loads", PadLayout.parse("v2").isDefault());
        assertTrue(PadLayout.parse(null).isDefault());
    }

    @Test public void savedLayoutsComeBackAsTheyWere() {
        PadLayout layout = new PadLayout();
        layout.moveTo(PadLayout.Control.FACE, 1700, 600, 2400, 1080, DENSITY);
        layout.setScale(PadLayout.Control.FACE, 1.3f);
        layout.setOpacity(PadLayout.Control.LB, 40);
        layout.spec(PadLayout.Control.HOME).hidden = true;
        PadLayout back = PadLayout.parse(layout.serialize());
        PadLayout.Spec face = back.spec(PadLayout.Control.FACE);
        assertEquals(layout.spec(PadLayout.Control.FACE).cx, face.cx, 1e-4);
        assertEquals(layout.spec(PadLayout.Control.FACE).cy, face.cy, 1e-4);
        assertEquals(1.3f, face.scale, 1e-3);
        assertEquals(40, back.spec(PadLayout.Control.LB).opacity);
        assertTrue(back.spec(PadLayout.Control.HOME).hidden);
        // Only edited controls are written; the rest follow the default for the screen.
        assertFalse(back.spec(PadLayout.Control.LEFT_STICK).placed());
        assertEquals(layout.serialize(), back.serialize());
    }

    @Test public void aResizedButUnmovedControlKeepsFollowingTheDefaultPlace() {
        PadLayout layout = new PadLayout();
        layout.setScale(PadLayout.Control.DPAD, .8f);
        PadLayout back = PadLayout.parse(layout.serialize());
        assertFalse(back.spec(PadLayout.Control.DPAD).placed());
        assertEquals(.8f, back.spec(PadLayout.Control.DPAD).scale, 1e-3);
    }

    @Test public void brokenOrUnknownEntriesAreSkippedAndTheRestKept() {
        PadLayout layout = PadLayout.parse("v1|PADDLE,0.5,0.5,1,100,0|LB,x,0.2,1,100,0|RB,-,-,1.5,70,0|LT,0.1|"
                + "RT,7,-3,9,500,1");
        assertTrue(layout.spec(PadLayout.Control.LB).isDefault());
        assertEquals(1.5f, layout.spec(PadLayout.Control.RB).scale, 1e-3);
        assertEquals(70, layout.spec(PadLayout.Control.RB).opacity);
        assertTrue(layout.spec(PadLayout.Control.LT).isDefault());
        // Values out of range are clamped, not refused.
        PadLayout.Spec rt = layout.spec(PadLayout.Control.RT);
        assertEquals(1, rt.cx, 0);
        assertEquals(0, rt.cy, 0);
        assertEquals(PadLayout.MAX_SCALE, rt.scale, 0);
        assertEquals(PadLayout.MAX_OPACITY, rt.opacity);
        assertTrue(rt.hidden);
    }

    @Test public void theWasdStickIsLeftOutUntilShownAndThenKept() {
        PadLayout layout = new PadLayout();
        assertTrue("Not in the default layout", layout.spec(PadLayout.Control.WASD).hidden);
        assertTrue(layout.isDefault());
        assertEquals("v1", layout.serialize());
        layout.spec(PadLayout.Control.WASD).hidden = false;
        layout.moveTo(PadLayout.Control.WASD, 600, 700, 2400, 1080, DENSITY);
        layout.setScale(PadLayout.Control.WASD, 1.2f);
        layout.setOpacity(PadLayout.Control.WASD, 60);
        assertFalse(layout.isDefault());
        PadLayout back = PadLayout.parse(layout.serialize());
        PadLayout.Spec wasd = back.spec(PadLayout.Control.WASD);
        assertFalse(wasd.hidden);
        assertEquals(layout.spec(PadLayout.Control.WASD).cx, wasd.cx, 1e-4);
        assertEquals(layout.spec(PadLayout.Control.WASD).cy, wasd.cy, 1e-4);
        assertEquals(1.2f, wasd.scale, 1e-3);
        assertEquals(60, wasd.opacity);
        assertEquals(layout.serialize(), back.serialize());
        // Shown but otherwise untouched is still an edit worth saving.
        PadLayout shown = new PadLayout();
        shown.spec(PadLayout.Control.WASD).hidden = false;
        assertFalse(PadLayout.parse(shown.serialize()).spec(PadLayout.Control.WASD).hidden);
        // Reset leaves it out again; a copy keeps it shown.
        assertFalse(back.copy().spec(PadLayout.Control.WASD).hidden);
        back.reset();
        assertTrue(back.spec(PadLayout.Control.WASD).hidden);
        assertTrue(back.isDefault());
    }

    /** With Show Touchpad button on, the microphone and menu buttons sit below the touchpad; the stick clears both. */
    @Test public void theWasdStickClearsTheMicrophoneAndMenuButtons() {
        // Phones as short as 392 dp, both ways up, and a tablet. A 16:10 or 16:9 phone has no room below the buttons, so
        // there the stick clears the controls instead (theDefaultClustersDoNotOverlapOnPhonesInBothOrientations).
        float[][] surfaces = {{2400, 1078, 2.75f}, {1600, 720, 2}, {2340, 1080, 2.75f}, {1080, 2400, 2.75f}, {2560, 1600, 2}};
        PadLayout layout = new PadLayout();
        layout.spec(PadLayout.Control.WASD).hidden = false;
        float[] box = new float[4], wasd = new float[4];
        for (float[] surface : surfaces) {
            float width = surface[0], height = surface[1], density = surface[2];
            // The top row's bottom as ModernController.topRowBottom finds it, touchpad included.
            float rowBottom = 0;
            for (PadLayout.Control control : new PadLayout.Control[] {PadLayout.Control.SELECT, PadLayout.Control.HOME,
                    PadLayout.Control.START, PadLayout.Control.TOUCHPAD}) {
                layout.bounds(control, width, height, density, box);
                rowBottom = Math.max(rowBottom, box[1] + box[3]);
            }
            layout.bounds(PadLayout.Control.WASD, width, height, density, wasd);
            String where = width + "x" + height;
            assertTrue(where + " stays on the surface", wasd[1] + wasd[3] <= height + .01f);
            float size = 48 * density, margin = 12 * density;
            float[][] buttons = {
                    com.limelight.ui.MicButton.Placement.center(Float.NaN, Float.NaN, width, height, size, margin, rowBottom),
                    com.limelight.ui.MenuButton.Placement.center(Float.NaN, Float.NaN, width, height, size, margin, rowBottom)};
            for (float[] center : buttons) {
                float l = center[0] - size / 2, t = center[1] - size / 2;
                boolean overlap = l < wasd[0] + wasd[2] && wasd[0] < l + size && t < wasd[1] + wasd[3] && wasd[1] < t + size;
                assertFalse("The WASD stick overlaps a button at " + center[0] + "," + center[1] + " on " + where, overlap);
            }
        }
        // A 1080p phone at 420 dpi (411 dp tall) has no room below the buttons: the stick keeps its spot below the
        // touchpad there.
        layout.bounds(PadLayout.Control.TOUCHPAD, 1920, 1080, 2.625f, box);
        layout.bounds(PadLayout.Control.WASD, 1920, 1080, 2.625f, wasd);
        assertTrue(wasd[1] >= box[1] + box[3]);
        assertTrue(wasd[1] < box[1] + box[3] + PadLayout.TOP_BUTTONS_DP * 2.625f);
    }

    @Test public void aLayoutSavedBeforeTheWasdStickLoadsWithoutIt() {
        // As 4.5.0 saved it: a moved face cluster and a hidden Home, no WASD entry.
        PadLayout layout = PadLayout.parse("v1|FACE,0.7083,0.5556,1.300,100,0|HOME,-,-,1.000,100,1");
        assertEquals(.7083f, layout.spec(PadLayout.Control.FACE).cx, 1e-4);
        assertTrue(layout.spec(PadLayout.Control.HOME).hidden);
        assertTrue(layout.spec(PadLayout.Control.WASD).hidden);
        assertTrue(layout.spec(PadLayout.Control.WASD).isDefault());
        // The stick saves as an entry of its own, which an older build skips as an unknown control.
        PadLayout withStick = new PadLayout();
        withStick.spec(PadLayout.Control.WASD).hidden = false;
        String text = withStick.serialize();
        assertEquals("v1|WASD,-,-,1.000,100,0", text);
    }

    @Test public void anotherVersionLoadsAsTheDefault() {
        assertTrue(PadLayout.parse("v3|FACE,0.5,0.5,1,100,0").isDefault());
        assertTrue(PadLayout.parse("").isDefault());
    }

    /**
     * L3: layouts stay in the v1 format so a downgraded build keeps the controls it knows, while the v2 prefix this
     * version briefly wrote still loads.
     */
    @Test public void layoutsStayV1SoADowngradeKeepsThemAndV2StillLoads() {
        PadLayout layout = new PadLayout();
        layout.setGlyph(PadLayout.Glyph.PLAYSTATION);
        layout.setPointerMode(PadLayout.PointerMode.CAMERA);
        layout.spec(PadLayout.Control.WASD).hidden = false;
        layout.setScale(PadLayout.Control.FACE, 1.3f);
        String text = layout.serialize();
        assertTrue("v1, which an older parser accepts", text.startsWith("v1|"));
        // The glyph and mode entries have no comma fields, so the older parser skips them and keeps its controls.
        assertTrue(text.contains("|glyph=playstation"));
        assertTrue(text.contains("|mode=camera"));
        PadLayout back = PadLayout.parse(text);
        assertEquals(PadLayout.Glyph.PLAYSTATION, back.glyph());
        assertEquals(PadLayout.PointerMode.CAMERA, back.pointerMode());
        assertEquals(1.3f, back.spec(PadLayout.Control.FACE).scale, 1e-3);
        // A layout the short-lived v2 build wrote still loads, settings and all.
        PadLayout v2 = PadLayout.parse("v2|FACE,0.7083,0.5556,1.300,100,0|mode=camera");
        assertEquals(1.3f, v2.spec(PadLayout.Control.FACE).scale, 1e-3);
        assertEquals(PadLayout.PointerMode.CAMERA, v2.pointerMode());
    }

    @Test public void everyControlLiesOnTheSurfaceAtEverySize() {
        float[][] surfaces = {{2400, 1080}, {1080, 2400}, {2560, 1600}, {800, 480}};
        float[] box = new float[4];
        for (float[] surface : surfaces) {
            PadLayout layout = new PadLayout();
            for (PadLayout.Control control : PadLayout.Control.values()) layout.setScale(control, PadLayout.MAX_SCALE);
            for (PadLayout.Control control : PadLayout.Control.values()) {
                layout.bounds(control, surface[0], surface[1], DENSITY, box);
                assertTrue(control + " left", box[0] >= 0);
                assertTrue(control + " top", box[1] >= 0);
                assertTrue(control + " right", box[0] + box[2] <= surface[0] + .01f);
                assertTrue(control + " bottom", box[1] + box[3] <= surface[1] + .01f);
            }
        }
    }

    /**
     * Two controls the default layout leaves out (or the Steam setting gates) may take the same place when both are
     * shown, so the editor moves one rather than the default layout having to fit every combination.
     */
    private static boolean bothOptional(PadLayout.Control a, PadLayout.Control b) {
        return (a.hiddenByDefault || a.steam) && (b.hiddenByDefault || b.steam);
    }

    @Test public void theDefaultClustersDoNotOverlapOnPhonesInBothOrientations() {
        float[][] surfaces = {{2400, 1080}, {1080, 2400}, {1920, 1200}};
        PadLayout layout = new PadLayout();
        float[] a = new float[4], b = new float[4];
        PadLayout.Control[] controls = PadLayout.Control.values();
        for (float[] surface : surfaces) {
            for (int i = 0; i < controls.length; i++) {
                for (int j = i + 1; j < controls.length; j++) {
                    layout.bounds(controls[i], surface[0], surface[1], DENSITY, a);
                    layout.bounds(controls[j], surface[0], surface[1], DENSITY, b);
                    if (bothOptional(controls[i], controls[j])) continue;
                    boolean overlap = a[0] < b[0] + b[2] && b[0] < a[0] + a[2] && a[1] < b[1] + b[3] && b[1] < a[1] + a[3];
                    assertFalse(controls[i] + " overlaps " + controls[j] + " on " + surface[0] + "x" + surface[1], overlap);
                }
            }
        }
    }

    /** Small phones (800x360 dp at 2x, either way up) and the smallest scale: every control still takes a 48 dp touch, apart. */
    @Test public void everyDefaultControlTakesAFullTouchOnASmallPhone() {
        float density = 2;
        float[][] surfaces = {{1600, 720}, {720, 1600}, {1280, 800}};
        PadLayout.Control[] controls = PadLayout.Control.values();
        float[] a = new float[4], b = new float[4];
        for (float[] surface : surfaces) {
            PadLayout layout = new PadLayout();
            for (PadLayout.Control control : controls) {
                layout.bounds(control, surface[0], surface[1], density, a);
                String where = control + " on " + surface[0] + "x" + surface[1];
                assertTrue(where + " is " + a[2] / density + " dp wide", a[2] >= 48 * density - .01f);
                assertTrue(where + " is " + a[3] / density + " dp tall", a[3] >= 48 * density - .01f);
                if (control.shape == PadLayout.Shape.PILL) assertTrue(where + " pill", a[2] >= 72 * density - .01f);
                for (PadLayout.Control other : controls) {
                    if (other.ordinal() <= control.ordinal() || bothOptional(control, other)) continue;
                    layout.bounds(other, surface[0], surface[1], density, b);
                    boolean overlap = a[0] < b[0] + b[2] && b[0] < a[0] + a[2] && a[1] < b[1] + b[3] && b[1] < a[1] + a[3];
                    assertFalse(where + " overlaps " + other, overlap);
                }
            }
            layout.setScale(PadLayout.Control.HOME, PadLayout.MIN_SCALE);
            layout.bounds(PadLayout.Control.HOME, surface[0], surface[1], density, a);
            assertTrue("Shrinking stops at a full touch", a[2] >= 48 * density - .01f);
        }
    }

    @Test public void aTabletsControlsStopGrowingAtTheUnitCap() {
        float tablet = PadLayout.unit(2560, 1600, 2);
        assertEquals(PadLayout.MAX_UNIT_DP * 2, tablet, 0);
        assertEquals(1080, PadLayout.unit(2400, 1080, DENSITY), 0);
        // Portrait: narrow enough that the two sides fit next to each other.
        assertEquals(1080 / 1.75f, PadLayout.unit(1080, 2400, DENSITY), .01f);
    }

    @Test public void movingKeepsTheWholeControlOnTheSurface() {
        PadLayout layout = new PadLayout();
        layout.moveTo(PadLayout.Control.FACE, -500, 5000, 2400, 1080, DENSITY);
        float[] box = layout.bounds(PadLayout.Control.FACE, 2400, 1080, DENSITY, new float[4]);
        assertEquals(0, box[0], .5f);
        assertEquals(1080 - box[3], box[1], .5f);
        // The saved center is a fraction, so it lands in the same place after a rotation back and forth.
        PadLayout.Spec spec = layout.spec(PadLayout.Control.FACE);
        assertTrue(spec.cx > 0 && spec.cx < 1 && spec.cy > 0 && spec.cy < 1);
    }

    @Test public void resetDropsEveryEdit() {
        PadLayout layout = new PadLayout();
        layout.setScale(PadLayout.Control.LB, 2);
        layout.spec(PadLayout.Control.RB).hidden = true;
        PadLayout copy = layout.copy();
        layout.reset();
        assertTrue(layout.isDefault());
        assertFalse("A copy is its own", copy.isDefault());
    }

    @Test public void glyphsAndTheTouchpadModeAreSavedAndMigrateFromV1() {
        PadLayout layout = new PadLayout();
        layout.setGlyph(PadLayout.Glyph.PLAYSTATION);
        layout.setPointerMode(PadLayout.PointerMode.CAMERA);
        assertEquals("v1|glyph=playstation|mode=camera", layout.serialize());
        PadLayout back = PadLayout.parse(layout.serialize());
        assertEquals(PadLayout.Glyph.PLAYSTATION, back.glyph());
        assertEquals(PadLayout.PointerMode.CAMERA, back.pointerMode());
        assertFalse(back.isDefault());
        back.reset();
        assertEquals(PadLayout.Glyph.XBOX, back.glyph());
        assertEquals(PadLayout.PointerMode.MOUSE, back.pointerMode());
        assertTrue(back.isDefault());

        // A layout from the version before these fields: Xbox glyphs and a mouse touchpad.
        PadLayout old = PadLayout.parse("v1|FACE,0.7083,0.5556,1.300,100,0");
        assertEquals(PadLayout.Glyph.XBOX, old.glyph());
        assertEquals(PadLayout.PointerMode.MOUSE, old.pointerMode());
        assertEquals(1.3f, old.spec(PadLayout.Control.FACE).scale, 1e-3);
        // An unknown glyph or mode reads as the default rather than dropping the whole layout.
        PadLayout unknown = PadLayout.parse("v2|glyph=atari|mode=laser|LB,-,-,1,100,0");
        assertEquals(PadLayout.Glyph.XBOX, unknown.glyph());
        assertEquals(PadLayout.PointerMode.MOUSE, unknown.pointerMode());
        assertEquals(PadLayout.MAX_OPACITY, unknown.spec(PadLayout.Control.LB).opacity);
    }

    @Test public void presetsShowTheControlsTheyName() {
        PadLayout minimal = new PadLayout();
        minimal.applyPreset(PadLayout.Preset.MINIMAL);
        for (PadLayout.Control control : PadLayout.Control.values()) {
            boolean wanted = control == PadLayout.Control.LEFT_STICK || control == PadLayout.Control.RIGHT_STICK ||
                    control == PadLayout.Control.DPAD || control == PadLayout.Control.FACE ||
                    control == PadLayout.Control.SELECT || control == PadLayout.Control.START;
            if (!control.steam) assertEquals(control.name(), !wanted, minimal.spec(control).hidden);
        }
        // The pointer touchpad and the WASD stick are opt-in controls; Minimal leaves both out.
        assertTrue(minimal.spec(PadLayout.Control.POINTER_PAD).hidden);
        assertTrue(minimal.spec(PadLayout.Control.WASD).hidden);

        PadLayout full = new PadLayout();
        full.applyPreset(PadLayout.Preset.FULL);
        for (PadLayout.Control control : PadLayout.Control.values()) {
            if (control.steam) continue;
            // Every gamepad control and the pointer touchpad show; the WASD stick is a keyboard control and stays opt-in.
            assertEquals(control.name(), control == PadLayout.Control.WASD, full.spec(control).hidden);
        }
        assertFalse(full.spec(PadLayout.Control.POINTER_PAD).hidden);

        PadLayout back = new PadLayout();
        back.applyPreset(PadLayout.Preset.FULL);
        back.applyPreset(PadLayout.Preset.DEFAULT);
        assertTrue(back.isDefault());
        assertEquals("v1", back.serialize());
    }

    @Test public void aPresetLeavesTheSteamControlsAsTheyAre() {
        PadLayout.Control steam = null;
        for (PadLayout.Control control : PadLayout.Control.values()) {
            if (control.steam) { steam = control; break; }
        }
        assertNotNull(steam);
        for (PadLayout.Preset preset : PadLayout.Preset.values()) {
            PadLayout layout = new PadLayout();
            layout.spec(steam).cx = 120;
            layout.spec(steam).cy = 340;
            layout.setScale(steam, 1.5f);
            layout.setOpacity(steam, 60);
            layout.spec(steam).hidden = !layout.spec(steam).defaultHidden;
            layout.spec(PadLayout.Control.FACE).scale = 1.4f;
            layout.applyPreset(preset);
            PadLayout.Spec kept = layout.spec(steam);
            String name = preset + " " + steam;
            assertEquals(name, 120f, kept.cx, 1e-3);
            assertEquals(name, 340f, kept.cy, 1e-3);
            assertEquals(name, 1.5f, kept.scale, 1e-3);
            assertEquals(name, 60, kept.opacity);
            assertEquals(name, !kept.defaultHidden, kept.hidden);
            assertEquals("the other controls take the preset", 1f, layout.spec(PadLayout.Control.FACE).scale, 1e-3);
        }
    }

    /** L2: a control preset changes which controls show, not the glyph family or what the pointer touchpad does. */
    @Test public void aPresetKeepsTheGlyphsAndTheTouchpadMode() {
        PadLayout layout = new PadLayout();
        layout.setGlyph(PadLayout.Glyph.PLAYSTATION);
        layout.setPointerMode(PadLayout.PointerMode.CAMERA);
        layout.applyPreset(PadLayout.Preset.MINIMAL);
        assertEquals(PadLayout.Glyph.PLAYSTATION, layout.glyph());
        assertEquals(PadLayout.PointerMode.CAMERA, layout.pointerMode());
        layout.applyPreset(PadLayout.Preset.FULL);
        assertEquals(PadLayout.Glyph.PLAYSTATION, layout.glyph());
        assertEquals(PadLayout.PointerMode.CAMERA, layout.pointerMode());
        layout.applyPreset(PadLayout.Preset.DEFAULT);
        assertEquals(PadLayout.Glyph.PLAYSTATION, layout.glyph());
        assertEquals(PadLayout.PointerMode.CAMERA, layout.pointerMode());
    }
}
