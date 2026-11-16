package com.limelight.binding.input.pad;

import org.junit.Test;

import static org.junit.Assert.*;

public class PadGlyphsTest {
    @Test public void eachFamilyNamesTheButtonsInItsOwnWayWithoutMovingThem() {
        assertEquals("A", PadGlyphs.face(PadLayout.Glyph.XBOX, 0));
        assertEquals("B", PadGlyphs.face(PadLayout.Glyph.XBOX, 1));
        assertEquals("X", PadGlyphs.face(PadLayout.Glyph.XBOX, 2));
        assertEquals("Y", PadGlyphs.face(PadLayout.Glyph.XBOX, 3));

        assertEquals("\u2715", PadGlyphs.face(PadLayout.Glyph.PLAYSTATION, 0));
        assertEquals("\u25EF", PadGlyphs.face(PadLayout.Glyph.PLAYSTATION, 1));
        assertEquals("\u25A1", PadGlyphs.face(PadLayout.Glyph.PLAYSTATION, 2));
        assertEquals("\u25B3", PadGlyphs.face(PadLayout.Glyph.PLAYSTATION, 3));

        // Nintendo's A is on the right and B at the bottom, where the PC's pad still gets A and B.
        assertEquals("B", PadGlyphs.face(PadLayout.Glyph.NINTENDO, 0));
        assertEquals("A", PadGlyphs.face(PadLayout.Glyph.NINTENDO, 1));
        assertEquals("Y", PadGlyphs.face(PadLayout.Glyph.NINTENDO, 2));
        assertEquals("X", PadGlyphs.face(PadLayout.Glyph.NINTENDO, 3));
    }

    @Test public void shouldersFollowTheFamilyAndOtherPillsKeepTheirName() {
        assertEquals("LB", PadGlyphs.shoulder(PadLayout.Control.LB, PadLayout.Glyph.XBOX));
        assertEquals("RT", PadGlyphs.shoulder(PadLayout.Control.RT, PadLayout.Glyph.XBOX));
        assertEquals("L1", PadGlyphs.shoulder(PadLayout.Control.LB, PadLayout.Glyph.PLAYSTATION));
        assertEquals("R2", PadGlyphs.shoulder(PadLayout.Control.RT, PadLayout.Glyph.PLAYSTATION));
        assertEquals("L", PadGlyphs.shoulder(PadLayout.Control.LB, PadLayout.Glyph.NINTENDO));
        assertEquals("ZR", PadGlyphs.shoulder(PadLayout.Control.RT, PadLayout.Glyph.NINTENDO));
        assertNull(PadGlyphs.shoulder(PadLayout.Control.L4, PadLayout.Glyph.PLAYSTATION));
    }
}
