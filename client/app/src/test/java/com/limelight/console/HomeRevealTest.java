package com.limelight.console;

import static com.limelight.nativeapi.NativeApiFixtures.game;
import static com.limelight.nativeapi.NativeApiFixtures.library;
import static com.limelight.nativeapi.NativeApiFixtures.nativeHost;
import static com.limelight.nativeapi.NativeApiFixtures.ok;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.content.Context;

import androidx.appcompat.view.ContextThemeWrapper;

import com.limelight.R;
import com.limelight.nativeapi.NativeApiFixtures;

import org.junit.After;
import org.junit.Before;
import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

import java.util.Collections;
import java.util.HashSet;

/**
 * Where bringing a card in starts the TV's grid: the section's title shows above a card in its first row, while a card
 * further down a long section comes in itself, under an empty title's height, so it can take focus (the release
 * review's Library-longer-than-a-screen focus loss).
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class HomeRevealTest {
    @Rule public TemporaryFolder temporary = new TemporaryFolder();

    private ConsoleRig rig;
    private ContextThemeWrapper context;

    @Before
    public void setUp() throws Exception {
        rig = new ConsoleRig(temporary.newFolder());
        context = new ContextThemeWrapper(RuntimeEnvironment.getApplication(), R.style.AppTheme);
        String[] games = new String[24];
        for (int i = 0; i < games.length; i++) {
            games[i] = game("t-g" + i, i + 1, "Game " + i, "steam:" + (400 + i), false);
        }
        rig.transport.reply("tower", "library", ok(library("t1", games)));
        rig.transport.reply("tower", "session", ok(NativeApiFixtures.IDLE));
    }

    @After
    public void tearDown() {
        rig.close();
    }

    private ConsoleHomeListAdapter adapter(boolean television) throws Exception {
        ConsoleHomeListAdapter adapter =
                new ConsoleHomeListAdapter(context, null, uuid -> null, null, television);
        adapter.submit(HomeModel.of(rig.repository.refresh(Collections.singletonList(nativeHost("tower", "Tower", true))),
                new HashSet<>(Collections.singletonList("tower"))));
        return adapter;
    }

    @Test
    public void theTitleShowsAboveACardOnlyWhileTheCardStaysOnScreen() {
        // A TV: a 120 px title and a 300 px row of posters in a 1080 px list.
        assertTrue("the first row under the title", ConsoleHomeListAdapter.titleKeepsCardOnScreen(1, 120, 300, 1080));
        assertTrue("two rows still fit", ConsoleHomeListAdapter.titleKeepsCardOnScreen(2, 120, 300, 1080));
        assertFalse("four rows do not", ConsoleHomeListAdapter.titleKeepsCardOnScreen(4, 120, 300, 1080));
        assertFalse("not measured yet", ConsoleHomeListAdapter.titleKeepsCardOnScreen(1, 120, 0, 1080));
        assertFalse("no viewport yet", ConsoleHomeListAdapter.titleKeepsCardOnScreen(1, 120, 300, 0));
    }

    @Test
    public void aDeepCardComesInItselfUnderAnEmptyTitleAndAPhoneGoesToTheCard() throws Exception {
        ConsoleHomeListAdapter adapter = adapter(true);
        int first = adapter.firstCard();
        int heading = adapter.sectionStart(first);
        int deep = first + 13;
        assertEquals("one Library section", heading, adapter.sectionStart(deep));

        ConsoleHomeListAdapter.Reveal top = adapter.reveal(first, 1, 120, 300, 1080, true);
        assertEquals("the title above the first row", heading, top.position);
        assertEquals(0, top.offset);

        ConsoleHomeListAdapter.Reveal down = adapter.reveal(deep, 4, 120, 300, 1080, true);
        assertEquals("the card itself, so it can take focus", deep, down.position);
        assertEquals("under an empty title's height", 120, down.offset);

        ConsoleHomeListAdapter.Reveal phone = adapter.reveal(deep, 4, 120, 300, 1080, false);
        assertEquals(deep, phone.position);
        assertEquals(0, phone.offset);
    }

    @Test
    public void nothingToShowStartsNowhere() {
        ConsoleHomeListAdapter empty = new ConsoleHomeListAdapter(context, null, uuid -> null, null);
        ConsoleHomeListAdapter.Reveal reveal = empty.reveal(-1, 0, 60, 60, 1000, true);
        assertEquals(-1, reveal.position);
        assertEquals(0, reveal.offset);
    }
}
