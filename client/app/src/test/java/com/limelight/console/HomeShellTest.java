package com.limelight.console;

import static com.limelight.nativeapi.NativeApiFixtures.nativeHost;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;
import static org.robolectric.Shadows.shadowOf;

import android.app.Activity;
import android.app.Application;
import android.content.Intent;
import android.content.ServiceConnection;
import android.os.Looper;
import android.preference.PreferenceManager;
import android.view.KeyEvent;
import android.view.View;
import android.view.ViewGroup;
import android.widget.EditText;
import android.widget.LinearLayout;
import android.widget.TextView;

import androidx.appcompat.app.AlertDialog;
import androidx.recyclerview.widget.RecyclerView;

import com.google.android.material.appbar.MaterialToolbar;
import com.google.android.material.button.MaterialButton;
import com.limelight.PcView;
import com.limelight.PcViewLifecycleTest;
import com.limelight.R;
import com.limelight.computers.ComputerManagerListener;
import com.limelight.computers.ComputerManagerService;
import com.limelight.computers.HostActions;
import com.limelight.files.FileTransferActivity;
import com.limelight.nativeapi.NativeApiFixtures;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.jni.NightjarPolicyJni;
import com.limelight.preferences.StreamSettings;
import com.limelight.ui.ActionSheet;
import com.limelight.ui.ShellBar;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowDialog;
import org.robolectric.util.ReflectionHelpers;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.List;
import java.util.Set;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.Executor;
import java.util.function.LongSupplier;

