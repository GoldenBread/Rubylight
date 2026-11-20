package com.limelight.binding.input.pad;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.graphics.Color;

import com.limelight.binding.input.LayoutStick;
import com.limelight.nvstream.input.ControllerPacket;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.GraphicsMode;

/** The look's moving parts: how a press eases back, which parts a press lights, the touchpad trail and the halo. */
@RunWith(RobolectricTestRunner.class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = 30, application = Application.class, qualifiers = "xhdpi")
public class PadPainterTest {
    @Test public void aLetGoPartEasesBackToRestWithinTheReleaseTime() {
        assertEquals(1, PadPainter.easeBack(0), 0);
        assertEquals(1, PadPainter.easeBack(-5), 0);
        assertEquals(0, PadPainter.easeBack(PadPainter.RELEASE_MS), 0);
        assertEquals(0, PadPainter.easeBack(PadPainter.RELEASE_MS + 50), 0);
        float before = 1;
        for (long ms = 10; ms < PadPainter.RELEASE_MS; ms += 10) {
            float level = PadPainter.easeBack(ms);
            assertTrue(ms + " ms: " + level, level < before && level > 0);
            before = level;
        }
        assertTrue("Most of the way back by half the time", PadPainter.easeBack(PadPainter.RELEASE_MS / 2) < .5f);
    }

    @Test public void aPressLightsTheControlsOwnParts() {
        PadPainter.State state = new PadPainter.State();
        state.flags = ControllerPacket.A_FLAG | ControllerPacket.Y_FLAG;
        assertEquals("A at the bottom and Y at the top", 1 | 8, PadPainter.heldParts(PadLayout.Control.FACE, state));
        state.flags = ControllerPacket.UP_FLAG | ControllerPacket.RIGHT_FLAG;
        assertEquals("Up and right", 1 | 8, PadPainter.heldParts(PadLayout.Control.DPAD, state));
        state.flags = LayoutStick.UP | LayoutStick.LEFT;
        assertEquals("W and A", 1 | 4, PadPainter.heldParts(PadLayout.Control.WASD, state));
        state.flags = ControllerPacket.LS_CLK_FLAG;
        assertEquals("A stick's click", 1, PadPainter.heldParts(PadLayout.Control.LEFT_STICK, state));
        assertEquals("but not the other stick's", 0, PadPainter.heldParts(PadLayout.Control.RIGHT_STICK, state));
        state.flags = 0;
        state.pressed = true;
        assertEquals(1, PadPainter.heldParts(PadLayout.Control.START, state));
        assertEquals(1, PadPainter.heldParts(PadLayout.Control.B, state));
        state.pressed = false;
        assertEquals(0, PadPainter.heldParts(PadLayout.Control.START, state));
    }

    @Test public void theTrailKeepsTheFingersLastPlacesAndFades() {
        PadPainter.State state = new PadPainter.State();
        state.trail(1, 1, 1000);
        assertFalse("One place is no trail yet", state.trailShows(1000));
        for (int i = 2; i <= PadPainter.TRAIL + 3; i++) state.trail(i, i, 1000 + i);
        assertEquals(PadPainter.TRAIL, state.trailCount);
        assertEquals("The oldest places gave way", 4, state.trailX[state.trailStart], 0);
        long last = 1000 + PadPainter.TRAIL + 3;
        assertTrue(state.trailShows(last + PadPainter.TRAIL_MS - 1));
        assertFalse("Gone once faded", state.trailShows(last + PadPainter.TRAIL_MS));
        state.clear();
        assertEquals(0, state.trailCount);
    }

    /** The halo reaches past a held button's edge, where a thumb on the button leaves it in view; at rest or at 0 nothing. */
    @Test public void theHaloReachesPastAHeldButtonsEdge() {
        PadPainter painter = new PadPainter(RuntimeEnvironment.getApplication());
        float density = RuntimeEnvironment.getApplication().getResources().getDisplayMetrics().density;
        int size = Math.round(56 * density), margin = Math.round(40 * density);
        PadPainter.State state = new PadPainter.State();
        state.pressed = true;
        state.press[0] = 1;
        int outside = margin + size + Math.round(6 * density), middle = margin + size / 2;
        Bitmap held = halo(painter, state, 1, size, margin);
        assertTrue("A halo 6 dp past the button's box", Color.alpha(held.getPixel(outside, middle)) > 0);
        assertEquals("but not far off", 0, Color.alpha(held.getPixel(margin + size + Math.round(30 * density), middle)));

        state.press[0] = 0;
        state.pressed = false;
        assertEquals("At rest, none", 0, Color.alpha(halo(painter, state, 1, size, margin).getPixel(outside, middle)));
        state.press[0] = 1;
        assertEquals("At opacity 0, none", 0, Color.alpha(halo(painter, state, 0, size, margin).getPixel(outside, middle)));
    }

    /**
     * PlayStation's face shapes are drawn in the labels' one neutral colour, never green, red, blue or pink, so the
     * cluster at rest has no coloured pixel; Xbox's letters keep their colours.
     */
    @Test public void playStationShapesAreNeutralAndXboxLettersKeepTheirColours() {
        PadPainter painter = new PadPainter(RuntimeEnvironment.getApplication());
        painter.setGlyph(PadLayout.Glyph.PLAYSTATION);
        assertEquals("PlayStation", 0, coloured(painter));
        painter.setGlyph(PadLayout.Glyph.XBOX);
        assertTrue("Xbox", coloured(painter) > 0);
    }

    /** How many pixels of a face cluster at rest are clearly coloured rather than grey. */
    private static int coloured(PadPainter painter) {
        int size = 300;
        Bitmap bitmap = Bitmap.createBitmap(size, size, Bitmap.Config.ARGB_8888);
        painter.draw(new Canvas(bitmap), PadLayout.Control.FACE, 0, 0, size, size, new PadPainter.State());
        int[] pixels = new int[size * size];
        bitmap.getPixels(pixels, 0, size, 0, 0, size, size);
        int count = 0;
        for (int pixel : pixels) {
            if (Color.alpha(pixel) < 40) continue;
            int r = Color.red(pixel), g = Color.green(pixel), b = Color.blue(pixel);
            if (Math.max(r, Math.max(g, b)) - Math.min(r, Math.min(g, b)) > 24) count++;
        }
        return count;
    }

    private static Bitmap halo(PadPainter painter, PadPainter.State state, float alpha, int size, int margin) {
        Bitmap bitmap = Bitmap.createBitmap(size + 2 * margin, size + 2 * margin, Bitmap.Config.ARGB_8888);
        painter.drawHalo(new Canvas(bitmap), PadLayout.Control.START, margin, margin, size, size, state, alpha, 0);
        return bitmap;
    }
}
