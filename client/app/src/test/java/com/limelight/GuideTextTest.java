package com.limelight;

import android.app.Application;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;

/** The pairing guide and the Wake-on-LAN help: plain sentences for any device and any Moonlight-compatible host. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class)
public class GuideTextTest {
    private final Application app = RuntimeEnvironment.getApplication();

    private static void plain(String name, String text) {
        // A TV, a handheld or a Shield is "this device"; Nightjar asks for no particular host.
        for (String word : new String[] {"Android", "Rubylight", "Apollo", "Sunshine", "Vibepollo"}) {
            assertFalse(name + " names " + word, text.contains(word));
        }
        for (String line : text.split("\n")) {
            assertFalse(name + " is a numbered list: " + line, line.trim().matches("\\d+\\..*"));
        }
    }

    @Test
    public void thePairingGuideIsNoInstructionListAndNamesNoPlatformOrHost() {
        plain("guide_text", app.getString(R.string.guide_text));
    }

    @Test
    public void theWakeOnLanHelpSaysWhatItDoesAndNeedsInTwoSentences() {
        String help = app.getString(R.string.wol_help_text);
        plain("wol_help_text", help);
        assertEquals(help, 2, help.trim().split("(?<=\\.)\\s+").length);
    }
}
