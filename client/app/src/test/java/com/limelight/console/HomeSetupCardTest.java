package com.limelight.console;

import static com.limelight.nativeapi.NativeApiFixtures.nativeHost;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.content.Context;
import android.os.Build;
import android.preference.PreferenceManager;
import android.view.View;
import android.widget.TextView;

import com.limelight.PcViewLifecycleTest;
import com.limelight.R;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.preferences.GlPreferences;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.preferences.SimpleProfileModel;

import org.junit.After;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;

import java.io.File;

/**
 * Home's set-up card: after the first pairing it says what the automatic settings will use for this device and that
 * PC, has nothing to press but its close button, and never returns once closed.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class, shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class HomeSetupCardTest {
    private Context context;

    @Before
    public void setUp() throws Exception {
        context = RuntimeEnvironment.getApplication();
        PreferenceManager.getDefaultSharedPreferences(context).edit().clear().commit();
        context.getSharedPreferences("home_setup_card", Context.MODE_PRIVATE).edit().clear().commit();
        GlPreferences gl = GlPreferences.readPreferences(context);
        gl.savedFingerprint = Build.FINGERPRINT;
        gl.glRenderer = "Test renderer";
        gl.writePreferences();
        File directory = new File(context.getFilesDir(), "console-library");
        assertTrue(directory.isDirectory() || directory.mkdirs());
        assertTrue(new File(directory, "tower.json").createNewFile());
        settingsAre("auto", "auto", "auto");
    }

    @After
    public void tearDown() {
        // Back to the engine a JVM test does not have: every row reads as Custom again.
        SimpleProfileModel.use(null);
        PreferenceManager.getDefaultSharedPreferences(context).edit().clear().commit();
        context.getSharedPreferences("home_setup_card", Context.MODE_PRIVATE).edit().clear().commit();
    }

    /** What the simple layer's model says the stream's settings are; null for Custom. */
    private static void settingsAre(String smoothness, String picture, String hdr) {
        String choices = smoothness == null ? "null" : "{\"smoothness\":\"" + smoothness + "\",\"picture\":\"" + picture
                + "\",\"hdr\":\"" + hdr + "\"}";
        SimpleProfileModel.use(request -> "{\"ok\":{\"choices\":" + choices + "}}");
    }

    private ActivityController<HomeShellTest.Home> home() {
        return Robolectric.buildActivity(HomeShellTest.Home.class).setup().visible();
    }

    /** Home once the feed has reported the paired PC. */
    private static View cardAfter(HomeShellTest.Home home, ComputerDetails... hosts) {
        HostFeed feed = ReflectionHelpers.getField(home, "feed");
        for (ComputerDetails host : hosts) feed.update(host);
        home.onHostsChanged();
        return home.findViewById(R.id.home_setup_card);
    }

    private static String text(View card, int id) {
        return ((TextView) card.findViewById(id)).getText().toString();
    }

    @Test
    public void onlyTheFirstPairingOffersTheCard() {
        SetupCard.offer(context, "tower", 0);
        assertEquals("tower", SetupCard.host(context));
        SetupCard.offer(context, "laptop", 0);
        assertEquals("a later pairing does not take the card over", "tower", SetupCard.host(context));
        SetupCard.dismiss(context);
        assertNull(SetupCard.host(context));
        SetupCard.offer(context, "laptop", 0);
        assertNull("closed once, it never returns", SetupCard.host(context));
    }

    @Test
    public void aPairingWithAnotherPcAlreadyPairedIsNotTheFirst() {
        SetupCard.offer(context, "laptop", 1);
        assertNull(SetupCard.host(context));
        SetupCard.offer(context, "tower", 0);
        assertNull("and no later pairing is the first either", SetupCard.host(context));
    }

    @Test
    public void theLineNamesTheSizeTheRateHdrAndTheCodec() {
        String auto = SetupCard.values(context, 2560, 1440, 120, 60000, true, PreferenceConfiguration.FormatOption.AUTO);
        assertEquals(StreamProfileSummary.values(context, 2560, 1440, 120, 60000) + " · HDR · "
                + context.getString(R.string.home_setup_codec_auto), auto);
        assertTrue(auto, auto.contains("2560") && auto.contains("1440") && auto.contains("120") && auto.contains("60"));
        String forced = SetupCard.values(context, 1920, 1080, 60, 20000, false, PreferenceConfiguration.FormatOption.FORCE_HEVC);
        assertTrue(forced, forced.endsWith(" · SDR · HEVC"));
        assertTrue(SetupCard.values(context, 1920, 1080, 60, 20000, false, PreferenceConfiguration.FormatOption.FORCE_AV1)
                .endsWith(" · SDR · AV1"));
    }

    @Test
    public void homeShowsTheCardForThePcOnceItIsAmongTheHostsAndClosingItIsForGood() {
        SetupCard.offer(context, "tower", 0);
        ActivityController<HomeShellTest.Home> controller = home();
        try {
            HomeShellTest.Home home = controller.get();
            View card = home.findViewById(R.id.home_setup_card);
            assertEquals("its PC has not been reported yet", View.GONE, card.getVisibility());
            cardAfter(home, nativeHost("tower", "Tower", true));
            assertEquals(View.VISIBLE, card.getVisibility());
            assertEquals(context.getString(R.string.home_setup_title, "Tower"), text(card, R.id.home_setup_title));
            PreferenceConfiguration config = PreferenceConfiguration.readPreferences(context, "tower", null, null, 0, null);
            String values = text(card, R.id.home_setup_values);
            assertTrue(values, values.startsWith(StreamProfileSummary.values(context, config.width, config.height, config.fps, config.bitrate)));
            assertTrue(values, values.endsWith(context.getString(R.string.home_setup_codec_auto)));
            // Nothing to press but the close button: the text is no button, and a screen reader hears it as one stop.
            assertFalse(card.isClickable());
            assertFalse(card.findViewById(R.id.home_setup_text).isClickable());
            assertEquals(text(card, R.id.home_setup_title) + ". " + values,
                    card.findViewById(R.id.home_setup_text).getContentDescription().toString());
            View close = card.findViewById(R.id.home_setup_dismiss);
            assertTrue(close.isFocusable());
            assertEquals(context.getString(R.string.home_setup_dismiss), close.getContentDescription().toString());

            close.performClick();
            assertEquals(View.GONE, card.getVisibility());
            assertNull(SetupCard.host(context));
        } finally {
            controller.pause().stop().destroy();
        }
        ActivityController<HomeShellTest.Home> again = home();
        try {
            assertEquals(View.GONE, cardAfter(again.get(), nativeHost("tower", "Tower", true)).getVisibility());
        } finally {
            again.pause().stop().destroy();
        }
    }

    @Test
    public void theCardSpeaksForAutoOnlyAndComesBackWhenTheSettingsAreAutoAgain() {
        SetupCard.offer(context, "tower", 0);
        settingsAre(null, null, null);
        ActivityController<HomeShellTest.Home> controller = home();
        try {
            HomeShellTest.Home home = controller.get();
            View card = cardAfter(home, nativeHost("tower", "Tower", true));
            assertEquals("Custom settings are the player's own: the card would claim them for Auto", View.GONE, card.getVisibility());
            settingsAre("auto", "best", "auto");
            controller.pause().resume();
            assertEquals("a choice other than Auto is the player's as well", View.GONE, card.getVisibility());
            assertEquals("hidden, not closed", "tower", SetupCard.host(context));
            settingsAre("auto", "auto", "auto");
            controller.pause().resume();
            assertEquals(View.VISIBLE, card.getVisibility());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void noCardWithoutAFirstPairingAndNoneForAPcThatIsNotPaired() {
        ActivityController<HomeShellTest.Home> controller = home();
        try {
            assertEquals(View.GONE, cardAfter(controller.get(), nativeHost("tower", "Tower", true)).getVisibility());
        } finally {
            controller.pause().stop().destroy();
        }
        SetupCard.offer(context, "gone", 0);
        ActivityController<HomeShellTest.Home> other = home();
        try {
            assertEquals(View.GONE, cardAfter(other.get(), nativeHost("tower", "Tower", true)).getVisibility());
        } finally {
            other.pause().stop().destroy();
        }
    }

    /** The card is the page's first content: the list starts beneath it while it shows, and where it did once it is closed. */
    @Test
    public void theListStartsBeneathTheCardAndReturnsWhenItIsClosed() {
        SetupCard.offer(context, "tower", 0);
        ActivityController<HomeShellTest.Home> controller = home();
        try {
            HomeShellTest.Home home = controller.get();
            View list = home.findViewById(R.id.console_list);
            HomeChrome chrome = ReflectionHelpers.getField(home, "chrome");
            org.robolectric.Shadows.shadowOf(android.os.Looper.getMainLooper()).idle();
            int top = list.getPaddingTop();
            assertEquals(0, chrome.setupCardRoom());
            View card = cardAfter(home, nativeHost("tower", "Tower", true));
            org.robolectric.Shadows.shadowOf(android.os.Looper.getMainLooper()).idle();
            assertTrue("the card is laid out", card.getHeight() > 0);
            assertEquals("under the top row, where the list begins", top, card.getTop());
            assertTrue("the first row leaves the card's height and a gap: " + chrome.setupCardRoom(),
                    chrome.setupCardRoom() > card.getHeight());
            assertEquals("the list's own top and its fade stay", top, list.getPaddingTop());
            assertEquals(0f, card.getTranslationY(), 0f);
            card.findViewById(R.id.home_setup_dismiss).performClick();
            org.robolectric.Shadows.shadowOf(android.os.Looper.getMainLooper()).idle();
            assertEquals(0, chrome.setupCardRoom());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    /** The card leaves as far as the list's first row has risen from its place at rest, and wholly behind the search. */
    @Test
    public void theCardLeavesWithTheFirstRow() {
        View first = new View(context);
        first.layout(0, 100, 10, 200);
        assertEquals(0f, HomeChrome.cardAway(false, 3, first, 100, 80), 0f);
        assertEquals("a row that starts lower is at rest too", 0f, HomeChrome.cardAway(false, 3, first, 90, 80), 0f);
        assertEquals(30f, HomeChrome.cardAway(false, 3, first, 130, 80), 0f);
        assertEquals("never farther than its height", 80f, HomeChrome.cardAway(false, 3, first, 400, 80), 0f);
        assertEquals("the first row is recycled", 80f, HomeChrome.cardAway(false, 3, null, 100, 80), 0f);
        assertEquals("an empty Home keeps it", 0f, HomeChrome.cardAway(false, 0, null, 100, 80), 0f);
        assertEquals(80f, HomeChrome.cardAway(true, 3, first, 100, 80), 0f);
    }

    /** On a TV the card stands under the top row's right end, the hero's words end before it, and the remote reaches its close button. */
    @Test
    @Config(qualifiers = "w960dp-h540dp-land-television-xhdpi")
    public void onATvTheCardShowsAndItsCloseButtonTakesFocus() {
        SetupCard.offer(context, "tower", 0);
        ActivityController<HomeShellTest.Home> controller = home();
        try {
            HomeShellTest.Home home = controller.get();
            View card = cardAfter(home, nativeHost("tower", "Tower", true));
            assertEquals(View.VISIBLE, card.getVisibility());
            org.robolectric.Shadows.shadowOf(android.os.Looper.getMainLooper()).idle();
            View hero = home.findViewById(R.id.home_hero);
            View title = home.findViewById(R.id.home_hero_title);
            assertTrue("the hero's title ends before the card: " + title.getRight() + " " + card.getLeft(),
                    card.getWidth() > 0 && hero.getLeft() + title.getRight() < card.getLeft());
            View close = card.findViewById(R.id.home_setup_dismiss);
            assertTrue(close.isFocusable());
            close.performClick();
            assertEquals(View.GONE, card.getVisibility());
            org.robolectric.Shadows.shadowOf(android.os.Looper.getMainLooper()).idle();
            assertTrue("and has its width back", hero.getLeft() + title.getRight() > card.getLeft());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    /** A device that comes from a version before the card has its PC paired already: pairing it again offers nothing. */
    @Test
    public void aDeviceThatAlreadyHasAPairedPcNeverGetsTheCard() throws Exception {
        ComputerDetails paired = new ComputerDetails();
        paired.uuid = "tower";
        paired.name = "Tower";
        paired.serverCert = com.limelight.nativeapi.NativeApiFixtures.certificate();
        com.limelight.computers.ComputerDatabaseManager database = new com.limelight.computers.ComputerDatabaseManager(context);
        try {
            database.updateComputer(paired);
        } finally {
            database.close();
        }
        try {
            SetupCard.atStart(context);
            SetupCard.offer(context, "tower", 0);
            assertNull("its only PC paired again is no first pairing", SetupCard.host(context));
        } finally {
            context.deleteDatabase("computers4.db");
            for (String name : context.databaseList()) context.deleteDatabase(name);
        }
    }

    /** A fresh install starts with nothing paired: the start marks nothing, and the first pairing offers the card. */
    @Test
    public void aFreshInstallStillGetsTheCardWithItsFirstPairing() {
        SetupCard.atStart(context);
        SetupCard.offer(context, "tower", 0);
        assertEquals("tower", SetupCard.host(context));
    }

    /** The card's PC is unpaired or removed while Home shows: the card goes with it, and is back if the PC is. */
    @Test
    public void theCardGoesWhenItsPcIsNoLongerPaired() {
        SetupCard.offer(context, "tower", 0);
        ActivityController<HomeShellTest.Home> controller = home();
        try {
            HomeShellTest.Home home = controller.get();
            View card = cardAfter(home, nativeHost("tower", "Tower", true));
            assertEquals(View.VISIBLE, card.getVisibility());
            ComputerDetails unpaired = nativeHost("tower", "Tower", true);
            unpaired.pairState = com.limelight.nvstream.http.PairingManager.PairState.NOT_PAIRED;
            unpaired.serverCert = null;
            cardAfter(home, unpaired);
            assertEquals("its PC is not paired any more", View.GONE, card.getVisibility());
            assertEquals("hidden, not closed", "tower", SetupCard.host(context));
            cardAfter(home, nativeHost("tower", "Tower", true));
            assertEquals(View.VISIBLE, card.getVisibility());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    /** With Custom settings the card stays away, and a poll does not ask the settings' model again: a resume does. */
    @Test
    public void aPollDoesNotReadTheSettingsAgainWhileTheCardIsHidden() {
        SetupCard.offer(context, "tower", 0);
        int[] asked = {0};
        SimpleProfileModel.use(request -> {
            asked[0]++;
            return "{\"ok\":{\"choices\":null}}";
        });
        ActivityController<HomeShellTest.Home> controller = home();
        try {
            HomeShellTest.Home home = controller.get();
            View card = cardAfter(home, nativeHost("tower", "Tower", true));
            assertEquals(View.GONE, card.getVisibility());
            int before = asked[0];
            assertTrue("the PC's arrival read them once", before > 0);
            for (int i = 0; i < 5; i++) home.onHostsChanged();
            assertEquals(before, asked[0]);
            controller.pause().resume();
            assertTrue("a resume reads them: they are changed on other screens", asked[0] > before);
        } finally {
            controller.pause().stop().destroy();
        }
    }

    /** HDR left on Auto reads as Recommended's row reads it: on, where the screen and the decoder show HDR. */
    @Test
    public void hdrOnAutoIsHdrWhereTheScreenAndTheDecoderShowIt() {
        ComputerDetails tower = nativeHost("tower", "Tower", true);
        try {
            ReflectionHelpers.setStaticField(com.limelight.preferences.SimpleProfile.class, "cachedCapabilities",
                    new SimpleProfileModel.Capabilities(1920, 1080, 60, true, false, true, true, false, true, null, null, null));
            assertTrue(SetupCard.text(context, tower).values, SetupCard.text(context, tower).values.contains(" · HDR · "));
            ReflectionHelpers.setStaticField(com.limelight.preferences.SimpleProfile.class, "cachedCapabilities",
                    new SimpleProfileModel.Capabilities(1920, 1080, 60, false, false, true, true, false, true, null, null, null));
            assertTrue(SetupCard.text(context, tower).values, SetupCard.text(context, tower).values.contains(" · SDR · "));
        } finally {
            ReflectionHelpers.setStaticField(com.limelight.preferences.SimpleProfile.class, "cachedCapabilities", null);
        }
    }

    /** Up from the list's first row, whatever its column, stops on the card's close button; Up again goes on to the top row. */
    private void upFromTheFirstRowReachesTheCloseButton() {
        SetupCard.offer(context, "tower", 0);
        ActivityController<HomeShellTest.Home> controller = home();
        try {
            HomeShellTest.Home home = controller.get();
            View card = cardAfter(home, nativeHost("tower", "Tower", true));
            org.robolectric.Shadows.shadowOf(android.os.Looper.getMainLooper()).idle();
            assertEquals(View.VISIBLE, card.getVisibility());
            android.view.ViewGroup list = home.findViewById(R.id.console_list);
            java.util.ArrayList<View> stops = list.getFocusables(View.FOCUS_DOWN);
            assertFalse("the list has something to stand on", stops.isEmpty());
            // The leftmost stop of the first row: straight above it are the tabs, not the button at the card's far end.
            View first = stops.get(0);
            for (View stop : stops) {
                int[] at = new int[2], best = new int[2];
                stop.getLocationInWindow(at);
                first.getLocationInWindow(best);
                if (at[1] < best[1] || (at[1] == best[1] && at[0] < best[0])) first = stop;
            }
            assertTrue(first.requestFocus());
            View close = card.findViewById(R.id.home_setup_dismiss);
            android.view.KeyEvent up = new android.view.KeyEvent(android.view.KeyEvent.ACTION_DOWN, android.view.KeyEvent.KEYCODE_DPAD_UP);
            assertTrue("Up from " + first + " is used", home.dispatchKeyEvent(up));
            assertTrue("and stands on the close button", close.isFocused());
            // From the button the press is the system's own: nothing here holds it back from the top row.
            View above = close.focusSearch(View.FOCUS_UP);
            assertTrue("above the button is the top row: " + above, above != null && above != close && !inside(list, above));
            assertTrue("Down from the button is used", home.dispatchKeyEvent(
                    new android.view.KeyEvent(android.view.KeyEvent.ACTION_DOWN, android.view.KeyEvent.KEYCODE_DPAD_DOWN)));
            View below = home.getWindow().getDecorView().findFocus();
            assertTrue("and is back in the list: " + below, below != null && inside(list, below));
        } finally {
            controller.pause().stop().destroy();
        }
    }

    private static boolean inside(View parent, View child) {
        for (View at = child; at != null; at = at.getParent() instanceof View ? (View) at.getParent() : null) {
            if (at == parent) return true;
        }
        return false;
    }

    @Test
    public void onAHandheldUpFromTheFirstRowReachesTheCloseButton() {
        upFromTheFirstRowReachesTheCloseButton();
    }

    @Test
    @Config(qualifiers = "w960dp-h540dp-land-television-xhdpi")
    public void onATvUpFromTheFirstRowReachesTheCloseButton() {
        upFromTheFirstRowReachesTheCloseButton();
    }
}