/** Home under the shell bar, form-change holds, and the options of the PCs listed on it. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class, shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class HomeShellTest extends HomeShellCase {
    /** The tabs' capsule on a 960 x 540 dp TV (xhdpi), which PCs and a PC's games must match. */
    public static String tvCapsule() {
        return TV_CAPSULE;
    }

    private static final String TV_CAPSULE = "at 96,54 height 80 text 26.0 gap 0";

    /** Home without the computer manager service: tests report hosts themselves. */
    public static class Home extends ConsoleHomeActivity {
        /** The real check would ask GitHub. */
        @Override protected void checkForUpdates() { }

        /** The PCs a wake packet was asked for; the real sender would use the network. */
        final List<String> woken = Collections.synchronizedList(new ArrayList<>());

        @Override protected void sendWake(ComputerDetails computer) {
            woken.add(computer.uuid);
        }

        @Override public boolean bindService(Intent intent, ServiceConnection connection, int flags) {
            return false;
        }
    }

    /** Lays the whole screen out as a 1920x1080 television would and asks where a D-pad press moves focus. */
    static View focusAfter(Activity screen, View from, int direction) {
        ViewGroup root = (ViewGroup) screen.getWindow().getDecorView();
        root.measure(View.MeasureSpec.makeMeasureSpec(1920, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(1080, View.MeasureSpec.EXACTLY));
        root.layout(0, 0, 1920, 1080);
        return from.focusSearch(direction);
    }

    @Test
    public void homeHasTheShellBarWithHomeCurrentAndNoMenuInItsToolbar() {
        ActivityController<Home> controller = home();
        try {
            ViewGroup bar = controller.get().findViewById(R.id.shell_bar);
            assertNotNull(bar);
            LinearLayout items = bar.findViewById(R.id.shell_bar_items);
            List<Integer> shown = new ArrayList<>();
            List<Integer> selected = new ArrayList<>();
            for (int i = 0; i < items.getChildCount(); i++) {
                View item = items.getChildAt(i);
                if (item.getVisibility() == View.VISIBLE) shown.add(item.getId());
                if (item.isSelected()) selected.add(item.getId());
            }
            assertEquals(Arrays.asList(R.id.shell_home, R.id.shell_pcs, R.id.shell_settings), shown);
            assertEquals(Collections.singletonList(R.id.shell_home), selected);
            MaterialToolbar toolbar = controller.get().findViewById(R.id.console_toolbar);
            assertEquals("Hosts and Settings left the toolbar for the bar", 0, toolbar.getMenu().size());
            assertEquals("the selected tab names the page, no heading over it", View.GONE, toolbar.getVisibility());
            assertEquals("a screen reader still hears the page's name", controller.get().getString(R.string.console_title),
                    String.valueOf(controller.get().getTitle()));
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void aRotationWhileAGameIsAddedKeepsHomeThenHomeFollowsTheNewForm() {
        ActivityController<Home> controller = home();
        try {
            Home screen = controller.get();
            Set<String> adding = ReflectionHelpers.getField(screen, "adding");
            adding.add("tower:7");
            RuntimeEnvironment.setQualifiers("w900dp-h700dp-land");
            screen.onConfigurationChanged(screen.getResources().getConfiguration());
            shadowOf(Looper.getMainLooper()).idle();
            assertFalse("the add still runs on this page", (boolean) ReflectionHelpers.getField(screen, "recreatingForForm"));
            assertFalse(screen.isDestroyed());

            // The add's end asks by itself; nothing else gives the window focus back.
            adding.clear();
            GameOpener.Screen opener = ReflectionHelpers.getField(screen, "opener");
            opener.onAdded();
            shadowOf(Looper.getMainLooper()).idle();
            assertTrue("done: Home is laid out for the handheld", (boolean) ReflectionHelpers.getField(screen, "recreatingForForm"));
        } finally {
            controller.pause().stop().destroy();
        }
    }

    /** A wake held Home through a form change, and nothing asked again when it ended. */
    @Test
    public void aRotationWhileAPcWakesKeepsHomeThenHomeFollowsWhenTheWakeEnds() {
        ActivityController<Home> controller = home();
        try {
            Home screen = controller.get();
            HostActions actions = ReflectionHelpers.getField(screen, "hostActions");
            ComputerDetails attic = new ComputerDetails();
            attic.uuid = "attic";
            attic.name = "Attic";
            attic.state = ComputerDetails.State.OFFLINE;
            attic.macAddress = "02:00:00:00:00:05";
            ReflectionHelpers.callInstanceMethod(actions, "startWake",
                    ReflectionHelpers.ClassParameter.from(ComputerDetails.class, attic),
                    ReflectionHelpers.ClassParameter.from(HostWaker.Sender.class, (HostWaker.Sender) host -> { }),
                    ReflectionHelpers.ClassParameter.from(Executor.class, (Executor) Runnable::run),
                    ReflectionHelpers.ClassParameter.from(LongSupplier.class, (LongSupplier) () -> 0L));
            assertTrue(actions.isWaking());
            RuntimeEnvironment.setQualifiers("w900dp-h700dp-land");
            screen.onConfigurationChanged(screen.getResources().getConfiguration());
            shadowOf(Looper.getMainLooper()).idle();
            assertFalse("the wake still runs on this page", (boolean) ReflectionHelpers.getField(screen, "recreatingForForm"));

            ReflectionHelpers.callInstanceMethod(actions, "endWake");
            shadowOf(Looper.getMainLooper()).idle();
            assertTrue("done: Home is laid out for the handheld", (boolean) ReflectionHelpers.getField(screen, "recreatingForForm"));
        } finally {
            controller.pause().stop().destroy();
        }
    }

    /** A pairing started on Home keeps Home, its pairing and its PIN through a form change; Home follows once it is done. */
    @Test
    public void aRunningPairingKeepsHomeThroughAFormChangeThenHomeFollows() throws Exception {
        ActivityController<Home> controller = home();
        CountDownLatch pollingStopped = new CountDownLatch(1);
        Thread worker = null;
        try {
            Home screen = controller.get();
            HostFeed feed = ReflectionHelpers.getField(screen, "feed");
            ReflectionHelpers.setField(feed, "binder", new ComputerManagerService().new ComputerManagerBinder() {
                @Override public boolean isPersistent() { return false; }
                @Override public void startPolling(ComputerManagerListener listener) { }
                @Override public void stopPolling() { }
                @Override public void invalidateStateForComputer(String uuid) { }
                @Override public String getUniqueId() { return "client"; }
                @Override public void waitForPollingStopped() {
                    try { pollingStopped.await(); }
                    catch (InterruptedException e) { Thread.currentThread().interrupt(); }
                }
            });
            ComputerDetails tower = new ComputerDetails();
            tower.uuid = "tower";
            tower.name = "Tower";
            tower.state = ComputerDetails.State.ONLINE;
            tower.activeAddress = new ComputerDetails.AddressTuple("127.0.0.1", 47989);
            HostActions actions = ReflectionHelpers.getField(screen, "hostActions");
            actions.pair(tower);
            shadowOf(Looper.getMainLooper()).idle();
            worker = ReflectionHelpers.getField(actions, "pairingThread");
            AlertDialog pin = ReflectionHelpers.getField(actions, "pairingDialog");
            assertTrue(pin.isShowing());

            RuntimeEnvironment.setQualifiers("w900dp-h700dp-land");
            screen.onConfigurationChanged(screen.getResources().getConfiguration());
            shadowOf(Looper.getMainLooper()).idle();
            assertFalse("Home waits for the pairing", (boolean) ReflectionHelpers.getField(screen, "recreatingForForm"));
            assertFalse(screen.isDestroyed());
            assertTrue("the pairing still runs", actions.isPairing());
            assertTrue("its PIN still shows", pin.isShowing());

            pin.getButton(AlertDialog.BUTTON_NEGATIVE).performClick();
            shadowOf(Looper.getMainLooper()).idle();
            worker.join(5000);
            assertFalse(actions.isPairing());
            screen.onWindowFocusChanged(true);
            shadowOf(Looper.getMainLooper()).idle();
            assertTrue("done: Home follows the form", (boolean) ReflectionHelpers.getField(screen, "recreatingForForm"));
        } finally {
            pollingStopped.countDown(); controller.pause().stop().destroy(); if (worker != null) worker.join(5000);
        }
    }

    @Test public void onDestroyClosesPairingWatchHandle() {
        ActivityController<Home> controller = home();
        long tracker = ReflectionHelpers.getField(ReflectionHelpers.getField(controller.get(), "pairingWatch"), "tracker");
        assertTrue(tracker > 0);
        controller.pause().stop().destroy();
        assertEquals(0, NightjarPolicyJni.pairingWatchBeginPoll(tracker, 10_000, true));
    }

    @Test public void onAPortraitPhoneTheHeadingsAndCardsStartUnderTheFirstTab() throws Exception { assertHeadingsAndCardsStartUnderTheFirstTab(); }

    @Test @Config(qualifiers = "w891dp-h411dp-land-xxhdpi")
    public void inLandscapeTheHeadingsAndCardsStartUnderTheFirstTab() throws Exception { assertHeadingsAndCardsStartUnderTheFirstTab(); }

    /** In the grid the section headings' text and the cards' edges sit on the screen gutter. */
    private void assertHeadingsAndCardsStartUnderTheFirstTab() throws Exception {
        chooseGrid();
        ActivityController<Home> controller = home();
        try {
            Home home = controller.get();
            showHosts(home, NativeApiFixtures.nativeHost("tower", "Tower", true));
            ViewGroup root = home.findViewById(android.R.id.content);
            android.util.DisplayMetrics metrics = home.getResources().getDisplayMetrics();
            root.measure(View.MeasureSpec.makeMeasureSpec(metrics.widthPixels, View.MeasureSpec.EXACTLY),
                    View.MeasureSpec.makeMeasureSpec(metrics.heightPixels, View.MeasureSpec.EXACTLY));
            root.layout(0, 0, metrics.widthPixels, metrics.heightPixels);
            int tab = home.getResources().getDimensionPixelSize(R.dimen.screen_gutter);
            RecyclerView list = home.findViewById(R.id.console_list);
            int headings = 0;
            int firstCard = Integer.MAX_VALUE;
            for (int i = 0; i < list.getChildCount(); i++) {
                View row = list.getChildAt(i);
                if (row instanceof TextView) {
                    assertEquals("heading " + ((TextView) row).getText(), tab, leftIn(root, row) + row.getPaddingStart());
                    headings++;
                } else {
                    firstCard = Math.min(firstCard, leftIn(root, row));
                }
            }
            assertTrue("a heading was laid out", headings > 0);
            assertEquals("the first card of a row", tab, firstCard);
        } finally {
            controller.pause().stop().destroy();
        }
    }

    private static int leftIn(ViewGroup root, View view) {
        android.graphics.Rect rect = new android.graphics.Rect(0, 0, view.getWidth(), view.getHeight());
        root.offsetDescendantRectToMyCoords(view, rect);
        return rect.left;
    }

    @Test
    public void thePcsButtonOpensTheListOnTopOfHomeAndSettingsOpensFromTheBar() {
        ActivityController<Home> controller = home();
        try {
            Home screen = controller.get();
            screen.findViewById(R.id.shell_pcs).performClick();
            Intent pcs = nextStarted(screen);
            assertEquals(PcView.class.getName(), pcs.getComponent().getClassName());
            assertTrue(pcs.getBooleanExtra(HomeSettings.EXTRA_STAY, false));
            assertFalse("Home is the root and stays under the list", screen.isFinishing());
            screen.findViewById(R.id.shell_settings).performClick();
            Intent settings = nextStarted(screen);
            assertTrue(settings.getBooleanExtra(ShellBar.EXTRA_FROM_SHELL, false));
        } finally {
            controller.pause().stop().destroy();
        }
    }

    /** L1 and R1 open the tab left or right of Home, round at the ends, as the tab's own button would. */
    @Test
    public void theShouldersOpenTheNeighbouringTabs() {
        shouldersOpenTheNeighbouringTabs();
    }

    @Test
    @Config(qualifiers = "television")
    public void onTelevisionTheShouldersOpenTheNeighbouringTabs() {
        shouldersOpenTheNeighbouringTabs();
    }

    private void shouldersOpenTheNeighbouringTabs() {
        ActivityController<Home> controller = home();
        try {
            Home screen = controller.get();
            assertTrue(shoulder(screen, KeyEvent.KEYCODE_BUTTON_R1));
            Intent pcs = nextStarted(screen);
            assertEquals("R1 from Home", PcView.class.getName(), pcs.getComponent().getClassName());
            // A quick second press, before PCs has taken the keys, does not open a tab again.
            assertTrue(shoulder(screen, KeyEvent.KEYCODE_BUTTON_R1));
            assertNull(nextStarted(screen));
            // Back on Home, the shoulders step again.
            controller.pause().resume();
            assertTrue(shoulder(screen, KeyEvent.KEYCODE_BUTTON_L1));
            Intent settings = nextStarted(screen);
            assertTrue("L1 from Home goes round to Settings", settings.getBooleanExtra(ShellBar.EXTRA_FROM_SHELL, false));
            assertEquals(StreamSettings.class.getName(), settings.getComponent().getClassName());
            // A held shoulder opens one tab, not one per repeat.
            screen.dispatchKeyEvent(KeyEvent.changeTimeRepeat(new KeyEvent(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BUTTON_R1), 0, 1));
            assertNull(nextStarted(screen));
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void theShoulderOrderIsTheBarsAndHomeCountsOnlyWhileShown() {
        assertEquals(ShellBar.Destination.PCS, ShellBar.next(ShellBar.Destination.HOME, true, true));
        assertEquals(ShellBar.Destination.SETTINGS, ShellBar.next(ShellBar.Destination.HOME, false, true));
        assertEquals(ShellBar.Destination.HOME, ShellBar.next(ShellBar.Destination.SETTINGS, true, true));
        assertEquals(ShellBar.Destination.HOME, ShellBar.next(ShellBar.Destination.PCS, false, true));
        assertEquals("no Home in the bar", ShellBar.Destination.SETTINGS, ShellBar.next(ShellBar.Destination.PCS, false, false));
        assertEquals(ShellBar.Destination.PCS, ShellBar.next(ShellBar.Destination.SETTINGS, true, false));
    }

    private static boolean shoulder(Activity screen, int keyCode) {
        boolean down = screen.dispatchKeyEvent(new KeyEvent(KeyEvent.ACTION_DOWN, keyCode));
        boolean up = screen.dispatchKeyEvent(new KeyEvent(KeyEvent.ACTION_UP, keyCode));
        shadowOf(Looper.getMainLooper()).idle();
        return down && up;
    }

    @Test
    public void backFromHomeThatIsNotTheRootReturnsToTheListWithoutStacking() {
        PreferenceManager.getDefaultSharedPreferences(context).edit().putBoolean(HomeSettings.PREF_KEY, false).commit();
        ActivityController<Home> controller = home();
        try {
            Home screen = controller.get();
            screen.getOnBackPressedDispatcher().onBackPressed();
            Intent pcs = nextStarted(screen);
            assertEquals(PcView.class.getName(), pcs.getComponent().getClassName());
            assertEquals(Intent.FLAG_ACTIVITY_CLEAR_TOP | Intent.FLAG_ACTIVITY_SINGLE_TOP,
                    pcs.getFlags() & (Intent.FLAG_ACTIVITY_CLEAR_TOP | Intent.FLAG_ACTIVITY_SINGLE_TOP));
            assertTrue(screen.isFinishing());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void everyOpenerOfAHostCardsOptionsShowsTheSameListAsThePcsScreen() throws Exception {
        ActivityController<Home> controller = home();
        try {
            Home screen = controller.get();
            showHosts(screen, nativeHost("tower", "Tower", true));
            // The PCs page's list, with Home's own games filter after Open library.
            List<String> expected = expected(screen, R.string.library_open, R.string.console_filter_only_pc,
                    R.string.folders_title, R.string.files_menu,
                    R.string.pc_menu_settings, R.string.host_menu_rename, R.string.pcview_menu_unpair_pc,
                    R.string.pcview_menu_delete_pc);

            View card = hostCard(screen, "tower");
            card.findViewById(R.id.console_more).performClick();
            ActionSheet sheet = openSheet();
            assertNotNull("the three dots", sheet);
            assertEquals(expected, labels(sheet));
            closeSheet(sheet);

            assertTrue(card.performLongClick());
            sheet = openSheet();
            assertNotNull("a long press", sheet);
            assertEquals(expected, labels(sheet));
            closeSheet(sheet);

            for (int key : new int[]{KeyEvent.KEYCODE_MENU, KeyEvent.KEYCODE_BUTTON_X}) {
                assertTrue(press(card, key));
                sheet = openSheet();
                assertNotNull("key " + key, sheet);
                assertEquals(expected, labels(sheet));
                closeSheet(sheet);
            }

            assertTrue(card.dispatchKeyEvent(new KeyEvent(0, 0, KeyEvent.ACTION_UP, KeyEvent.KEYCODE_F10, 0, KeyEvent.META_SHIFT_ON)));
            sheet = openSheet();
            assertNotNull("Shift+F10", sheet);
            closeSheet(sheet);
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void filesOpensTheFileTransferScreenForThatPc() throws Exception {
        ActivityController<Home> controller = home();
        try {
            Home screen = controller.get();
            showHosts(screen, nativeHost("tower", "Tower", true));
            hostCard(screen, "tower").findViewById(R.id.console_more).performClick();
            ActionSheet sheet = openSheet();
            MaterialButton files = button(sheet.findViewById(android.R.id.content), screen.getString(R.string.files_menu));
            assertNotNull(files);
            files.performClick();
            shadowOf(Looper.getMainLooper()).idle();
            Intent opened = nextStarted(screen);
            assertEquals(FileTransferActivity.class.getName(), opened.getComponent().getClassName());
            assertEquals("tower", opened.getStringExtra(FileTransferActivity.EXTRA_PC_UUID));
        } finally {
            controller.pause().stop().destroy();
        }
    }

    private static MaterialButton button(View view, String label) {
        if (view instanceof MaterialButton && label.equals(((MaterialButton) view).getText().toString())) {
            return (MaterialButton) view;
        }
        if (view instanceof ViewGroup) {
            ViewGroup group = (ViewGroup) view;
            for (int i = 0; i < group.getChildCount(); i++) {
                MaterialButton found = button(group.getChildAt(i), label);
                if (found != null) return found;
            }
        }
        return null;
    }

    @Test
    public void anOfflineHostOffersWakeOnlyWithAMacAddress() throws Exception {
        ActivityController<Home> controller = home();
        try {
            Home screen = controller.get();
            ComputerDetails attic = nativeHost("attic", "Attic", false);
            attic.macAddress = "aa:bb:cc:dd:ee:ff";
            showHosts(screen, attic, nativeHost("loft", "Loft", false));
            hostCard(screen, "attic").findViewById(R.id.console_more).performClick();
            ActionSheet sheet = openSheet();
            // Home offers its games filter on every PC, with no Open library row on an offline one.
            assertEquals(expected(screen, R.string.console_filter_only_pc, R.string.pcview_menu_send_wol,
                    R.string.pc_menu_settings, R.string.host_menu_rename, R.string.pcview_menu_delete_pc), labels(sheet));
            closeSheet(sheet);
            hostCard(screen, "loft").findViewById(R.id.console_more).performClick();
            sheet = openSheet();
            assertEquals(expected(screen, R.string.console_filter_only_pc, R.string.pc_menu_settings,
                    R.string.host_menu_rename, R.string.pcview_menu_delete_pc), labels(sheet));
            closeSheet(sheet);
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void theDpadMarksTheThreeDotsAtTheEndOfTheRowAndCenterOpensThem() throws Exception {
        ActivityController<Home> controller = home();
        try {
            Home screen = controller.get();
            showHosts(screen, nativeHost("tower", "Tower", true));
            View card = hostCard(screen, "tower");
            View more = card.findViewById(R.id.console_more);
            assertFalse("never a focus stop of its own", more.isFocusable());

            assertTrue(press(card, KeyEvent.KEYCODE_DPAD_RIGHT));
            assertTrue("Right marks the dots", more.isHovered());
            assertTrue(press(card, KeyEvent.KEYCODE_DPAD_LEFT));
            assertFalse("Left gives them back", more.isHovered());

            press(card, KeyEvent.KEYCODE_DPAD_RIGHT);
            assertTrue(more.isHovered());
            assertTrue(press(card, KeyEvent.KEYCODE_BACK));
            assertFalse("Back only unmarks", more.isHovered());
            assertNull(openSheet());

            press(card, KeyEvent.KEYCODE_DPAD_RIGHT);
            assertTrue(press(card, KeyEvent.KEYCODE_DPAD_CENTER));
            assertNotNull("Center opens the options", openSheet());
            assertFalse(more.isHovered());
            closeSheet(openSheet());

            // Without a mark, Center is the card's own tap.
            press(card, KeyEvent.KEYCODE_DPAD_CENTER);
            assertNull(openSheet());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void renameAndRemoveFromHomeReachTheComputerManager() throws Exception {
        ActivityController<Home> controller = home();
        try {
            Home screen = controller.get();
            ComputerDetails tower = nativeHost("tower", "Tower", true);
            showHosts(screen, tower);
            HostFeed feed = ReflectionHelpers.getField(screen, "feed");

            hostCard(screen, "tower").findViewById(R.id.console_more).performClick();
            ActionSheet sheet = openSheet();
            findButton(sheet, screen.getString(R.string.host_menu_rename)).performClick();
            shadowOf(Looper.getMainLooper()).idle();
            AlertDialog dialog = (AlertDialog) ShadowDialog.getLatestDialog();
            EditText field = dialog.findViewById(R.id.rename_name);
            field.setText("Living room");
            dialog.getButton(AlertDialog.BUTTON_POSITIVE).performClick();
            shadowOf(Looper.getMainLooper()).idle();
            assertEquals(Collections.singletonList("tower=Living room"), renamed);
            assertEquals("Living room", feed.host("tower").displayName());

            hostCard(screen, "tower").findViewById(R.id.console_more).performClick();
            sheet = openSheet();
            findButton(sheet, screen.getString(R.string.pcview_menu_delete_pc)).performClick();
            shadowOf(Looper.getMainLooper()).idle();
            ((AlertDialog) ShadowDialog.getLatestDialog()).getButton(AlertDialog.BUTTON_POSITIVE).performClick();
            shadowOf(Looper.getMainLooper()).idle();
            assertEquals(1, removed.size());
            assertNull("the host leaves the feed at once", feed.host("tower"));
        } finally {
            controller.pause().stop().destroy();
        }
    }

    private static MaterialButton findButton(ActionSheet sheet, String text) {
        MaterialButton[] found = new MaterialButton[1];
        find(sheet.findViewById(android.R.id.content), text, found);
        assertNotNull("a button " + text, found[0]);
        return found[0];
    }

    private static void find(View view, String text, MaterialButton[] found) {
        if (view instanceof MaterialButton && ((MaterialButton) view).getText().toString().equals(text)) {
            found[0] = (MaterialButton) view;
        } else if (view instanceof ViewGroup) {
            ViewGroup group = (ViewGroup) view;
            for (int i = 0; i < group.getChildCount(); i++) find(group.getChildAt(i), text, found);
        }
    }
}
