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
}
