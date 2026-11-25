package com.limelight.console;

import static com.limelight.nativeapi.NativeApiFixtures.game;
import static com.limelight.nativeapi.NativeApiFixtures.library;
import static com.limelight.nativeapi.NativeApiFixtures.nativeHost;
import static com.limelight.nativeapi.NativeApiFixtures.ok;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertSame;
import static org.junit.Assert.assertTrue;
import static org.robolectric.Shadows.shadowOf;

import android.app.Application;
import android.content.Intent;
import android.os.Looper;
import android.view.View;
import android.widget.ListView;
import android.widget.TextView;

import androidx.appcompat.app.AlertDialog;

import com.limelight.PcView;
import com.limelight.PcViewLifecycleTest;
import com.limelight.R;
import com.limelight.nativeapi.NativeApiFixtures;
import com.limelight.nvstream.http.ComputerDetails;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowDialog;
import org.robolectric.util.ReflectionHelpers;

import java.util.ArrayList;
import java.util.Arrays;

/** Offline PC look, Wake-on-LAN flows and multi-PC game chooser behavior on Home. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class, shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class HomeShellOfflineWakeTest extends HomeShellCase {

    /** A game of the saved library on a PC that is away, a game on one that answers, and the away PC's own card. */
    private void assertOfflineLook(HomeShellTest.Home home) throws Exception {
        showHosts(home, 2, true, nativeHost("tower", "Tower", false), nativeHost("laptop", "Laptop", true));
        View away = cardView(home, "game:host:tower:2");
        View awake = cardView(home, "game:host:laptop:2");
        assertEquals(OfflineLook.DIMMED, away.getAlpha(), 0f);
        assertEquals(1f, awake.getAlpha(), 0f);
        TextView line = away.findViewById(R.id.console_subtitle);
        if (line != null) {
            assertEquals("says the PC is asleep", asleep(home, home.getString(R.string.console_card_on_host, "Tower")),
                    line.getText().toString());
            assertFalse(((TextView) awake.findViewById(R.id.console_subtitle)).getText().toString().contains("asleep"));
        } else {
            // A poster on a TV has no second line: its badge says it.
            TextView badge = away.findViewById(R.id.console_badge);
            assertEquals(View.VISIBLE, badge.getVisibility());
            assertEquals(home.getString(R.string.console_badge_asleep), badge.getText().toString());
            assertEquals(View.GONE, awake.findViewById(R.id.console_badge).getVisibility());
        }
        assertTrue("and so does a screen reader: " + away.getContentDescription(),
                away.getContentDescription().toString().endsWith(", PC asleep"));
        assertTrue("still focusable", away.isFocusable());
        assertTrue("still clickable", away.isClickable());
        assertTrue("and takes focus", away.requestFocus());
        View host = hostCard(home, "tower");
        assertEquals(OfflineLook.DIMMED, host.getAlpha(), 0f);
        assertTrue(host.isFocusable());
        assertEquals(1f, hostCard(home, "laptop").getAlpha(), 0f);
    }

    @Test
    public void onAPhoneAPcThatIsAwayKeepsItsSavedGamesDimmedAndFocusable() throws Exception {
        ActivityController<HomeShellTest.Home> controller = home();
        try {
            assertOfflineLook(controller.get());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    @Config(qualifiers = "television")
    public void onTelevisionAPcThatIsAwayKeepsItsSavedGamesDimmedAndFocusable() throws Exception {
        ActivityController<HomeShellTest.Home> controller = home();
        try {
            assertOfflineLook(controller.get());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void tappingAPcThatIsAwayWakesItAndStaysOnHome() throws Exception {
        ActivityController<HomeShellTest.Home> controller = home();
        try {
            HomeShellTest.Home home = controller.get();
            ComputerDetails attic = nativeHost("attic", "Attic", false);
            attic.macAddress = "aa:bb:cc:dd:ee:ff";
            showHosts(home, attic);
            assertTrue(hostCard(home, "attic").performClick());
            await(() -> home.woken.size() == 1);
            assertEquals("attic", home.woken.get(0));
            assertNull("no screen opens while it wakes", nextStarted(home));
            // A second tap while it is waking sends nothing new.
            hostCard(home, "attic").performClick();
            Thread.sleep(100);
            assertEquals(1, home.woken.size());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void aWakeFromHomeShowsItsWaitUntilItEndsAndASecondTapShowsTheSameWait() throws Exception {
        ActivityController<HomeShellTest.Home> controller = home();
        try {
            HomeShellTest.Home home = controller.get();
            ComputerDetails attic = nativeHost("attic", "Attic", false);
            attic.macAddress = "aa:bb:cc:dd:ee:ff";
            showHosts(home, attic);
            assertTrue(hostCard(home, "attic").performClick());
            await(() -> home.woken.size() == 1);
            AlertDialog wait = (AlertDialog) ShadowDialog.getLatestDialog();
            assertNotNull(wait);
            assertTrue("the wait shows for as long as it lasts", wait.isShowing());
            assertEquals(home.getString(R.string.console_wake_waiting, "Attic"),
                    ((TextView) wait.findViewById(android.R.id.message)).getText().toString());
            hostCard(home, "attic").performClick();
            shadowOf(Looper.getMainLooper()).idle();
            assertSame("a second tap shows the same wait", wait, ShadowDialog.getLatestDialog());
            assertTrue(wait.isShowing());
            assertEquals("and sends nothing new", 1, home.woken.size());
            wait.getButton(AlertDialog.BUTTON_NEGATIVE).performClick();
            shadowOf(Looper.getMainLooper()).idle();
            assertFalse(wait.isShowing());
            HomeWake wake = ReflectionHelpers.getField(home, "wake");
            assertFalse("Cancel ends the wait", wake.isWaiting());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void tappingAPcThatIsAwayWithoutAMacAddressOpensThePcList() throws Exception {
        ActivityController<HomeShellTest.Home> controller = home();
        try {
            HomeShellTest.Home home = controller.get();
            showHosts(home, nativeHost("loft", "Loft", false));
            hostCard(home, "loft").performClick();
            Intent pcs = nextStarted(home);
            assertNotNull(pcs);
            assertEquals(PcView.class.getName(), pcs.getComponent().getClassName());
            assertTrue(home.woken.isEmpty());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void aGameSeveralPcsHaveAsksWhichPcAndOpensItOnTheChosenOne() throws Exception {
        ActivityController<HomeShellTest.Home> controller = home();
        ConsoleRig rig = new ConsoleRig(temporary.newFolder());
        try {
            HomeShellTest.Home home = controller.get();
            for (String uuid : new String[]{"tower", "laptop"}) {
                rig.transport.reply(uuid, "library", ok(library("r", game(uuid + "-g0", 1, "Portal", "steam:400", false))));
                rig.transport.reply(uuid, "session", ok(NativeApiFixtures.IDLE));
            }
            rig.repository.refresh(Arrays.asList(nativeHost("tower", "Tower", true), nativeHost("laptop", "Laptop", true)));
            ReflectionHelpers.setField(home, "repository", rig.repository);
            showHosts(home, 1, true, nativeHost("tower", "Tower", true), nativeHost("laptop", "Laptop", false));
            assertTrue(cardView(home, "game:game:steam:400").performClick());
            // More taps while the question is pending ask nothing again.
            cardView(home, "game:game:steam:400").performClick();
            await(() -> ShadowDialog.getLatestDialog() instanceof AlertDialog
                    && ShadowDialog.getLatestDialog().isShowing());
            shadowOf(Looper.getMainLooper()).idle();
            Thread.sleep(100);
            shadowOf(Looper.getMainLooper()).idle();
            assertEquals("one question", 1, ShadowDialog.getShownDialogs().size());
            AlertDialog chooser = (AlertDialog) ShadowDialog.getLatestDialog();
            ListView choices = chooser.getListView();
            assertEquals(2, choices.getCount());
            assertEquals("The PC that answers comes first", "Tower", choices.getItemAtPosition(0).toString());
            assertEquals("a PC that is away says so", asleep(home, "Laptop"), choices.getItemAtPosition(1).toString());
            assertNull("nothing opens before the choice", nextStarted(home));
            choices.performItemClick(null, 1, choices.getItemIdAtPosition(1));
            Intent detail = nextStarted(home);
            assertNotNull(detail);
            assertEquals("laptop", detail.getStringExtra(GameDetailActivity.EXTRA_HOST_UUID));
            assertEquals("Portal", detail.getStringExtra(GameDetailActivity.EXTRA_TITLE));
        } finally {
            rig.close();
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void aGameCardIsAwayOnlyWhenEveryPcThatHasItIsAway() throws Exception {
        ActivityController<HomeShellTest.Home> controller = home();
        try {
            HomeShellTest.Home home = controller.get();
            showHosts(home, 1, true, nativeHost("tower", "Tower", false), nativeHost("laptop", "Laptop", true));
            assertEquals("one PC answers: the shared game is playable", 1f,
                    cardView(home, "game:game:steam:400").getAlpha(), 0f);
            showHosts(home, 1, true, nativeHost("tower", "Tower", false), nativeHost("laptop", "Laptop", false));
            assertEquals("both are away", OfflineLook.DIMMED, cardView(home, "game:game:steam:400").getAlpha(), 0f);
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void tappingASecondPcThatIsAwayMovesTheWaitToIt() throws Exception {
        ActivityController<HomeShellTest.Home> controller = home();
        try {
            HomeShellTest.Home home = controller.get();
            ComputerDetails attic = nativeHost("attic", "Attic", false);
            attic.macAddress = "aa:bb:cc:dd:ee:ff";
            ComputerDetails loft = nativeHost("loft", "Loft", false);
            loft.macAddress = "11:22:33:44:55:66";
            showHosts(home, attic, loft);
            hostCard(home, "attic").performClick();
            await(() -> home.woken.size() == 1);
            hostCard(home, "loft").performClick();
            await(() -> home.woken.size() == 2);
            assertEquals(Arrays.asList("attic", "loft"), new ArrayList<>(home.woken));
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void aGameOnOnePcOpensWithoutAskingWhichPc() throws Exception {
        ActivityController<HomeShellTest.Home> controller = home();
        try {
            HomeShellTest.Home home = controller.get();
            showHosts(home, nativeHost("tower", "Tower", true));
            cardView(home, "game:host:tower:1").performClick();
            Intent detail = nextStarted(home);
            assertNotNull(detail);
            assertEquals("tower", detail.getStringExtra(GameDetailActivity.EXTRA_HOST_UUID));
        } finally {
            controller.pause().stop().destroy();
        }
    }
}
