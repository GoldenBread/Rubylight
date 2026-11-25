package com.limelight.console;

import static com.limelight.nativeapi.NativeApiFixtures.game;
import static com.limelight.nativeapi.NativeApiFixtures.library;
import static com.limelight.nativeapi.NativeApiFixtures.nativeHost;
import static com.limelight.nativeapi.NativeApiFixtures.ok;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertTrue;
import static org.robolectric.Shadows.shadowOf;

import android.app.Activity;
import android.content.Context;
import android.content.Intent;
import android.os.Build;
import android.os.Looper;
import android.preference.PreferenceManager;
import android.view.KeyEvent;
import android.view.View;
import android.view.ViewGroup;
import android.view.ViewParent;

import androidx.recyclerview.widget.RecyclerView;

import com.google.android.material.button.MaterialButton;
import com.limelight.R;
import com.limelight.computers.ComputerManagerListener;
import com.limelight.computers.ComputerManagerService;
import com.limelight.nativeapi.ConsoleSnapshot;
import com.limelight.nativeapi.NativeApiFixtures;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.preferences.GlPreferences;
import com.limelight.ui.ActionSheet;

import org.junit.After;
import org.junit.Before;
import org.junit.Rule;
import org.junit.rules.TemporaryFolder;
import org.robolectric.Robolectric;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.shadows.ShadowDialog;
import org.robolectric.util.ReflectionHelpers;

import java.io.File;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.HashSet;
import java.util.List;
import java.util.Set;
import java.util.function.BooleanSupplier;

/** Shared Robolectric fixture and UI helpers for Home shell, navigation and offline/wake test suites. */
abstract class HomeShellCase {
    @Rule public TemporaryFolder temporary = new TemporaryFolder();

    protected Context context;
    protected final List<String> renamed = new ArrayList<>();
    protected final List<ComputerDetails> removed = new ArrayList<>();

    @Before
    public void setUp() throws Exception {
        context = RuntimeEnvironment.getApplication();
        PreferenceManager.getDefaultSharedPreferences(context).edit().clear().commit();
        GlPreferences gl = GlPreferences.readPreferences(context);
        gl.savedFingerprint = Build.FINGERPRINT;
        gl.glRenderer = "Test renderer";
        gl.writePreferences();
        // What a host's refresh leaves behind once it has shown its games.
        File directory = new File(context.getFilesDir(), "console-library");
        assertTrue(directory.isDirectory() || directory.mkdirs());
        Files.write(new File(directory, "tower.json").toPath(),
                "{\"fetched_ms\":1,\"library\":{\"revision\":\"r1\",\"games\":[{\"install_id\":\"i1\",\"app_id\":1,\"title\":\"Portal\"}]}}"
                        .getBytes(StandardCharsets.UTF_8));
    }

    @After
    public void tearDown() {
        PreferenceManager.getDefaultSharedPreferences(context).edit().clear().commit();
    }

    protected ActivityController<HomeShellTest.Home> home() {
        return Robolectric.buildActivity(HomeShellTest.Home.class).setup().visible();
    }

    protected static ComputerManagerService.ComputerManagerBinder binder(List<String> renamed, List<ComputerDetails> removed) {
        return new ComputerManagerService().new ComputerManagerBinder() {
            @Override public boolean isPersistent() { return false; }
            @Override public void startPolling(ComputerManagerListener listener) { }
            @Override public void stopPolling() { }
            @Override public void renameComputer(String uuid, String alias) { renamed.add(uuid + "=" + alias); }
            @Override public void removeComputer(ComputerDetails computer) { removed.add(computer); }
            @Override public void invalidateStateForComputer(String uuid) { }
            @Override public String getUniqueId() { return null; }
            @Override public void waitForPollingStopped() { }
        };
    }

    /** Reports the hosts to Home's feed and paints the cards they would give, laid out in a 1080x1920 list. */
    protected RecyclerView showHosts(HomeShellTest.Home home, ComputerDetails... hosts) throws Exception {
        return showHosts(home, 1, hosts);
    }

    /** {@link #showHosts(HomeShellTest.Home, ComputerDetails...)} with {@code games} games on each online host. */
    protected RecyclerView showHosts(HomeShellTest.Home home, int games, ComputerDetails... hosts) throws Exception {
        return showHosts(home, games, false, hosts);
    }

