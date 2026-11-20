package com.limelight.preferences;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

/** The stream start's notice that Controllers Off keeps the connected controllers on this device. */
public class ControllersChoiceTest {
    @Test
    public void onlyOffWithAControllerConnectedLeavesItHere() {
        assertTrue(ControllersChoice.leavesConnectedHere(false, 1));
        assertTrue(ControllersChoice.leavesConnectedHere(false, 4));
        assertFalse("nothing to leave out", ControllersChoice.leavesConnectedHere(false, 0));
        assertFalse("One player and A player each send every controller", ControllersChoice.leavesConnectedHere(true, 1));
        assertFalse(ControllersChoice.leavesConnectedHere(true, 0));
    }

    /** Once a stream: at its start, else for the first controller connected during it. */
    @Test
    public void theOffNoticeIsSaidOnceAStream() {
        ControllersChoice.OffNotice later = new ControllersChoice.OffNotice(false, false);
        assertFalse("nothing connected at the start", later.take(0));
        assertTrue("the first controller connected during the stream", later.take(1));
        assertFalse("a second one is not said again", later.take(1));

        ControllersChoice.OffNotice atStart = new ControllersChoice.OffNotice(false, false);
        assertTrue(atStart.take(2));
        assertFalse("not again for one connected later", atStart.take(1));

        assertFalse("controllers are sent", new ControllersChoice.OffNotice(true, false).take(1));
        ControllersChoice.OffNotice second = new ControllersChoice.OffNotice(false, true);
        assertFalse("a second screen's controllers belong to the first", second.take(1));
        assertFalse(second.take(1));
    }

    @Test
    public void theLogNamesTheChoiceAsDesktopDoes() {
        assertEquals("off", ControllersChoice.key(false, true));
        assertEquals("off", ControllersChoice.key(false, false));
        assertEquals("one_player", ControllersChoice.key(true, false));
        assertEquals("player_each", ControllersChoice.key(true, true));
    }
}
