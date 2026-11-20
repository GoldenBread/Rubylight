package com.limelight.binding.input.pad;

import org.junit.Test;

import static org.junit.Assert.*;

/** Portrait and landscape keep separate layouts, and older one-layout saves still load. */
public class PadLayoutKeysTest {
    @Test public void eachOrientationAndGameHasItsOwnKey() {
        assertEquals("layout:landscape", PadLayoutKeys.key(true, null));
        assertEquals("layout:portrait", PadLayoutKeys.key(false, null));
        assertEquals("layout:landscape:pc-1/app-7", PadLayoutKeys.key(true, "pc-1/app-7"));
        assertEquals("layout:portrait:pc-1/app-7", PadLayoutKeys.key(false, "pc-1/app-7"));
        assertEquals("layout:portrait", PadLayoutKeys.otherOrientationKey(true, null));
        assertEquals("layout:landscape:pc-1/app-7", PadLayoutKeys.otherOrientationKey(false, "pc-1/app-7"));
    }

    @Test public void theOlderLayoutsKeepTheirNames() {
        assertEquals("layout", PadLayoutKeys.legacyKey(null));
        assertEquals("layout:pc-1/app-7", PadLayoutKeys.legacyKey("pc-1/app-7"));
        assertEquals("layout", PadLayoutKeys.legacyKey(""));
        assertTrue(PadLayoutKeys.ours(PadLayoutKeys.legacyKey("pc-1/app-7")));
        assertTrue(PadLayoutKeys.ours("layout:portrait"));
        assertFalse(PadLayoutKeys.ours("classic-migrated"));
        assertFalse(PadLayoutKeys.ours(null));
    }

    /** Where the editor's panel was left is kept per orientation, apart from the layouts. */
    @Test public void theEditorPanelsPlaceHasAKeyPerOrientationThatIsNoLayout() {
        assertEquals("editor-panel:landscape", PadLayoutKeys.panelKey(true));
        assertEquals("editor-panel:portrait", PadLayoutKeys.panelKey(false));
        assertFalse(PadLayoutKeys.ours(PadLayoutKeys.panelKey(true)));
    }

    @Test public void theTwoOrientationsNeverShareAKey() {
        assertNotEquals(PadLayoutKeys.key(true, "game"), PadLayoutKeys.key(false, "game"));
        assertNotEquals(PadLayoutKeys.key(true, null), PadLayoutKeys.key(false, null));
        assertNotEquals(PadLayoutKeys.key(true, "game"), PadLayoutKeys.legacyKey("game"));
    }
}