    /**
     * {@link #showHosts(HomeShellTest.Home, int, ComputerDetails...)}; with {@code saved}, every host has listed its games once
     * before (so an offline one still shows them from its saved library) and Portal is one game across hosts.
     */
    protected RecyclerView showHosts(HomeShellTest.Home home, int games, boolean saved, ComputerDetails... hosts) throws Exception {
        HostFeed feed = ReflectionHelpers.getField(home, "feed");
        ReflectionHelpers.setField(feed, "binder", binder(renamed, removed));
        ConsoleRig rig = new ConsoleRig(temporary.newFolder());
        try {
            Set<String> online = new HashSet<>();
            for (ComputerDetails host : hosts) {
                feed.update(host);
                if (host.state == ComputerDetails.State.ONLINE) online.add(host.uuid);
                if (host.state == ComputerDetails.State.ONLINE || saved) {
                    String[] listed = new String[games];
                    for (int i = 0; i < games; i++) {
                        listed[i] = game(host.uuid + "-g" + i, i + 1, i == 0 ? "Portal" : "Game " + i,
                                saved && i == 0 ? "steam:400" : null, false);
                    }
                    rig.transport.reply(host.uuid, "library", ok(library("r", listed)));
                    rig.transport.reply(host.uuid, "session", ok(NativeApiFixtures.IDLE));
                }
                if (saved && host.state != ComputerDetails.State.ONLINE) {
                    // The host was awake once: its library is on disk, and only that is left of it now.
                    rig.repository.refresh(Collections.singletonList(nativeHost(host.uuid, host.name, true)));
                }
            }
            ConsoleSnapshot snapshot = rig.repository.refresh(Arrays.asList(hosts));
            HomeModel model = HomeModel.of(snapshot, online);
            // A phone's Home is the list of cards; a TV's is the shelves.
            ConsoleHomeAdapter shelves = ReflectionHelpers.getField(home, "shelves");
            if (shelves != null) {
                shelves.submit(model);
            } else {
                ConsoleHomeListAdapter adapter = ReflectionHelpers.getField(home, "adapter");
                adapter.submit(model);
            }
        } finally {
            rig.close();
        }
        RecyclerView list = home.findViewById(R.id.console_list);
        list.measure(View.MeasureSpec.makeMeasureSpec(1080, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(1920, View.MeasureSpec.EXACTLY));
        list.layout(0, 0, 1080, 1920);
        return list;
    }

    protected static View hostCard(HomeShellTest.Home home, String uuid) {
        return cardView(home, "host:" + uuid);
    }

    /** The laid-out card with the adapter key, on a phone's list or a TV's shelves. */
    protected static View cardView(HomeShellTest.Home home, String key) {
        RecyclerView list = home.findViewById(R.id.console_list);
        ConsoleHomeAdapter shelves = ReflectionHelpers.getField(home, "shelves");
        if (shelves != null) {
            ConsoleHomeAdapter.Cursor at = shelves.positionOfKey(key);
            assertNotNull("a card for " + key, at);
            list.measure(View.MeasureSpec.makeMeasureSpec(1080, View.MeasureSpec.EXACTLY),
                    View.MeasureSpec.makeMeasureSpec(1920, View.MeasureSpec.EXACTLY));
            list.layout(0, 0, 1080, 1920);
            View card = shelves.cardView(list, at);
            assertNotNull("laid out", card);
            return card;
        }
        ConsoleHomeListAdapter adapter = ReflectionHelpers.getField(home, "adapter");
        int position = adapter.positionOfKey(key);
        assertTrue("a card for " + key, position >= 0);
        list.measure(View.MeasureSpec.makeMeasureSpec(1080, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(1920, View.MeasureSpec.EXACTLY));
        list.layout(0, 0, 1080, 1920);
        RecyclerView.ViewHolder holder = list.findViewHolderForAdapterPosition(position);
        assertNotNull("laid out", holder);
        return holder.itemView;
    }

    protected static List<String> labels(ActionSheet sheet) {
        List<String> actions = new ArrayList<>();
        collect(sheet.findViewById(android.R.id.content), actions);
        return actions;
    }

    private static void collect(View view, List<String> actions) {
        if (view instanceof MaterialButton) {
            actions.add(((MaterialButton) view).getText().toString());
        } else if (view instanceof ViewGroup) {
            ViewGroup group = (ViewGroup) view;
            for (int i = 0; i < group.getChildCount(); i++) collect(group.getChildAt(i), actions);
        }
    }

    protected List<String> expected(HomeShellTest.Home home, int... ids) {
        List<String> labels = new ArrayList<>();
        for (int id : ids) labels.add(home.getString(id));
        return labels;
    }

    protected static ActionSheet openSheet() {
        shadowOf(Looper.getMainLooper()).idle();
        android.app.Dialog dialog = ShadowDialog.getLatestDialog();
        return dialog instanceof ActionSheet && dialog.isShowing() ? (ActionSheet) dialog : null;
    }

    protected static void closeSheet(ActionSheet sheet) {
        sheet.dismiss();
        shadowOf(Looper.getMainLooper()).idle();
    }

    protected static boolean press(View card, int keyCode) {
        // A view group hands key events to its own listener only while it has focus.
        if (!card.hasFocus()) assertTrue(card.requestFocus());
        boolean down = card.dispatchKeyEvent(new KeyEvent(KeyEvent.ACTION_DOWN, keyCode));
        boolean up = card.dispatchKeyEvent(new KeyEvent(KeyEvent.ACTION_UP, keyCode));
        return down || up;
    }

    protected static Intent nextStarted(Activity activity) {
        return shadowOf(activity).getNextStartedActivity();
    }

    /** Lets Home's worker and the main looper run until the condition holds. */
    protected static void await(BooleanSupplier done) throws Exception {
        long end = System.currentTimeMillis() + 5000;
        while (!done.getAsBoolean() && System.currentTimeMillis() < end) {
            shadowOf(Looper.getMainLooper()).idle();
            Thread.sleep(20);
        }
        assertTrue("in time", done.getAsBoolean());
    }

    protected static String asleep(HomeShellTest.Home home, String line) {
        return home.getString(R.string.console_card_asleep, line);
    }

    /** Shelves are the default now; these tests are about the grid, which stays a choice. */
    protected static void chooseGrid() {
        new HomeView(HomeView.Layout.GRID, HomeView.Group.NONE, HomeView.Size.MEDIUM)
                .save(PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication()));
    }

    protected static RecyclerView list(HomeShellTest.Home home) {
        return home.findViewById(R.id.console_list);
    }

    protected static boolean isInside(View parent, View child) {
        if (child == parent) return true;
        for (ViewParent p = child.getParent(); p != null; p = p.getParent()) {
            if (p == parent) return true;
        }
        return false;
    }

    protected static void layOut(HomeShellTest.Home home) {
        View root = home.getWindow().getDecorView();
        int w = home.getResources().getDisplayMetrics().widthPixels;
        int h = home.getResources().getDisplayMetrics().heightPixels;
        root.measure(View.MeasureSpec.makeMeasureSpec(w, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(h, View.MeasureSpec.EXACTLY));
        root.layout(0, 0, w, h);
    }
}
