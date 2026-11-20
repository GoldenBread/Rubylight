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

    @Test
    public void theLogNamesTheChoiceAsDesktopDoes() {
        assertEquals("off", ControllersChoice.key(false, true));
        assertEquals("off", ControllersChoice.key(false, false));
        assertEquals("one_player", ControllersChoice.key(true, false));
        assertEquals("player_each", ControllersChoice.key(true, true));
    }
}
