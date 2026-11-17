package com.limelight.binding.input.pad;

import org.junit.Test;

import static org.junit.Assert.*;

/** The pointer touchpad's movement in host pixels: the stream's size, Trackpad speed, kept fraction and short range. */
public class PointerMoveTest {
    private static final int STREAM_WIDTH = 1920, STREAM_HEIGHT = 1080;

    @Test public void aPhoneMovesThePointerByTheStreamsPixels() {
        PointerMove move = new PointerMove();
        int[] out = new int[2];
        move.setViewport(STREAM_WIDTH, STREAM_HEIGHT, 1600, 900, 100, 1);
        assertTrue(move.move(100, 0, out));
        assertEquals("A 1600 px phone streaming 1920 moves the pointer 120", 120, out[0]);
        assertEquals(0, out[1]);

        move.setViewport(STREAM_WIDTH, STREAM_HEIGHT, 1080, 1920, 100, 1);
        assertTrue(move.move(0, 100, out));
        assertEquals("A 1080 px phone streaming 4K leaks nothing", 0, out[0]);
        assertEquals("and converts the vertical axis too", 56, out[1]);
    }

    @Test public void trackpadSpeedScalesTheMovement() {
        PointerMove normal = new PointerMove(), fast = new PointerMove();
        int[] a = new int[2], b = new int[2];
        normal.setViewport(STREAM_WIDTH, STREAM_HEIGHT, 1600, 900, 100, 1);
        fast.setViewport(STREAM_WIDTH, STREAM_HEIGHT, 1600, 900, 150, 1);
        normal.move(100, 0, a);
        fast.move(100, 0, b);
        assertEquals(120, a[0]);
        assertEquals(180, b[0]);
    }

    @Test public void theFractionOfAHostPixelIsCarried() {
        PointerMove move = new PointerMove();
        int[] out = new int[2];
        // 1 touchpad pixel is 1.2 host pixels: three of them are four host pixels, not three.
        move.setViewport(STREAM_WIDTH, STREAM_HEIGHT, 1600, 900, 100, 1);
        int total = 0;
        for (int i = 0; i < 3; i++) {
            move.move(1, 0, out);
            total += out[0];
        }
        assertEquals(4, total);
    }

    @Test public void aMoveNeverLeavesTheMousesRange() {
        PointerMove move = new PointerMove();
        int[] out = new int[2];
        move.setViewport(STREAM_WIDTH, STREAM_HEIGHT, 1600, 900, 100, 1);
        assertTrue(move.move(1_000_000, 1_000_000, out));
        assertEquals(Short.MAX_VALUE, out[0]);
        assertEquals(Short.MAX_VALUE, out[1]);
        assertTrue(move.move(-1_000_000, -1_000_000, out));
        assertEquals(Short.MIN_VALUE, out[0]);
        assertEquals(Short.MIN_VALUE, out[1]);
    }

    @Test public void anUnsizedViewMovesNothing() {
        PointerMove move = new PointerMove();
        int[] out = new int[2];
        move.setViewport(STREAM_WIDTH, STREAM_HEIGHT, 0, 0, 100, 1);
        assertFalse(move.move(100, 100, out));
        assertEquals(0, out[0]);
        assertEquals(0, out[1]);
    }

    @Test public void aZoomedDesktopViewMovesThePointerOneToOne() {
        PointerMove move = new PointerMove();
        int[] out = new int[2];
        // At 2x zoom a view pixel is half a host pixel: the same drag crosses half the host.
        move.setViewport(STREAM_WIDTH, STREAM_HEIGHT, 1600, 900, 100, 2);
        assertTrue(move.move(100, 0, out));
        assertEquals(60, out[0]);
    }

    @Test public void aMoveThatRoundsToNothingSaysSo() {
        PointerMove move = new PointerMove();
        int[] out = new int[2];
        move.setViewport(STREAM_WIDTH, STREAM_HEIGHT, 1600, 900, 100, 1);
        assertFalse(move.move(0, 0, out));
        assertEquals(0, out[0]);
    }
}
