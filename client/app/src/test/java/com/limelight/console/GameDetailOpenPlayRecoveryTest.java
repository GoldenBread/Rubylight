package com.limelight.console;

import static com.limelight.nativeapi.NativeApiFixtures.game;
import static com.limelight.nativeapi.NativeApiFixtures.library;
import static com.limelight.nativeapi.NativeApiFixtures.nativeHost;
import static com.limelight.nativeapi.NativeApiFixtures.ok;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;
import static org.robolectric.Shadows.shadowOf;

import android.app.Application;
import android.app.Dialog;
import android.content.Context;
import android.content.Intent;
import android.net.ConnectivityManager;
import android.net.NetworkInfo;
import android.os.Bundle;
import android.os.SystemClock;
import android.widget.TextView;

import androidx.appcompat.app.AlertDialog;

import com.limelight.PcViewLifecycleTest;
import com.limelight.R;
import com.limelight.nativeapi.NativeApiFixtures;
import com.limelight.nativeapi.RecentsStore;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;
import com.limelight.pin.PinGate;
import com.limelight.pin.PinLockActivity;
import com.limelight.pin.PinStore;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowDialog;
import org.robolectric.util.ReflectionHelpers;

import java.nio.charset.StandardCharsets;
import java.util.Collections;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.ExecutorService;

