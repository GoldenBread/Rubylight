package com.limelight.binding.input.pad;

import org.junit.Test;

import java.util.Map;

import java.util.HashMap;
import com.limelight.nvstream.input.ControllerPacket;

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
        assertEquals(1.3f, face.scale(), 1e-3);
        assertEquals(40, back.spec(PadLayout.Control.LB).opacity);
        assertTrue(back.spec(PadLayout.Control.HOME).hidden);
        // Only edited controls are written; the rest follow the default for the screen.
        assertFalse(back.spec(PadLayout.Control.LEFT_STICK).placed());
        assertEquals(layout.serialize(), back.serialize());
    }

    /** A layout saved before free resizing has one scale per control: it loads as that size in both directions. */
    @Test public void aLayoutSavedBeforeFreeResizingLoadsWithItsShape() {
        PadLayout old = PadLayout.parse("v1|FACE,0.7083,0.5556,1.300,100,0|LT,-,-,0.700,80,0");
        PadLayout.Spec face = old.spec(PadLayout.Control.FACE);
        assertEquals(1.3f, face.widthScale, 1e-6);
        assertEquals(1.3f, face.heightScale, 1e-6);
        assertEquals(.7f, old.spec(PadLayout.Control.LT).widthScale, 1e-6);
        assertEquals(.7f, old.spec(PadLayout.Control.LT).heightScale, 1e-6);
        // Saved again, it is written exactly as before: six fields, no width and height of its own.
        assertEquals("v1|FACE,0.7083,0.5556,1.300,100,0|LT,-,-,0.700,80,0", old.serialize());
        float[] box = old.bounds(PadLayout.Control.FACE, 2400, 1080, DENSITY, new float[4]);
        assertEquals("Square, as it always was", box[2], box[3], 0);
    }

    /**
     * A width and a height of their own come back as they were. They follow the six fields an older build reads, and
     * the one scale it reads is the larger, clamped to its own 0.5 to 2, so it shows a sensible size.
     */
    @Test public void independentWidthAndHeightComeBackAndAnOlderBuildReadsSixFields() {
        PadLayout layout = new PadLayout();
        layout.setSize(PadLayout.Control.FACE, 2.5f, 1.2f);
        layout.setSize(PadLayout.Control.LB, .6f, 1.4f);
        String text = layout.serialize();
        assertEquals("v1|FACE,-,-,2.500,100,0,2.500,1.200|LB,-,-,1.400,100,0,0.600,1.400", text);
        PadLayout back = PadLayout.parse(text);
        assertEquals(2.5f, back.spec(PadLayout.Control.FACE).widthScale, 1e-6);
        assertEquals(1.2f, back.spec(PadLayout.Control.FACE).heightScale, 1e-6);
        assertEquals(.6f, back.spec(PadLayout.Control.LB).widthScale, 1e-6);
        assertEquals(1.4f, back.spec(PadLayout.Control.LB).heightScale, 1e-6);
        assertEquals(text, back.serialize());

        // How the build before this one reads each entry: the first six fields, the scale clamped to 0.5 to 2.
        for (String part : text.split("\\|")) {
            String[] fields = part.split(",", -1);
            if (fields.length < 6) continue;
            float olderScale = Math.max(.5f, Math.min(2f, Float.parseFloat(fields[3])));
            PadLayout.Spec spec = back.spec(PadLayout.Control.valueOf(fields[0]));
            assertEquals(part, Math.min(2f, spec.scale()), olderScale, 1e-6);
            assertEquals(part, String.valueOf(spec.opacity), fields[4]);
            assertEquals(part, spec.hidden ? "1" : "0", fields[5]);
        }

        // A broken width or height falls back to the one scale rather than dropping the entry.
        PadLayout broken = PadLayout.parse("v1|RB,-,-,1.500,70,0,x,0.800|RT,-,-,1.200,100,0,1.100");
        assertEquals(1.5f, broken.spec(PadLayout.Control.RB).widthScale, 1e-6);
        assertEquals(1.5f, broken.spec(PadLayout.Control.RB).heightScale, 1e-6);
        assertEquals(70, broken.spec(PadLayout.Control.RB).opacity);
        assertEquals("Seven fields: the one scale", 1.2f, broken.spec(PadLayout.Control.RT).heightScale, 1e-6);
    }

    /**
     * A control can be sized up to the whole surface in each direction, and no further: what was sized on a larger
     * surface fits again. The default unit's tablet cap sets the default sizes only.
     */
    @Test public void aControlGrowsToTheWholeSurfaceAndNoFurther() {
        float[][] surfaces = {{2400, 1080}, {1080, 2400}, {2560, 1600}};
        float[] box = new float[4];
        for (float[] surface : surfaces) {
            for (PadLayout.Control control : PadLayout.Control.values()) {
                PadLayout layout = new PadLayout();
                layout.setBounds(control, 0, 0, surface[0], surface[1], surface[0], surface[1], DENSITY);
                layout.bounds(control, surface[0], surface[1], DENSITY, box);
                String where = control + " on " + surface[0] + "x" + surface[1];
                assertEquals(where, 0, box[0], .5f);
                assertEquals(where, 0, box[1], .5f);
                assertEquals(where, surface[0], box[2], .5f);
                assertEquals(where, surface[1], box[3], .5f);
                // Saved and loaded, it still fills the surface.
                PadLayout.parse(layout.serialize()).bounds(control, surface[0], surface[1], DENSITY, box);
                assertEquals(where + " saved", surface[0], box[2], 1.5f);
                assertEquals(where + " saved", surface[1], box[3], 1.5f);
                // Sized past the surface (on a larger one), each side stops at the surface.
                layout.setSize(control, PadLayout.MAX_SCALE, PadLayout.MAX_SCALE);
                layout.bounds(control, surface[0], surface[1], DENSITY, box);
                assertEquals(where + " past it", surface[0], box[2], .5f);
                assertEquals(where + " past it", surface[1], box[3], .5f);
            }
        }

        // Growing in both directions keeps the shape and stops where the first side meets the surface.
        PadLayout layout = new PadLayout();
        layout.setSize(PadLayout.Control.FACE, 1.5f, 1f);
        layout.scaleFrom(PadLayout.Control.FACE, 1.5f, 1f, 1000, 2400, 1080, DENSITY);
        layout.bounds(PadLayout.Control.FACE, 2400, 1080, DENSITY, box);
        PadLayout.Spec face = layout.spec(PadLayout.Control.FACE);
        assertEquals("Its shape kept", 1.5f, face.widthScale / face.heightScale, 1e-4);
        assertEquals("as tall as the surface", 1080, box[3], .5f);
        assertTrue(box[2] < 2400);
        // A tablet's control grows past the default unit's cap, up to the whole surface.
        PadLayout tablet = new PadLayout();
        tablet.scaleFrom(PadLayout.Control.LEFT_STICK, 1, 1, 1000, 2560, 1600, 2);
        assertEquals(1600, tablet.bounds(PadLayout.Control.LEFT_STICK, 2560, 1600, 2, box)[3], .5f);
        // Shrinking stops at the smallest scale.
        layout.scaleFrom(PadLayout.Control.FACE, 1, 2, .01f, 2400, 1080, DENSITY);
        assertEquals(PadLayout.MIN_SCALE, face.widthScale, 1e-6);
        assertEquals(2 * PadLayout.MIN_SCALE, face.heightScale, 1e-6);
    }

    /** Every control, the split members too, holds its own slot of the on-screen gamepad report. */
    @Test public void everyControlHasAGamepadSlotOfItsOwn() {
        assertTrue(PadLayout.Control.values().length <= com.limelight.binding.input.TouchLayout.MAX_BUTTONS);
    }

    /** A layout from before the split members, and an untouched one, keep the face cluster and the D-pad whole. */
    @Test public void anOldLayoutLoadsWithTheClustersWholeAndTheMembersHidden() {
        for (PadLayout layout : new PadLayout[] {new PadLayout(), PadLayout.parse("v1|FACE,0.7083,0.5556,1.300,100,0")}) {
            for (PadLayout.Control cluster : new PadLayout.Control[] {PadLayout.Control.FACE, PadLayout.Control.DPAD}) {
                assertTrue(layout.shown(cluster));
                assertFalse(layout.isSplit(cluster));
                for (PadLayout.Control member : PadLayout.members(cluster)) {
                    assertEquals(cluster, member.cluster());
                    assertTrue(member.name(), member.hiddenByDefault);
                    assertTrue(member.name(), layout.spec(member).hidden);
                    assertFalse(member.name(), layout.shown(member));
                }
            }
        }
        assertEquals("Nothing new is written for them", "v1", new PadLayout().serialize());
        assertEquals(0, PadLayout.members(PadLayout.Control.LB).length);
        assertNull(PadLayout.Control.LB.cluster());
    }

    /**
     * Split puts each member in its button's direction from the cluster's centre, at its size, spread outward until no
     * two overlap; the cluster hides, and the cluster and its members never show together. Join brings the cluster
     * back as it was.
     */
    @Test public void splitMembersKeepTheClustersArrangementAndJoinBringsTheClusterBack() {
        float width = 2400, height = 1080;
        PadLayout layout = new PadLayout();
        layout.moveTo(PadLayout.Control.FACE, 1800, 600, width, height, DENSITY);
        layout.setSize(PadLayout.Control.FACE, 1.2f, 1f);
        layout.setOpacity(PadLayout.Control.FACE, 60);
        // Nothing else shown, so no neighbour moves the members off the cluster's centre.
        for (PadLayout.Control other : PadLayout.Control.values()) {
            if (other != PadLayout.Control.FACE) layout.spec(other).hidden = true;
        }
        float[] face = layout.bounds(PadLayout.Control.FACE, width, height, DENSITY, new float[4]);
        layout.split(PadLayout.Control.FACE, width, height, DENSITY);
        assertTrue(layout.spec(PadLayout.Control.FACE).hidden);
        assertTrue(layout.isSplit(PadLayout.Control.FACE));
        float[] box = new float[4];
        float diameter = 2 * PadInput.FACE_RADIUS * Math.min(face[2], face[3]);
        float midX = face[0] + face[2] / 2, midY = face[1] + face[3] / 2;
        for (PadLayout.Control member : PadLayout.members(PadLayout.Control.FACE)) {
            assertTrue(layout.shown(member));
            layout.bounds(member, width, height, DENSITY, box);
            float[] center = PadInput.FACE_CENTERS[member.memberIndex()];
            // Its button's direction from the middle, at least its whole size and two pixels out, so none overlap.
            float dx = (center[0] - .5f) * face[2], dy = (center[1] - .5f) * face[3];
            float wantX = midX + (dx == 0 ? 0 : Math.signum(dx) * Math.max(Math.abs(dx), diameter + 2));
            float wantY = midY + (dy == 0 ? 0 : Math.signum(dy) * Math.max(Math.abs(dy), diameter + 2));
            assertEquals(member.name(), wantX, box[0] + box[2] / 2, .5f);
            assertEquals(member.name(), wantY, box[1] + box[3] / 2, .5f);
            assertEquals(member.name(), diameter, box[2], .5f);
            assertEquals(member.name(), diameter, box[3], .5f);
            assertEquals("The cluster's opacity", 60, layout.spec(member).opacity);
        }
        // A cluster shown again hides its members, whatever their own setting says.
        layout.spec(PadLayout.Control.FACE).hidden = false;
        assertFalse(layout.shown(PadLayout.Control.A));
        layout.spec(PadLayout.Control.FACE).hidden = true;

        layout.join(PadLayout.Control.FACE, width, height, DENSITY);
        assertTrue(layout.shown(PadLayout.Control.FACE));
        for (PadLayout.Control member : PadLayout.members(PadLayout.Control.FACE)) assertTrue(layout.spec(member).hidden);
        assertArrayEquals("Where and as it was", face, layout.bounds(PadLayout.Control.FACE, width, height, DENSITY,
                new float[4]), .01f);

        // The D-pad's arms reach as far as their pressed amber; neighbours overlap at the inner corners, so a finger
        // there holds the diagonal, and the middle is in none.
        PadLayout dpad = new PadLayout();
        float[] pad = dpad.bounds(PadLayout.Control.DPAD, width, height, DENSITY, new float[4]);
        dpad.split(PadLayout.Control.DPAD, width, height, DENSITY);
        float[] up = dpad.bounds(PadLayout.Control.DPAD_UP, width, height, DENSITY, new float[4]);
        float[] right = dpad.bounds(PadLayout.Control.DPAD_RIGHT, width, height, DENSITY, new float[4]);
        float[] down = dpad.bounds(PadLayout.Control.DPAD_DOWN, width, height, DENSITY, new float[4]);
        float[] left = dpad.bounds(PadLayout.Control.DPAD_LEFT, width, height, DENSITY, new float[4]);
        float padX = pad[0] + pad[2] / 2, padY = pad[1] + pad[3] / 2;
        assertEquals("Up sits over the top arm", padX, up[0] + up[2] / 2, .5f);
        assertTrue(up[1] + up[3] / 2 < padY && down[1] + down[3] / 2 > padY);
        assertTrue(left[0] + left[2] / 2 < padX && right[0] + right[2] / 2 > padX);
        assertTrue("Up and right overlap", overlap(up, right));
        assertTrue("Down and left overlap", overlap(down, left));
        assertFalse("Opposite arms never overlap", overlap(up, down) || overlap(left, right));
        for (float[] arm : new float[][] {up, right, down, left}) {
            assertFalse("The middle holds nothing", padX > arm[0] && padX < arm[0] + arm[2] && padY > arm[1] && padY < arm[1] + arm[3]);
            assertTrue("A full touch", Math.min(arm[2], arm[3]) >= PadLayout.MIN_TOUCH_DP * DENSITY - .01f);
        }

        // A preset keeps the clusters whole.
        dpad.applyPreset(PadLayout.Preset.FULL);
        assertTrue(dpad.shown(PadLayout.Control.DPAD));
        assertFalse(dpad.shown(PadLayout.Control.DPAD_UP));
    }

    /** The preview test's surfaces and the Galaxy Z Fold7's inner screen both ways: width, height and density. */
    private static final float[][] SURFACES = {{1600, 720, 2}, {720, 1600, 2}, {1280, 800, 1}, {2184, 1968, 2.625f},
            {1968, 2184, 2.625f}};

    /**
     * No two split members overlap, so one finger never presses two, on every surface above at the default size and the
     * smallest: face buttons share no pixel, also once placed in whole pixels; D-pad neighbours share only their inner
     * corner, which holds the diagonal, and opposite arms nothing. Every member stays on the surface and off the
     * default layout's other controls.
     */
    @Test public void splitMembersNeverOverlapAndStayOnTheSurface() {
        for (float[] surface : SURFACES) {
            float width = surface[0], height = surface[1], density = surface[2];
            for (float scale : new float[] {1, PadLayout.MIN_SCALE}) {
                PadLayout layout = new PadLayout();
                for (PadLayout.Control cluster : new PadLayout.Control[] {PadLayout.Control.FACE, PadLayout.Control.DPAD}) {
                    layout.setSize(cluster, scale, scale);
                    layout.split(cluster, width, height, density);
                    PadLayout.Control[] members = PadLayout.members(cluster);
                    float[][] boxes = new float[members.length][];
                    for (int i = 0; i < members.length; i++) {
                        boxes[i] = layout.bounds(members[i], width, height, density, new float[4]);
                        float[] b = boxes[i];
                        String where = members[i] + " at " + width + "x" + height + ", scale " + scale;
                        assertTrue(where + " on the surface", b[0] >= -.01f && b[1] >= -.01f && b[0] + b[2] <= width + .01f
                                && b[1] + b[3] <= height + .01f);
                    }
                    // Nor over a neighbour: moved together off one, where spreading carried a member onto it.
                    for (int i = 0; i < members.length; i++) {
                        for (PadLayout.Control other : PadLayout.Control.values()) {
                            if (other == cluster || other.cluster() == cluster || other.steam || !layout.shown(other)) continue;
                            assertFalse(members[i] + " over " + other + " at " + width + "x" + height + ", scale " + scale,
                                    overlap(boxes[i], layout.bounds(other, width, height, density, new float[4])));
                        }
                    }
                    float[] up = layout.bounds(PadLayout.Control.DPAD_UP, width, height, density, new float[4]);
                    float corner = PadInput.DPAD_CORNER / PadInput.DPAD_ARM * up[2] + 1;
                    for (int i = 0; i < members.length; i++) {
                        for (int j = i + 1; j < members.length; j++) {
                            String pair = members[i] + " and " + members[j] + " at " + width + "x" + height + ", scale " + scale;
                            boolean opposite = opposite(members[i], members[j]);
                            if (cluster == PadLayout.Control.FACE || opposite) {
                                assertFalse(pair + " overlap", overlap(boxes[i], boxes[j]));
                                assertFalse(pair + " overlap in whole pixels", overlap(pixels(boxes[i]), pixels(boxes[j])));
                            } else {
                                assertTrue(pair + " share the diagonal's corner", overlap(boxes[i], boxes[j]));
                                float across = Math.min(boxes[i][0] + boxes[i][2], boxes[j][0] + boxes[j][2])
                                        - Math.max(boxes[i][0], boxes[j][0]);
                                float down = Math.min(boxes[i][1] + boxes[i][3], boxes[j][1] + boxes[j][3])
                                        - Math.max(boxes[i][1], boxes[j][1]);
                                assertTrue(pair + " share only the corner: " + across + "x" + down,
                                        across <= corner && down <= corner);
                            }
                        }
                    }
                }
            }
        }
    }

    private static boolean opposite(PadLayout.Control a, PadLayout.Control b) {
        java.util.List<PadLayout.Control> pair = java.util.Arrays.asList(a, b);
        return pair.containsAll(java.util.Arrays.asList(PadLayout.Control.A, PadLayout.Control.Y))
                || pair.containsAll(java.util.Arrays.asList(PadLayout.Control.B, PadLayout.Control.X))
                || pair.containsAll(java.util.Arrays.asList(PadLayout.Control.DPAD_UP, PadLayout.Control.DPAD_DOWN))
                || pair.containsAll(java.util.Arrays.asList(PadLayout.Control.DPAD_LEFT, PadLayout.Control.DPAD_RIGHT));
    }

    /** The box a view gets from a float box: whole pixels, as the controller places it. */
    private static float[] pixels(float[] box) {
        return new float[] {Math.round(box[0]), Math.round(box[1]), Math.round(box[2]), Math.round(box[3])};
    }

    /**
     * The pointer touchpad's default place is clear of the stream's microphone and menu buttons, which sit below the
     * top row, and of every control of the default layout, on every surface above; where there is room it keeps the
     * middle of the surface.
     */
    @Test public void thePointerTouchpadsDefaultIsClearOfTheMicrophoneAndMenuButtons() {
        for (float[] surface : SURFACES) {
            float width = surface[0], height = surface[1], density = surface[2];
            String where = width + "x" + height;
            PadLayout layout = new PadLayout();
            float[] pad = layout.bounds(PadLayout.Control.POINTER_PAD, width, height, density, new float[4]);
            assertTrue(where + " on the surface", pad[0] >= 0 && pad[1] >= 0 && pad[0] + pad[2] <= width + .01f
                    && pad[1] + pad[3] <= height + .01f);
            // Where the controller puts the buttons: centred below the top row's bottom, the menu button beside it.
            float rowBottom = 0;
            for (PadLayout.Control row : new PadLayout.Control[] {PadLayout.Control.SELECT, PadLayout.Control.HOME,
                    PadLayout.Control.START, PadLayout.Control.TOUCHPAD}) {
                float[] box = layout.bounds(row, width, height, density, new float[4]);
                rowBottom = Math.max(rowBottom, box[1] + box[3]);
            }
            float size = com.limelight.ui.MicButton.SIZE_DP * density, margin = 12 * density;
            float[] mic = com.limelight.ui.MicButton.Placement.center(Float.NaN, Float.NaN, width, height, size, margin,
                    rowBottom);
            float[] menu = com.limelight.ui.MenuButton.Placement.center(Float.NaN, Float.NaN, width, height, size, margin,
                    rowBottom);
            for (float[] button : new float[][] {mic, menu}) {
                float[] box = {button[0] - size / 2, button[1] - size / 2, size, size};
                assertFalse(where + ": under a stream button", overlap(pad, box));
            }
            for (PadLayout.Control control : PadLayout.Control.values()) {
                if (control == PadLayout.Control.POINTER_PAD || control.steam || !layout.shown(control)) continue;
                float[] box = layout.bounds(control, width, height, density, new float[4]);
                assertFalse(where + ": over " + control, overlap(pad, box));
            }
            if (height / 2 - pad[3] / 2 > rowBottom + (PadLayout.TOP_BUTTONS_DP + 8) * density) {
                assertEquals(where + ": the middle where there is room", height / 2, pad[1] + pad[3] / 2, .5f);
            }
        }
    }

    private static boolean overlap(float[] a, float[] b) {
        return a[0] < b[0] + b[2] && b[0] < a[0] + a[2] && a[1] < b[1] + b[3] && b[1] < a[1] + a[3];
    }

    /**
     * A split layout comes back as it was. Its members' entries are of the six-field kind every version reads, so an
     * older build skips their unknown names and keeps the rest of the layout.
     */
    @Test public void aSplitLayoutSurvivesSaveAndLoad() {
        PadLayout layout = new PadLayout();
        layout.split(PadLayout.Control.FACE, 2400, 1080, DENSITY);
        layout.split(PadLayout.Control.DPAD, 2400, 1080, DENSITY);
        layout.moveTo(PadLayout.Control.Y, 1200, 300, 2400, 1080, DENSITY);
        layout.spec(PadLayout.Control.X).hidden = true;
        String text = layout.serialize();
        PadLayout back = PadLayout.parse(text);
        assertEquals(text, back.serialize());
        for (PadLayout.Control control : PadLayout.Control.values()) {
            PadLayout.Spec a = layout.spec(control), b = back.spec(control);
            assertEquals(control.name(), a.hidden, b.hidden);
            assertEquals(control.name(), a.cx, b.cx, 1e-4);
            assertEquals(control.name(), a.cy, b.cy, 1e-4);
            assertEquals(control.name(), a.widthScale, b.widthScale, 1e-3);
            assertEquals(control.name(), a.heightScale, b.heightScale, 1e-3);
        }
        assertTrue(back.isSplit(PadLayout.Control.FACE));
        assertFalse("X hidden", back.shown(PadLayout.Control.X));
        assertTrue(back.shown(PadLayout.Control.Y));
        for (String part : text.split("\\|")) {
            String[] fields = part.split(",", -1);
            if (fields.length < 6 || PadLayout.Control.valueOf(fields[0]).cluster() == null) continue;
            Float.parseFloat(fields[3]);
            Integer.parseInt(fields[4]);
        }
    }

    @Test public void aResizedButUnmovedControlKeepsFollowingTheDefaultPlace() {
        PadLayout layout = new PadLayout();
        layout.setScale(PadLayout.Control.DPAD, .8f);
        PadLayout back = PadLayout.parse(layout.serialize());
        assertFalse(back.spec(PadLayout.Control.DPAD).placed());
        assertEquals(.8f, back.spec(PadLayout.Control.DPAD).scale(), 1e-3);
    }

    @Test public void brokenOrUnknownEntriesAreSkippedAndTheRestKept() {
        PadLayout layout = PadLayout.parse("v1|PADDLE,0.5,0.5,1,100,0|LB,x,0.2,1,100,0|RB,-,-,1.5,70,0|LT,0.1|"
                + "RT,7,-3,900,500,1");
        assertTrue(layout.spec(PadLayout.Control.LB).isDefault());
        assertEquals(1.5f, layout.spec(PadLayout.Control.RB).scale(), 1e-3);
        assertEquals(70, layout.spec(PadLayout.Control.RB).opacity);
        assertTrue(layout.spec(PadLayout.Control.LT).isDefault());
        // Values out of range are clamped, not refused.
        PadLayout.Spec rt = layout.spec(PadLayout.Control.RT);
        assertEquals(1, rt.cx, 0);
        assertEquals(0, rt.cy, 0);
        assertEquals(PadLayout.MAX_SCALE, rt.scale(), 0);
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
        assertEquals(1.2f, wasd.scale(), 1e-3);
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
        assertEquals(1.3f, back.spec(PadLayout.Control.FACE).scale(), 1e-3);
        // A layout the short-lived v2 build wrote still loads, settings and all.
        PadLayout v2 = PadLayout.parse("v2|FACE,0.7083,0.5556,1.300,100,0|mode=camera");
        assertEquals(1.3f, v2.spec(PadLayout.Control.FACE).scale(), 1e-3);
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
     * shown, so the editor moves one rather than the default layout having to fit every combination. A split
     * cluster's members show only in their cluster's place, over its buttons; splitMembersSitOverTheClustersButtons
     * checks where.
     */
    private static boolean bothOptional(PadLayout.Control a, PadLayout.Control b) {
        if (a.cluster() != null || b.cluster() != null) return true;
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
        assertEquals(1.3f, old.spec(PadLayout.Control.FACE).scale(), 1e-3);
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
            // Every gamepad control and the pointer touchpad show; the WASD stick is a keyboard control and stays
            // opt-in, and the face cluster and the D-pad stay whole.
            assertEquals(control.name(), control == PadLayout.Control.WASD || control.cluster() != null,
                    full.spec(control).hidden);
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
            layout.setScale(PadLayout.Control.FACE, 1.4f);
            layout.applyPreset(preset);
            PadLayout.Spec kept = layout.spec(steam);
            String name = preset + " " + steam;
            assertEquals(name, 120f, kept.cx, 1e-3);
            assertEquals(name, 340f, kept.cy, 1e-3);
            assertEquals(name, 1.5f, kept.scale(), 1e-3);
            assertEquals(name, 60, kept.opacity);
            assertEquals(name, !kept.defaultHidden, kept.hidden);
            assertEquals("the other controls take the preset", 1f, layout.spec(PadLayout.Control.FACE).scale(), 1e-3);
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

    @Test public void aTouchpadAddedBeforeItsPlaceMovedStaysInTheMiddle() {
        float width = 2400, height = 1080;
        float[] box = new float[4];
        // The Add touchpad button of an older build showed it at its default place, then the middle, and never
        // placed it; the layout it saved has the touchpad shown and unplaced.
        PadLayout old = PadLayout.parse("v1|POINTER_PAD,-,-,1.000,100,0");
        old.bounds(PadLayout.Control.POINTER_PAD, width, height, DENSITY, box);
        assertEquals(width / 2, box[0] + box[2] / 2, 1);
        assertEquals(height / 2, box[1] + box[3] / 2, 1);
        // Saved again, it keeps that place.
        PadLayout again = PadLayout.parse(old.serialize());
        again.bounds(PadLayout.Control.POINTER_PAD, width, height, DENSITY, box);
        assertEquals(height / 2, box[1] + box[3] / 2, 1);

        // A touchpad added now shows at the new default place, below the microphone and menu buttons, and keeps it
        // through a save; an older build ignores the marker.
        PadLayout added = new PadLayout();
        added.spec(PadLayout.Control.POINTER_PAD).hidden = false;
        float[] fresh = added.bounds(PadLayout.Control.POINTER_PAD, width, height, DENSITY, new float[4]);
        assertTrue("the new place is below the middle", fresh[1] + fresh[3] / 2 > height / 2 + 1);
        String text = added.serialize();
        assertEquals("v1|pad=below|POINTER_PAD,-,-,1.000,100,0", text);
        PadLayout loaded = PadLayout.parse(text);
        assertFalse(loaded.spec(PadLayout.Control.POINTER_PAD).placed());
        assertArrayEquals(fresh, loaded.bounds(PadLayout.Control.POINTER_PAD, width, height, DENSITY, box), .01f);

        // The Full preset shows it too, at the new place.
        PadLayout full = new PadLayout();
        full.applyPreset(PadLayout.Preset.FULL);
        assertFalse(PadLayout.parse(full.serialize()).spec(PadLayout.Control.POINTER_PAD).placed());

        // An older layout with the touchpad left out shows it at the new place once it is added.
        PadLayout hidden = PadLayout.parse("v1|FACE,0.7083,0.5556,1.300,100,0");
        assertTrue(hidden.spec(PadLayout.Control.POINTER_PAD).hidden);
        assertFalse(hidden.spec(PadLayout.Control.POINTER_PAD).placed());
        assertEquals("v1", new PadLayout().serialize());
    }

    private static float[] span(PadLayout layout, PadLayout.Control cluster, float width, float height) {
        float left = Float.MAX_VALUE, top = Float.MAX_VALUE, right = -Float.MAX_VALUE, bottom = -Float.MAX_VALUE;
        for (PadLayout.Control member : PadLayout.members(cluster)) {
            if (!layout.shown(member)) continue;
            float[] box = layout.bounds(member, width, height, DENSITY, new float[4]);
            left = Math.min(left, box[0]);
            top = Math.min(top, box[1]);
            right = Math.max(right, box[0] + box[2]);
            bottom = Math.max(bottom, box[1] + box[3]);
        }
        return new float[] {left, top, right - left, bottom - top};
    }

    /** Moves the shown members of {@code cluster} together by {@code dx}, {@code dy} pixels. */
    private static void moveMembers(PadLayout layout, PadLayout.Control cluster, float dx, float dy, float width,
                                    float height) {
        for (PadLayout.Control member : PadLayout.members(cluster)) {
            if (!layout.shown(member)) continue;
            float[] box = layout.bounds(member, width, height, DENSITY, new float[4]);
            layout.moveTo(member, box[0] + box[2] / 2 + dx, box[1] + box[3] / 2 + dy, width, height, DENSITY);
        }
    }

    /** A layout with only {@code cluster} shown, so a Split meets no neighbour to move its members off. */
    private static PadLayout alone(PadLayout.Control cluster) {
        PadLayout layout = new PadLayout();
        for (PadLayout.Control control : PadLayout.Control.values()) {
            if (control != cluster && control.cluster() != cluster) layout.spec(control).hidden = true;
        }
        return layout;
    }

    @Test public void joinPutsTheClusterWhereItsMembersStand() {
        float width = 2400, height = 1080;
        for (PadLayout.Control cluster : new PadLayout.Control[] {PadLayout.Control.FACE, PadLayout.Control.DPAD}) {
            PadLayout layout = alone(cluster);
            float[] home = layout.bounds(cluster, width, height, DENSITY, new float[4]);
            layout.split(cluster, width, height, DENSITY);
            float[] member = layout.bounds(PadLayout.members(cluster)[0], width, height, DENSITY, new float[4]);
            // The player drags the four, together, towards the middle of the screen.
            float dx = width / 2 - home[0] - home[2] / 2, dy = height / 2 - home[1] - home[3] / 2;
            moveMembers(layout, cluster, dx, dy, width, height);
            float[] members = span(layout, cluster, width, height);
            layout.join(cluster, width, height, DENSITY);
            float[] joined = layout.bounds(cluster, width, height, DENSITY, new float[4]);
            // The cluster from which Split lays them out as they stand: its own size, in the middle of the four.
            assertEquals(cluster + ": where they are", members[0] + members[2] / 2, joined[0] + joined[2] / 2, 1);
            assertEquals(cluster.name(), members[1] + members[3] / 2, joined[1] + joined[3] / 2, 1);
            assertTrue(cluster + ": not back home", Math.abs(joined[0] - home[0]) > 100);
            assertEquals(cluster + ": its size", home[2], joined[2], home[2] * .01f);
            assertEquals(cluster.name(), home[3], joined[3], home[3] * .01f);

            // Split lays them out around it as they stood; joined again unmoved, it is exactly as it was.
            layout.split(cluster, width, height, DENSITY);
            float[] split = layout.bounds(PadLayout.members(cluster)[0], width, height, DENSITY, new float[4]);
            assertEquals(cluster + ": the member where it stood", member[0] + dx, split[0], 1);
            assertEquals(cluster.name(), member[1] + dy, split[1], 1);
            assertEquals(cluster.name(), member[2], split[2], .5f);
            layout.join(cluster, width, height, DENSITY);
            assertArrayEquals(cluster + ": round trip", joined, layout.bounds(cluster, width, height, DENSITY,
                    new float[4]), .01f);
        }
    }

    @Test public void moveJoinSplitCyclesKeepTheSize() {
        float width = 2400, height = 1080;
        for (PadLayout.Control cluster : new PadLayout.Control[] {PadLayout.Control.FACE, PadLayout.Control.DPAD}) {
            // Alone and in the middle, so no move carries a member off the screen or onto a neighbour.
            PadLayout layout = alone(cluster);
            float[] start = layout.bounds(cluster, width, height, DENSITY, new float[4]);
            layout.split(cluster, width, height, DENSITY);
            moveMembers(layout, cluster, width / 2 - start[0] - start[2] / 2, height / 2 - start[1] - start[3] / 2, width,
                    height);
            float[] memberStart = layout.bounds(PadLayout.members(cluster)[0], width, height, DENSITY, new float[4]);
            float[][] moves = {{-300, 0}, {0, -120}, {250, 80}, {-40, 60}, {180, -90}, {-90, 30}};
            for (int cycle = 0; cycle < moves.length; cycle++) {
                moveMembers(layout, cluster, moves[cycle][0], moves[cycle][1], width, height);
                layout.join(cluster, width, height, DENSITY);
                float[] joined = layout.bounds(cluster, width, height, DENSITY, new float[4]);
                assertEquals(cluster + " cycle " + cycle, start[2], joined[2], start[2] * .01f);
                assertEquals(cluster + " cycle " + cycle, start[3], joined[3], start[3] * .01f);
                layout.split(cluster, width, height, DENSITY);
                float[] member = layout.bounds(PadLayout.members(cluster)[0], width, height, DENSITY, new float[4]);
                assertEquals(cluster + " member, cycle " + cycle, memberStart[2], member[2], memberStart[2] * .01f);
                assertEquals(cluster + " member, cycle " + cycle, memberStart[3], member[3], memberStart[3] * .01f);
            }
        }
    }

    @Test public void oneMemberLeftJoinsAClusterOfFullSizeAroundIt() {
        float width = 2400, height = 1080;
        for (PadLayout.Control cluster : new PadLayout.Control[] {PadLayout.Control.FACE, PadLayout.Control.DPAD}) {
            PadLayout layout = alone(cluster);
            float[] home = layout.bounds(cluster, width, height, DENSITY, new float[4]);
            layout.split(cluster, width, height, DENSITY);
            PadLayout.Control[] members = PadLayout.members(cluster);
            for (int i = 1; i < members.length; i++) layout.spec(members[i]).hidden = true;
            // Moved to the middle, where splitting it again meets no neighbour.
            float dx = width / 2 - home[0] - home[2] / 2, dy = height / 2 - home[1] - home[3] / 2;
            moveMembers(layout, cluster, dx, dy, width, height);
            float[] kept = layout.bounds(members[0], width, height, DENSITY, new float[4]);
            layout.join(cluster, width, height, DENSITY);
            float[] joined = layout.bounds(cluster, width, height, DENSITY, new float[4]);
            // Not a cluster the size of one button: the size the members came from, where they put its middle.
            assertEquals(cluster + ": its size", home[2], joined[2], home[2] * .01f);
            assertEquals(cluster.name(), home[3], joined[3], home[3] * .01f);
            layout.split(cluster, width, height, DENSITY);
            float[] back = layout.bounds(members[0], width, height, DENSITY, new float[4]);
            assertEquals(cluster + ": the member where it stood", kept[0], back[0], 1);
            assertEquals(cluster.name(), kept[1], back[1], 1);
        }
    }

    /** The face members split alone in the middle of a 2400 x 1080 surface, each then {@code button} pixels wide. */
    private static PadLayout faceMembersInTheMiddle(float button) {
        float width = 2400, height = 1080;
        PadLayout layout = alone(PadLayout.Control.FACE);
        float[] home = layout.bounds(PadLayout.Control.FACE, width, height, DENSITY, new float[4]);
        layout.split(PadLayout.Control.FACE, width, height, DENSITY);
        moveMembers(layout, PadLayout.Control.FACE, width / 2 - home[0] - home[2] / 2, height / 2 - home[1] - home[3] / 2,
                width, height);
        for (PadLayout.Control member : MEMBERS_OF_FACE) {
            float[] box = layout.bounds(member, width, height, DENSITY, new float[4]);
            float cx = box[0] + box[2] / 2, cy = box[1] + box[3] / 2;
            layout.setBounds(member, cx - button / 2, cy - button / 2, button, button, width, height, DENSITY);
        }
        return layout;
    }

    private static float[][] memberBoxes(PadLayout layout) {
        float[][] boxes = new float[MEMBERS_OF_FACE.length][];
        for (int i = 0; i < boxes.length; i++) boxes[i] = layout.bounds(MEMBERS_OF_FACE[i], 2400, 1080, DENSITY, new float[4]);
        return boxes;
    }

    @Test public void shrunkMembersInADiamondJoinASquareClusterAndSplitBackWhereTheyStood() {
        float width = 2400, height = 1080;
        float[][] places = {{.5f, .78f}, {.78f, .5f}, {.22f, .5f}, {.5f, .22f}};
        // A little smaller than Split made them (164 px here): the cluster draws each button where its member stood,
        // square.
        PadLayout layout = faceMembersInTheMiddle(156);
        float[][] before = memberBoxes(layout);
        layout.join(PadLayout.Control.FACE, width, height, DENSITY);
        float[] face = layout.bounds(PadLayout.Control.FACE, width, height, DENSITY, new float[4]);
        assertEquals("square", face[2], face[3], 1);
        for (int i = 0; i < places.length; i++) {
            assertEquals(MEMBERS_OF_FACE[i] + " x", before[i][0] + before[i][2] / 2, face[0] + places[i][0] * face[2], 2);
            assertEquals(MEMBERS_OF_FACE[i] + " y", before[i][1] + before[i][3] / 2, face[1] + places[i][1] * face[3], 2);
        }
        layout.split(PadLayout.Control.FACE, width, height, DENSITY);
        for (int i = 0; i < before.length; i++) {
            assertArrayEquals(MEMBERS_OF_FACE[i].name(), before[i], memberBoxes(layout)[i], 2);
        }
        layout.join(PadLayout.Control.FACE, width, height, DENSITY);
        assertArrayEquals(face, layout.bounds(PadLayout.Control.FACE, width, height, DENSITY, new float[4]), .5f);

        // Shrunk to a full touch, drawing them where they stood would make the buttons more than half again their
        // size: the square their size gives, centred on them, and Split puts them back.
        layout = faceMembersInTheMiddle(132);
        before = memberBoxes(layout);
        layout.join(PadLayout.Control.FACE, width, height, DENSITY);
        face = layout.bounds(PadLayout.Control.FACE, width, height, DENSITY, new float[4]);
        assertEquals(132 / (2 * PadInput.FACE_RADIUS), face[2], 1);
        assertEquals(face[2], face[3], 1);
        assertEquals(width / 2, face[0] + face[2] / 2, 2);
        layout.split(PadLayout.Control.FACE, width, height, DENSITY);
        for (int i = 0; i < before.length; i++) {
            assertArrayEquals(MEMBERS_OF_FACE[i].name(), before[i], memberBoxes(layout)[i], 2);
        }
    }

    @Test public void aPairALittleCloserThanSplitSpreadsItStillSetsItsSide() {
        float width = 2400, height = 1080, button = 150, least = button + 2, half = .97f * least;
        PadLayout layout = faceMembersInTheMiddle(button);
        // A bottom, B right, X left, Y top, each pair 0.97 of the way Split spreads members this size.
        float[][] centers = {{1200, 540 + half}, {1200 + half, 540}, {1200 - half, 540}, {1200, 540 - half}};
        for (int i = 0; i < centers.length; i++) {
            layout.moveTo(MEMBERS_OF_FACE[i], centers[i][0], centers[i][1], width, height, DENSITY);
        }
        layout.join(PadLayout.Control.FACE, width, height, DENSITY);
        float[] face = layout.bounds(PadLayout.Control.FACE, width, height, DENSITY, new float[4]);
        float reach = .28f;
        assertEquals("the width from B and X", 2 * half / (2 * reach), face[2], 1);
        assertEquals("the height from A and Y", 2 * half / (2 * reach), face[3], 1);
        assertEquals("B drawn where it stood", 1200 + half, face[0] + .78f * face[2], 2);
        assertEquals("A drawn where it stood", 540 + half, face[1] + .78f * face[3], 2);
    }

    @Test public void heapedMirroredOrFarApartMembersJoinButtonsOfTheirSize() {
        float width = 2400, height = 1080, touch = 48 * DENSITY;
        float[][][] arrangements = {
                // All four heaped on one spot.
                {{1200, 540}, {1200, 540}, {1200, 540}, {1200, 540}},
                // Mirrored: A at the top, B on the left, X on the right, Y at the bottom.
                {{1200, 300}, {960, 540}, {1440, 540}, {1200, 780}},
                // Spread to the four edges of the screen.
                {{1200, 1014}, {2334, 540}, {66, 540}, {1200, 66}},
        };
        for (float[][] centers : arrangements) {
            PadLayout layout = faceMembersInTheMiddle(132);
            for (int i = 0; i < centers.length; i++) {
                layout.moveTo(MEMBERS_OF_FACE[i], centers[i][0], centers[i][1], width, height, DENSITY);
            }
            layout.join(PadLayout.Control.FACE, width, height, DENSITY);
            float[] face = layout.bounds(PadLayout.Control.FACE, width, height, DENSITY, new float[4]);
            float button = 2 * PadInput.FACE_RADIUS * Math.min(face[2], face[3]);
            assertTrue("at least a full touch: " + button, button >= touch - .5f);
            assertTrue("at most half again the members': " + button, button <= 1.5f * 132 + .5f);
            assertTrue(face[2] > 0 && face[3] > 0);
        }
    }

    @Test public void aSplitRightAfterJoinTakesTheClustersOpacityAndKeepsOffNeighbours() {
        float width = 2400, height = 1080;
        PadLayout layout = faceMembersInTheMiddle(156);
        float[][] before = memberBoxes(layout);
        layout.join(PadLayout.Control.FACE, width, height, DENSITY);
        layout.setOpacity(PadLayout.Control.FACE, 40);
        layout.split(PadLayout.Control.FACE, width, height, DENSITY);
        for (int i = 0; i < before.length; i++) {
            assertArrayEquals("back where it stood", before[i], memberBoxes(layout)[i], 2);
            assertEquals("as faint as the cluster", 40, layout.spec(MEMBERS_OF_FACE[i]).opacity);
        }

        // RB moved onto where B stood while they were joined: Split keeps the members off it.
        layout.join(PadLayout.Control.FACE, width, height, DENSITY);
        layout.spec(PadLayout.Control.RB).hidden = false;
        layout.moveTo(PadLayout.Control.RB, before[1][0] + before[1][2] / 2, before[1][1] + before[1][3] / 2, width,
                height, DENSITY);
        float[] rb = layout.bounds(PadLayout.Control.RB, width, height, DENSITY, new float[4]);
        layout.split(PadLayout.Control.FACE, width, height, DENSITY);
        for (float[] box : memberBoxes(layout)) {
            boolean over = box[0] < rb[0] + rb[2] && rb[0] < box[0] + box[2] && box[1] < rb[1] + rb[3]
                    && rb[1] < box[1] + box[3];
            assertFalse("a member over RB", over);
        }
    }

    @Test public void membersThatStandAlikeInBothArrangementsKeepTheOneTheClusterHad() {
        float[][] ax = {{.5f, .78f}, null, {.22f, .5f}, null};
        float[][] by = {null, {.78f, .5f}, null, {.5f, .22f}};
        float[][] one = {{.5f, .78f}, null, null, null};
        for (boolean current : new boolean[] {false, true}) {
            assertEquals(current, PadLayout.arrangement(ax, current));
            assertEquals(current, PadLayout.arrangement(by, current));
            assertEquals(current, PadLayout.arrangement(one, current));
            // All four in the middle tell nothing either.
            float[][] heap = {{.5f, .5f}, {.5f, .5f}, {.5f, .5f}, {.5f, .5f}};
            assertEquals(current, PadLayout.arrangement(heap, current));
        }
        // Three are enough: A, B and X standard, or B, A and Y swapped.
        assertFalse(PadLayout.arrangement(new float[][] {{.5f, .78f}, {.78f, .5f}, {.22f, .5f}, null}, true));
        assertTrue(PadLayout.arrangement(new float[][] {{.78f, .5f}, {.5f, .78f}, null, {.22f, .5f}}, false));
    }

    @Test public void aPresetOrAddShowsTheClusterInTheArrangementItsMembersStoodIn() {
        float width = 2400, height = 1080;
        for (PadLayout.Preset preset : PadLayout.Preset.values()) {
            // Swapped members carried over before the arrangement was saved: no token.
            PadLayout layout = PadLayout.parse(ClassicLayoutMigration.swappedDefaults(width, height, DENSITY)
                    .serialize().replace("|faces=swapped", ""));
            assertFalse(layout.facesSwapped());
            layout.applyPreset(preset);
            assertTrue(preset + ": B at the bottom", layout.facesSwapped());
        }
        // All four hidden, then a preset, or the cluster added back: from where they were.
        for (PadLayout.Preset preset : PadLayout.Preset.values()) {
            PadLayout hidden = PadLayout.parse(ClassicLayoutMigration.swappedDefaults(width, height, DENSITY)
                    .serialize().replace("|faces=swapped", ""));
            for (PadLayout.Control member : MEMBERS_OF_FACE) hidden.spec(member).hidden = true;
            assertFalse(hidden.isSplit(PadLayout.Control.FACE));
            hidden.applyPreset(preset);
            assertTrue(preset + ", all four hidden: B at the bottom", hidden.facesSwapped());
        }
        PadLayout layout = PadLayout.parse(ClassicLayoutMigration.swappedDefaults(width, height, DENSITY)
                .serialize().replace("|faces=swapped", ""));
        for (PadLayout.Control member : MEMBERS_OF_FACE) layout.spec(member).hidden = true;
        layout.takeFaceArrangement();
        assertTrue(layout.facesSwapped());
        // Standard members leave a standard cluster standard.
        PadLayout standard = new PadLayout();
        standard.split(PadLayout.Control.FACE, width, height, DENSITY);
        standard.applyPreset(PadLayout.Preset.FULL);
        assertFalse(standard.facesSwapped());
    }

    private static final PadLayout.Control[] MEMBERS_OF_FACE = PadLayout.members(PadLayout.Control.FACE);

    @Test public void aClusterJoinedFromSwappedMembersKeepsTheirButtons() {
        float width = 2400, height = 1080;
        PadLayout layout = ClassicLayoutMigration.swappedDefaults(width, height, DENSITY);
        float[] bottom = layout.bounds(PadLayout.Control.B, width, height, DENSITY, new float[4]);
        layout.join(PadLayout.Control.FACE, width, height, DENSITY);
        assertTrue(layout.facesSwapped());
        // The bottom of the cluster sends B, its right A, its left Y and its top X.
        float[] face = layout.bounds(PadLayout.Control.FACE, width, height, DENSITY, new float[4]);
        float[][] places = {{.5f, .78f}, {.78f, .5f}, {.22f, .5f}, {.5f, .22f}};
        int[] sent = {ControllerPacket.B_FLAG, ControllerPacket.A_FLAG, ControllerPacket.Y_FLAG, ControllerPacket.X_FLAG};
        for (int i = 0; i < places.length; i++) {
            assertEquals(sent[i], PadInput.face(places[i][0] * face[2], places[i][1] * face[3], face[2], face[3], true));
        }
        assertEquals(ControllerPacket.A_FLAG, PadInput.face(.5f * face[2], .78f * face[3], face[2], face[3]));

        // Saved and loaded it stays so, and Split brings B back to the bottom.
        String text = layout.serialize();
        assertTrue(text, text.contains("|faces=swapped"));
        PadLayout loaded = PadLayout.parse(text);
        assertTrue(loaded.facesSwapped());
        loaded.split(PadLayout.Control.FACE, width, height, DENSITY);
        float[] b = loaded.bounds(PadLayout.Control.B, width, height, DENSITY, new float[4]);
        float[] a = loaded.bounds(PadLayout.Control.A, width, height, DENSITY, new float[4]);
        assertTrue("B at the bottom", b[1] > a[1] + 1 && b[1] > loaded.bounds(PadLayout.Control.Y, width, height,
                DENSITY, new float[4])[1]);
        assertTrue("A on the right", a[0] > b[0] + 1);
        assertTrue(loaded.facesSwapped());
        // Joined again, still swapped; members in the standard places join standard.
        loaded.join(PadLayout.Control.FACE, width, height, DENSITY);
        assertTrue(loaded.facesSwapped());
        PadLayout standard = new PadLayout();
        standard.split(PadLayout.Control.FACE, width, height, DENSITY);
        standard.join(PadLayout.Control.FACE, width, height, DENSITY);
        assertFalse(standard.facesSwapped());
        assertTrue("B where the bottom was", Math.abs(bottom[1] - b[1]) < face[3]);

        // A copy, as the editor's draft, keeps it.
        assertTrue(loaded.copy().facesSwapped());
        // A preset keeps the arrangement, as it keeps the glyphs; Reset gives the standard one.
        loaded.applyPreset(PadLayout.Preset.MINIMAL);
        assertTrue(loaded.facesSwapped());
        loaded.reset();
        assertFalse(loaded.facesSwapped());
        assertTrue(loaded.isDefault());
    }

    @Test public void theSwappedArrangementLeavesOlderLayoutsAndBuildsAlone() {
        // Saved before it existed: the standard arrangement.
        assertFalse(PadLayout.parse("v1|FACE,0.7083,0.5556,1.300,100,0").facesSwapped());
        assertFalse(PadLayout.parse("v1|FACE,-,-,2.500,100,0,2.500,1.200").facesSwapped());
        String text = "v1|faces=swapped|FACE,0.7083,0.5556,1.300,100,0|LT,-,-,0.700,80,0";
        PadLayout swapped = PadLayout.parse(text);
        assertTrue(swapped.facesSwapped());
        assertEquals(.7083f, swapped.spec(PadLayout.Control.FACE).cx, .0001f);
        assertEquals("v1|faces=swapped", swappedOnly().serialize());

        // The first build with this format (40569fe2e) reads it, skipping the token and loading every entry.
        Map<String, float[]> old = parseAsTheFirstBuild(text);
        assertEquals(2, old.size());
        assertArrayEquals(new float[] {.7083f, .5556f, 1.3f, 100, 0}, old.get("FACE"), .0001f);
        assertArrayEquals(new float[] {Float.NaN, Float.NaN, .7f, 80, 0}, old.get("LT"), .0001f);
        // The same build reads the whole layout this build saves with the arrangement.
        PadLayout saved = PadLayout.parse("v1|FACE,0.7000,0.5000,1.200,100,0");
        saved.setFacesSwapped(true);
        Map<String, float[]> read = parseAsTheFirstBuild(saved.serialize());
        assertTrue(saved.serialize(), read.containsKey("FACE"));
        assertFalse(read.containsKey("faces=swapped"));
    }

    /**
     * The parse loop of the build that introduced this format (40569fe2e, PadLayout.parse), copied as it was:
     * each entry as {cx, cy, scale, opacity, hidden} under its control's name.
     */
    private static Map<String, float[]> parseAsTheFirstBuild(String text) {
        Map<String, float[]> layout = new HashMap<>();
        String[] parts = text.split("\\|");
        if (parts.length == 0 || !"v1".equals(parts[0])) return layout;
        for (int i = 1; i < parts.length; i++) {
            String[] fields = parts[i].split(",", -1);
            if (fields.length < 6) continue;
            if (PadLayout.Control.named(fields[0]) == null) continue;
            try {
                float cx = Float.NaN, cy = Float.NaN;
                if (!"-".equals(fields[1]) && !"-".equals(fields[2])) {
                    cx = Float.parseFloat(fields[1]);
                    cy = Float.parseFloat(fields[2]);
                }
                layout.put(fields[0], new float[] {cx, cy, Float.parseFloat(fields[3]), Integer.parseInt(fields[4]),
                        "1".equals(fields[5]) ? 1 : 0});
            } catch (NumberFormatException ignored) {
                // Skip this entry and keep the rest.
            }
        }
        return layout;
    }

    private static PadLayout swappedOnly() {
        PadLayout layout = new PadLayout();
        layout.setFacesSwapped(true);
        return layout;
    }
}
