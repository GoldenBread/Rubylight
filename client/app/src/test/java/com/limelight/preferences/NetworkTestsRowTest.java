package com.limelight.preferences;

import android.app.Application;
import android.content.Intent;
import android.os.Looper;
import android.preference.Preference;
import android.preference.PreferenceGroup;
import android.widget.ListView;

import androidx.appcompat.app.AlertDialog;

import com.limelight.R;
import com.limelight.computers.ComputerDatabaseManager;
import com.limelight.computers.HostNames;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.TestHosts;

import org.junit.After;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.Shadows;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowDialog;

import java.util.ArrayList;
import java.util.List;

import static org.junit.Assert.*;

/**
 * Settings' Connection test row: a saved PC's connection tests, which live on that PC's settings screen, found from
 * Streaming and from search.
 */
@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class NetworkTestsRowTest {
    private final Application app = RuntimeEnvironment.getApplication();

    @After
    public void forget() {
        ComputerDatabaseManager database = new ComputerDatabaseManager(app);
        try {
            for (ComputerDetails pc : database.getAllComputers()) database.deleteComputer(pc);
        } finally {
            database.close();
        }
    }

    private void save(String uuid, String name) {
        ComputerDetails pc = new ComputerDetails();
        pc.uuid = uuid;
        pc.name = name;
        ComputerDatabaseManager database = new ComputerDatabaseManager(app);
        try {
            assertTrue(database.updateComputer(pc));
        } finally {
            database.close();
        }
    }

    /** Settings that read the saved PCs at once, on the test's thread, instead of on a thread of their own. */
    private static ActivityController<StreamSettings> open() {
        ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).create();
        controller.get().savedPcReader = Runnable::run;
        return controller.start().postCreate(null).resume().visible();
    }

    /** The row in the Streaming section, with the network rows behind its More row open. */
    private static Preference row(StreamSettings screen) {
        SettingsStructureTest.settle(screen);
        return SettingsStructureTest.open(screen, "streaming").findPreference(StreamSettings.NETWORK_TESTS_KEY);
    }

    private static void click(Preference preference) {
        preference.getOnPreferenceClickListener().onPreferenceClick(preference);
        Shadows.shadowOf(Looper.getMainLooper()).idle();
    }

    @Test
    public void streamingShowsTheRowBesideTheNetworkSettings() {
        save("one-pc", "Den PC");
        try (ActivityController<StreamSettings> controller = open()) {
            Preference row = row(controller.get());
            assertNotNull(row);
            assertEquals(app.getString(R.string.network_tests_title), row.getTitle().toString());
            assertTrue(row.isEnabled());
            // Its hint; the PyroWave frame pace has its own row in Debug › Diagnostics.
            assertEquals(app.getString(R.string.summary_network_tests), row.getSummary().toString());
        }
    }

    @Test
    public void itIsAMainRowThatShowsWithStreamingFolded() {
        save("one-pc", "Den PC");
        try (ActivityController<StreamSettings> controller = open()) {
            StreamSettings screen = controller.get();
            assertFalse(StreamSettings.foldsInSection(screen, "streaming", StreamSettings.NETWORK_TESTS_KEY));
            SettingsStructureTest.settle(screen);
            screen.moreOpen.remove("streaming");
            screen.showSection("streaming");
            SettingsStructureTest.settle(screen);
            StreamSettings.SettingsFragment folded = (StreamSettings.SettingsFragment) SettingsStructureTest.shown(screen);
            Preference row = folded.findPreference(StreamSettings.NETWORK_TESTS_KEY);
            // A folded row is left off the screen until More opens.
            assertNotNull("shows before More Streaming settings opens", row);
        }
    }

    @Test
    public void withoutASavedPcTheRowIsHidden() {
        try (ActivityController<StreamSettings> controller = open()) {
            StreamSettings screen = controller.get();
            StreamSettings.SettingsFragment streaming = SettingsStructureTest.open(screen, "streaming");
            android.preference.PreferenceGroup main = (android.preference.PreferenceGroup) streaming.findPreference("category_streaming_main");
            assertFalse(SettingsStructureTest.keys(main).contains(StreamSettings.NETWORK_TESTS_KEY));
        }
    }

    @Test
    public void searchFindsItByItsTitleAndWhatItMeasures() {
        save("one-pc", "Den PC");
        try (ActivityController<StreamSettings> controller = open()) {
            StreamSettings screen = controller.get();
            SettingsStructureTest.settle(screen);
            // Its title and its hint, which names what it measures.
            for (String query : new String[] {"Connection test", "jitter", "download speed"}) {
                StreamSettings.SettingsFragment results = SettingsStructureTest.search(screen, query);
                assertNotNull(query, SettingsStructureTest.resultGroup(results, StreamSettings.NETWORK_TESTS_KEY));
            }
        }
    }

    @Test
    public void oneSavedPcOpensItsTestsAtOnce() {
        save("one-pc", "Den PC");
        try (ActivityController<StreamSettings> controller = open()) {
            StreamSettings screen = controller.get();
            click(row(screen));
            assertNull("no picker for one PC", ShadowDialog.getLatestDialog());
            Intent opened = Shadows.shadowOf(screen).getNextStartedActivity();
            assertEquals(HostStreamSettings.class.getName(), opened.getComponent().getClassName());
            assertEquals("one-pc", opened.getStringExtra("host_uuid"));
            assertEquals(HostStreamSettings.NETWORK_TESTS_ROW, opened.getStringExtra(HostStreamSettings.EXTRA_FOCUS_KEY));
        }
    }

    @Test
    public void severalSavedPcsAskWhichFirstByTheirNames() {
        save("pc-zed", "Zed");
        save("pc-den", "Den PC");
        HostNames.set(app, "pc-zed", "Attic");
        try (ActivityController<StreamSettings> controller = open()) {
            StreamSettings screen = controller.get();
            click(row(screen));
            assertNull(Shadows.shadowOf(screen).getNextStartedActivity());
            AlertDialog picker = (AlertDialog) ShadowDialog.getLatestDialog();
            assertNotNull(picker);
            ListView list = picker.getListView();
            assertEquals(2, list.getAdapter().getCount());
            // The user's own name for a PC, in name order.
            assertEquals("Attic", list.getAdapter().getItem(0).toString());
            assertEquals("Den PC", list.getAdapter().getItem(1).toString());
            list.performItemClick(null, 1, 1);
            Shadows.shadowOf(Looper.getMainLooper()).idle();
            Intent opened = Shadows.shadowOf(screen).getNextStartedActivity();
            assertEquals("pc-den", opened.getStringExtra("host_uuid"));
            assertEquals(HostStreamSettings.NETWORK_TESTS_ROW, opened.getStringExtra(HostStreamSettings.EXTRA_FOCUS_KEY));
        } finally {
            HostNames.forget(app, "pc-zed");
        }
    }

    @Test
    public void theSavedPcsAreReadOffTheUiThreadAndAnAnswerAfterTheScreenIsGoneChangesNothing() {
        save("one-pc", "Den PC");
        List<Runnable> reads = new ArrayList<>();
        ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).create();
        controller.get().savedPcReader = reads::add;
        controller.start().postCreate(null).resume().visible();
        StreamSettings screen = controller.get();
        StreamSettings.SettingsFragment streaming = SettingsStructureTest.open(screen, "streaming");
        PreferenceGroup main = (PreferenceGroup) streaming.findPreference("category_streaming_main");
        assertFalse("the screen asked for the saved PCs without waiting for them", reads.isEmpty());
        assertFalse("the row waits for the answer", SettingsStructureTest.keys(main).contains(StreamSettings.NETWORK_TESTS_KEY));
        for (Runnable read : new ArrayList<>(reads)) read.run();
        Shadows.shadowOf(Looper.getMainLooper()).idle();
        assertTrue("the answer shows it", SettingsStructureTest.keys(main).contains(StreamSettings.NETWORK_TESTS_KEY));

        reads.clear();
        controller.pause().resume();
        assertFalse("each return to the screen reads again", reads.isEmpty());
        controller.pause().stop().destroy();
        // The read ends after the screen is gone: nothing is touched, nothing throws.
        for (Runnable read : new ArrayList<>(reads)) read.run();
        Shadows.shadowOf(Looper.getMainLooper()).idle();
    }

    @Test
    @Config(qualifiers = "w360dp-h400dp")
    public void thePcsSettingsOpenAtItsTests() {
        HostCapabilities_save();
        Intent intent = new Intent(app, HostStreamSettings.class).putExtra("host_uuid", "tests-pc")
                .putExtra("host_name", "Den PC")
                .putExtra(HostStreamSettings.EXTRA_FOCUS_KEY, HostStreamSettings.NETWORK_TESTS_ROW);
        ActivityController<HostStreamSettings> controller = Robolectric.buildActivity(HostStreamSettings.class, intent).setup().visible();
        try {
            Shadows.shadowOf(Looper.getMainLooper()).idle();
            HostStreamSettings screen = controller.get();
            ListView list = screen.findViewById(android.R.id.list);
            int position = StreamSettings.RootFragment.positionOf(list, HostStreamSettings.NETWORK_TESTS_ROW);
            assertTrue("the row exists", position > 0);
            assertTrue("the list starts at the tests, not at the top: first " + list.getFirstVisiblePosition(),
                    list.getFirstVisiblePosition() > 0
                            && position >= list.getFirstVisiblePosition() && position <= list.getLastVisiblePosition());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    private void HostCapabilities_save() {
        com.limelight.computers.HostCapabilityCache.save(app, "tests-pc", TestHosts.rubylight());
    }
}
