package com.limelight.console;

import static com.limelight.nativeapi.NativeApiFixtures.game;
import static com.limelight.nativeapi.NativeApiFixtures.library;
import static com.limelight.nativeapi.NativeApiFixtures.nativeHost;
import static com.limelight.nativeapi.NativeApiFixtures.ok;
import static org.junit.Assert.assertTrue;
import static org.robolectric.Shadows.shadowOf;

import android.app.Dialog;
import android.content.Context;
import android.content.Intent;
import android.os.Bundle;
import android.os.Looper;
import android.preference.PreferenceManager;

import androidx.appcompat.app.AlertDialog;

import com.limelight.computers.ComputerManagerListener;
import com.limelight.computers.ComputerManagerService;
import com.limelight.nativeapi.NativeApiFixtures;
import com.limelight.nvstream.http.ComputerDetails;

import org.junit.After;
import org.junit.Before;
import org.junit.Rule;
import org.junit.rules.TemporaryFolder;
import org.robolectric.Robolectric;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.shadows.ShadowDialog;
import org.robolectric.util.ReflectionHelpers;

import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.TimeUnit;
import java.util.function.BooleanSupplier;

/** Shared Robolectric harness for Open &amp; play countdown, replacement and recovery tests. */
abstract class GameDetailOpenPlayCase {
    @Rule public TemporaryFolder temporary = new TemporaryFolder();

    protected static final String PORTAL_APP = "{\"install_id\":\"t-portal\",\"app_id\":11,\"title\":\"Portal\"}";
    protected static final String ZELDA_APP = "{\"install_id\":\"t-zelda\",\"app_id\":22,\"title\":\"Zelda\"}";
    protected static final String SESSION_FAILS =
            "{\"error\":{\"kind\":\"status\",\"message\":\"boom\",\"http_status\":500}}";

    protected Context context;
    protected ConsoleRig rig;
    protected String sessionThere = NativeApiFixtures.IDLE;
    protected String stableThere = null;
    protected String libraryThere = library("r", game("t-portal", 11, "Portal", "steam:400", false));
    protected ActivityController<GameDetailWakeTest.Detail> controller;
    /** Every page a test opened: the ones it opened before its last must not outlive it either. */
    protected final List<ActivityController<GameDetailWakeTest.Detail>> opened = new ArrayList<>();
    protected GameDetailWakeTest.Detail screen;
    protected HostFeed feed;

    @Before
    public void setUp() throws Exception {
        context = RuntimeEnvironment.getApplication();
        PreferenceManager.getDefaultSharedPreferences(context).edit().clear().commit();
        OpenPlay.disarm(context);
        GameDetailWakeTest.Detail.woken.clear();
        GameDetailWakeTest.Detail.played.clear();
        ShadowDialog.reset();
    }

    @After
    public void tearDown() {
        // A read still under way must not start its game in the next test.
        for (ActivityController<GameDetailWakeTest.Detail> page : opened) {
            if (!page.get().isDestroyed()) page.destroy();
        }
        GameDetailWakeTest.Detail.repository = null;
    }

    protected void open(boolean online, String mac, Bundle state) throws Exception {
        rig = new ConsoleRig(temporary.newFolder());
        rig.transport.reply("tower", "library", ok(libraryThere));
        rig.transport.reply("tower", "session", sessionThere.startsWith("{\"error") ? sessionThere : ok(sessionThere));
        rig.repository.refresh(Collections.singletonList(nativeHost("tower", "Tower", true)));
        GameDetailWakeTest.Detail.repository = rig.repository;
        Intent intent = new Intent(context, GameDetailWakeTest.Detail.class)
                .putExtra(GameDetailActivity.EXTRA_HOST_UUID, "tower")
                .putExtra(GameDetailActivity.EXTRA_APP_ID, 11)
                .putExtra(GameDetailActivity.EXTRA_TITLE, "Portal")
                .putExtra(GameDetailActivity.EXTRA_HOST_NAME, "Tower")
                .putExtra(GameDetailActivity.EXTRA_STABLE_ID, stableThere)
                .putExtra(GameDetailActivity.EXTRA_OPEN_PLAY, true);
        controller = Robolectric.buildActivity(GameDetailWakeTest.Detail.class, intent);
        opened.add(controller);
        controller = state == null ? controller.setup() : controller.setup(state);
        controller.visible();
        screen = controller.get();
        feed = ReflectionHelpers.getField(screen, "feed");
        ReflectionHelpers.setField(feed, "binder", binder());
        ComputerDetails tower = nativeHost("tower", "Tower", online);
        tower.macAddress = mac;
        feed.update(tower);
    }

    protected static ComputerManagerService.ComputerManagerBinder binder() {
        return new ComputerManagerService().new ComputerManagerBinder() {
            @Override public boolean isPersistent() { return false; }
            @Override public void startPolling(ComputerManagerListener listener) { }
            @Override public void stopPolling() { }
            @Override public void renameComputer(String uuid, String alias) { }
            @Override public void removeComputer(ComputerDetails computer) { }
            @Override public void invalidateStateForComputer(String uuid) { }
            @Override public String getUniqueId() { return "unique"; }
            @Override public void waitForPollingStopped() { }
        };
    }

    protected static void pass(long ms) {
        shadowOf(Looper.getMainLooper()).idleFor(ms, TimeUnit.MILLISECONDS);
    }

    protected static void await(BooleanSupplier done) throws Exception {
        long end = System.currentTimeMillis() + 5000;
        while (!done.getAsBoolean() && System.currentTimeMillis() < end) {
            pass(50);
            Thread.sleep(20);
        }
        assertTrue("in time", done.getAsBoolean());
    }

    /**
     * Until every read the page handed its worker thread is over and what it found is posted. Waiting moves no clock.
     *
     * <p>Under Robolectric each frame moves the clock 15 ms on, inside whichever idle draws it, and the page's
     * animations ask for frames for a while once a read's answer is drawn. A read still running during a {@link #pass}
     * lands at a point of it set by real time, and its frames push the clock past where the test meant to stop:
     * a countdown then ended inside a pass meant to stop 1 ms short of it, or its ticks fell later than the test
     * allowed for. Tests that time something against the countdown let the read land first.
     */
    protected void readsDone() throws Exception {
        // One task at a time: once this one runs, the ones before it are over.
        ReflectionHelpers.<ExecutorService>getField(screen, "worker").submit(() -> { })
                .get(5, TimeUnit.SECONDS);
        pass(20);
    }

    /** The page's first read of the PC, due 250 ms after it opens, landed. */
    protected void loaded() throws Exception {
        pass(300);
        readsDone();
        assertTrue("read", ReflectionHelpers.<Boolean>getField(screen, "modelLive"));
    }

    protected static AlertDialog shown() {
        Dialog dialog = ShadowDialog.getLatestDialog();
        assertTrue("a dialog is up", dialog != null && dialog.isShowing());
        return (AlertDialog) dialog;
    }
}