/** Open &amp; play network wait, session/read retries, stable-id matching, timeout limits and PIN gating. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class, shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class GameDetailOpenPlayRecoveryTest extends GameDetailOpenPlayCase {

    @Test
    public void aNetworkThatStaysAwayOpensAsUsualAfterAMinuteWithoutACountdownOrTheGuard() throws Exception {
        ConnectivityManager connectivity = (ConnectivityManager) context.getSystemService(Context.CONNECTIVITY_SERVICE);
        shadowOf(connectivity).setActiveNetworkInfo(null);
        open(true, null, null);
        assertEquals(context.getString(R.string.open_play_waiting_network),
                ((TextView) shown().findViewById(android.R.id.message)).getText().toString());
        pass(59_000);
        assertEquals(context.getString(R.string.open_play_waiting_network),
                ((TextView) shown().findViewById(android.R.id.message)).getText().toString());
        pass(1_500);
        assertEquals(context.getString(R.string.open_play_no_network),
                ((TextView) shown().findViewById(android.R.id.message)).getText().toString());
        pass(5000);
        assertFalse(OpenPlay.armed(context));
        assertTrue(GameDetailWakeTest.Detail.played.isEmpty());
        shown().dismiss();
        pass(100);
        assertTrue(screen.isFinishing());
    }

    @Test
    public void aNetworkThatComesBackGivesThePcAPollAndThenTheFullCountdown() throws Exception {
        ConnectivityManager connectivity = (ConnectivityManager) context.getSystemService(Context.CONNECTIVITY_SERVICE);
        NetworkInfo network = connectivity.getActiveNetworkInfo();
        shadowOf(connectivity).setActiveNetworkInfo(null);
        open(true, null, null);
        loaded();
        pass(30_000);
        shadowOf(connectivity).setActiveNetworkInfo(network);
        // The network is seen within a check and read again 250 ms later: that read lands before the clock goes on.
        pass(1_000);
        readsDone();
        pass(4_000);
        assertEquals(context.getString(R.string.open_play_waiting_network),
                ((TextView) shown().findViewById(android.R.id.message)).getText().toString());
        pass(1_500);
        assertEquals("Starting on Tower in 3", ((TextView) shown().findViewById(android.R.id.message)).getText().toString());
        assertFalse(OpenPlay.armed(context));
        pass(3100);
        assertTrue(OpenPlay.armed(context));
        await(() -> !GameDetailWakeTest.Detail.played.isEmpty());
    }

    @Test
    public void whatThePcSaysIsReadAgainOnceTheNetworkIsBack() throws Exception {
        ConnectivityManager connectivity = (ConnectivityManager) context.getSystemService(Context.CONNECTIVITY_SERVICE);
        NetworkInfo network = connectivity.getActiveNetworkInfo();
        shadowOf(connectivity).setActiveNetworkInfo(null);
        open(true, null, null);
        loaded();
        rig.transport.reply("tower", "session", ok(NativeApiFixtures.session("streaming", "other_device", ZELDA_APP, "[]")));
        shadowOf(connectivity).setActiveNetworkInfo(network);
        Dialog waiting = shown();
        await(() -> {
            pass(500);
            // No dialog while the countdown's end reads the PC.
            return ShadowDialog.getLatestDialog() != waiting && ShadowDialog.getLatestDialog().isShowing()
                    && ((TextView) shown().findViewById(android.R.id.message)).getText()
                    .toString().equals(context.getString(R.string.open_play_other_device, "Tower"));
        });
        assertTrue(GameDetailWakeTest.Detail.played.isEmpty());
    }

    @Test
    public void leavingThePageEndsTheWaitForTheNetwork() throws Exception {
        ConnectivityManager connectivity = (ConnectivityManager) context.getSystemService(Context.CONNECTIVITY_SERVICE);
        NetworkInfo network = connectivity.getActiveNetworkInfo();
        shadowOf(connectivity).setActiveNetworkInfo(null);
        open(true, null, null);
        pass(1000);
        controller.pause();
        shadowOf(connectivity).setActiveNetworkInfo(network);
        pass(70_000);
        Thread.sleep(200);
        pass(100);
        assertFalse(OpenPlay.armed(context));
        assertTrue(GameDetailWakeTest.Detail.played.isEmpty());
    }

    @Test
    public void theWaitForTheNetworkCancelsLikeTheCountdown() throws Exception {
        ConnectivityManager connectivity = (ConnectivityManager) context.getSystemService(Context.CONNECTIVITY_SERVICE);
        shadowOf(connectivity).setActiveNetworkInfo(null);
        open(true, null, null);
        pass(1000);
        shown().getButton(AlertDialog.BUTTON_NEGATIVE).performClick();
        pass(100);
        assertTrue(screen.isFinishing());
        pass(70_000);
        assertFalse(OpenPlay.armed(context));
        assertTrue(GameDetailWakeTest.Detail.played.isEmpty());
    }

    private void unknown() {
        ComputerDetails tower = nativeHost("tower", "Tower", false);
        tower.state = ComputerDetails.State.UNKNOWN;
        feed.update(tower);
    }

    @Test
    public void aPcNotHeardFromYetIsWaitedForAndNotCalledOffline() throws Exception {
        open(true, null, null);
        unknown();
        Dialog countdown = shown();
        pass(3100);
        Thread.sleep(200);
        pass(10_000);
        Thread.sleep(200);
        pass(10_000);
        assertTrue("no failure yet", ShadowDialog.getLatestDialog() == countdown);
        assertTrue(GameDetailWakeTest.Detail.played.isEmpty());
        feed.update(nativeHost("tower", "Tower", true));
        await(() -> !GameDetailWakeTest.Detail.played.isEmpty());
    }

    @Test
    public void aPcNeverHeardFromFailsAtTheLimit() throws Exception {
        open(true, null, null);
        unknown();
        Dialog countdown = shown();
        pass(3100);
        Thread.sleep(200);
        pass(44_000);
        assertTrue("within the limit", ShadowDialog.getLatestDialog() == countdown);
        pass(1_100);
        await(() -> ShadowDialog.getLatestDialog() != countdown);
        assertEquals(context.getString(R.string.error_pc_offline),
                ((TextView) shown().findViewById(android.R.id.message)).getText().toString());
        assertTrue(OpenPlay.armed(context));
    }

    @Test
    public void aSessionThatCouldNotBeReadIsAskedAgainAndAGameOfAnotherDeviceStillStopsTheStart() throws Exception {
        sessionThere = SESSION_FAILS;
        open(true, null, null);
        Dialog countdown = shown();
        pass(3100);
        Thread.sleep(300);
        pass(100);
        Thread.sleep(300);
        pass(100);
        assertTrue("not read as free", GameDetailWakeTest.Detail.played.isEmpty());
        rig.transport.reply("tower", "session", ok(NativeApiFixtures.session("streaming", "other_device", ZELDA_APP, "[]")));
        await(() -> {
            pass(2100);
            return ShadowDialog.getLatestDialog() != countdown;
        });
        assertEquals(context.getString(R.string.open_play_other_device, "Tower"),
                ((TextView) shown().findViewById(android.R.id.message)).getText().toString());
        assertTrue(GameDetailWakeTest.Detail.played.isEmpty());
    }

    @Test
    public void aGameAnotherDeviceStartedDuringTheCountdownStopsTheStart() throws Exception {
        open(true, null, null);
        loaded();
        Dialog countdown = shown();
        // The page read the PC as free; the other device starts after that read, and no poll comes before the end.
        rig.transport.reply("tower", "session", ok(NativeApiFixtures.session("streaming", "other_device", ZELDA_APP, "[]")));
        pass(3100);
        await(() -> ShadowDialog.getLatestDialog() != countdown && ShadowDialog.getLatestDialog().isShowing());
        assertEquals(context.getString(R.string.open_play_other_device, "Tower"),
                ((TextView) shown().findViewById(android.R.id.message)).getText().toString());
        assertTrue(GameDetailWakeTest.Detail.played.isEmpty());
    }

    @Test
    public void aSessionReadThatRecoversStartsTheGame() throws Exception {
        sessionThere = SESSION_FAILS;
        open(true, null, null);
        pass(3100);
        Thread.sleep(300);
        pass(100);
        assertTrue("a failed read is not a free PC", GameDetailWakeTest.Detail.played.isEmpty());
        rig.transport.reply("tower", "session", ok(NativeApiFixtures.IDLE));
        await(() -> {
            pass(2100);
            return !GameDetailWakeTest.Detail.played.isEmpty();
        });
    }

    @Test
    public void aSessionThatNeverReadsFailsAtTheLimitWithTheGuardArmed() throws Exception {
        sessionThere = SESSION_FAILS;
        open(true, null, null);
        // The limit names what the reads said: without one landed, it would say the list never came.
        loaded();
        Dialog countdown = shown();
        pass(3100);
        await(() -> {
            pass(2100);
            return ShadowDialog.getLatestDialog() != countdown;
        });
        assertEquals(context.getString(R.string.open_play_no_answer, "Tower"),
                ((TextView) shown().findViewById(android.R.id.message)).getText().toString());
        assertTrue(GameDetailWakeTest.Detail.played.isEmpty());
        assertTrue(OpenPlay.armed(context));
    }

    @Test
    public void anotherGameUnderTheSavedAppIdIsNotStartedUnderTheSavedTitle() throws Exception {
        stableThere = "t-portal";
        libraryThere = library("r", game("t-zelda", 11, "Zelda", "steam:500", false));
        open(true, null, null);
        Dialog countdown = shown();
        pass(3100);
        await(() -> ShadowDialog.getLatestDialog() != countdown && ShadowDialog.getLatestDialog().isShowing());
        assertEquals(context.getString(R.string.open_play_not_listed, "Tower"),
                ((TextView) shown().findViewById(android.R.id.message)).getText().toString());
        assertTrue(GameDetailWakeTest.Detail.played.isEmpty());
    }

    @Test
    public void aSavedGameTheHostMovedToAnotherAppIdStillStarts() throws Exception {
        stableThere = "t-portal";
        libraryThere = library("r", game("t-zelda", 11, "Zelda", "steam:500", false),
                game("t-portal", 33, "Portal", "steam:400", false));
        open(true, null, null);
        pass(3100);
        await(() -> !GameDetailWakeTest.Detail.played.isEmpty());
        assertEquals("Portal", GameDetailWakeTest.Detail.played.get(0));
        assertEquals(33, ReflectionHelpers.<GameDetailModel>getField(screen, "model").appId);
    }

    @Test
    public void aGameStartedFromALibraryIsRememberedByTheLibrarysIdWhichTheHostIsNotSent() throws Exception {
        open(true, null, null);
        ComputerDetails tower = nativeHost("tower", "Tower", true);
        NvApp app = CoverLoaders.appOf("Portal", 11,
                rig.repository.cached(Collections.singletonList(tower)).host("tower").sources.get(0));
        assertEquals("", app.getAppUuid());
        assertEquals("t-portal", RecentsStore.Recent.of(tower, app, 1).stableId);
        assertEquals("t-portal", PinLockActivity.forStream(context, new Intent(), tower, app)
                .getStringExtra("pin_recent_stable_id"));
    }

    @Test
    public void aClassicEntryWithNoUuidGoesByItsAppIdAndAnotherUuidIsAnotherGame() throws Exception {
        open(true, null, null);
        ComputerDetails tower = nativeHost("tower", "Tower", true);
        NvApp entry = new NvApp("Portal", 11, false);
        rig.classicApps.put("tower", Collections.singletonList(entry));
        assertTrue(rig.repository.classicListHas(tower, 11, "t-portal"));
        assertFalse(rig.repository.classicListHas(tower, 12, "t-portal"));
        entry.setAppUuid("t-zelda");
        assertFalse(rig.repository.classicListHas(tower, 11, "t-portal"));
    }

    /** To just before the countdown's end on a page read while the PC was away, then the PC comes back. */
    private Dialog pcBackAsTheCountdownEnds() throws Exception {
        open(false, null, null);
        loaded();
        Dialog countdown = shown();
        pass(ReflectionHelpers.<Long>getField(screen, "countdownEnd") - SystemClock.elapsedRealtime() - 100);
        feed.update(nativeHost("tower", "Tower", true));
        return countdown;
    }

    @Test
    public void aPcBackJustBeforeTheReloadIsReadAgainAndTheGameStarts() throws Exception {
        Dialog countdown = pcBackAsTheCountdownEnds();
        pass(150);
        assertTrue("armed, and no failure", OpenPlay.armed(context) && ShadowDialog.getLatestDialog() == countdown);
        await(() -> !GameDetailWakeTest.Detail.played.isEmpty());
        assertTrue(ShadowDialog.getLatestDialog() == countdown);
    }

    @Test
    public void aPcBackJustBeforeTheReloadWaitsWithinTheSameLimit() throws Exception {
        Dialog countdown = pcBackAsTheCountdownEnds();
        // Nothing is read from here on: the service is gone, so the reload stops at its first line.
        ReflectionHelpers.setField(feed, "binder", null);
        pass(44_500);
        assertTrue("within the limit", ShadowDialog.getLatestDialog() == countdown);
        pass(1_000);
        assertEquals(context.getString(R.string.open_play_no_list, "Tower"),
                ((TextView) shown().findViewById(android.R.id.message)).getText().toString());
        assertTrue(OpenPlay.armed(context));
    }

    @Test
    public void aPcThatWokeStillHasALimitWhenItsReadNeverComesBack() throws Exception {
        open(false, "00:11:22:33:44:55", null);
        Dialog countdown = shown();
        pass(3100);
        await(() -> !GameDetailWakeTest.Detail.woken.isEmpty());
        // Every read from here on queues behind this one.
        CountDownLatch hold = new CountDownLatch(1);
        ReflectionHelpers.<ExecutorService>getField(screen, "worker").execute(() -> {
            try {
                hold.await();
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
            }
        });
        feed.update(nativeHost("tower", "Tower", true));
        HostWaker waker = ReflectionHelpers.getField(screen, "waker");
        await(() -> {
            pass(1000);
            return !waker.isWaiting();
        });
        pass(30_000);
        assertTrue("within the limit", ShadowDialog.getLatestDialog() == countdown);
        pass(16_000);
        hold.countDown();
        assertEquals(context.getString(R.string.open_play_no_list, "Tower"),
                ((TextView) shown().findViewById(android.R.id.message)).getText().toString());
        assertTrue(OpenPlay.armed(context));
    }

    /** Called directly: the page's own callers ask {@code wakeable} first, which says no for a PC that is online. */
    @Test
    public void theOnlineBranchOfTheWakeHasALimitOfItsOwn() throws Exception {
        sessionThere = SESSION_FAILS;
        open(true, null, null);
        Dialog countdown = shown();
        pass(3100);
        // Nothing is read from here on: the service is gone, so the reload stops at its first line.
        ReflectionHelpers.setField(feed, "binder", null);
        Thread.sleep(300);
        pass(100);
        ReflectionHelpers.callInstanceMethod(screen, "stopAutoLimit");
        ReflectionHelpers.callInstanceMethod(screen, "wakeAndPlay");
        pass(44_000);
        assertTrue("within the limit", ShadowDialog.getLatestDialog() == countdown);
        pass(1_200);
        assertTrue("the limit ended it", ShadowDialog.getLatestDialog() != countdown);
        assertTrue(OpenPlay.armed(context));
    }

    @Test
    public void aRecreatedPageDoesNothingByItself() throws Exception {
        open(true, null, new Bundle());
        assertNull(ShadowDialog.getLatestDialog());
        pass(5000);
        assertTrue(GameDetailWakeTest.Detail.played.isEmpty());
        assertFalse(OpenPlay.armed(context));
    }

    @Test
    public void theAppPinComesFirstAndTheFullCountdownFollowsIt() throws Exception {
        PinGate.resetForTests();
        PinStore.clearForTests(context);
        PinStore store = new PinStore(context);
        store.setPin("1234".getBytes(StandardCharsets.US_ASCII));
        store.setMode(PinStore.Mode.APP);
        try {
            open(true, null, null);
            assertNull("no countdown behind the PIN", ShadowDialog.getLatestDialog());
            pass(5000);
            assertFalse(OpenPlay.armed(context));
            assertTrue(GameDetailWakeTest.Detail.played.isEmpty());

            PinGate.unlockApp();
            controller.pause().resume();
            assertEquals("Starting on Tower in 3", ((TextView) shown().findViewById(android.R.id.message)).getText().toString());
        } finally {
            store.removePin();
            PinGate.resetForTests();
            PinStore.clearForTests(context);
        }
    }
}
