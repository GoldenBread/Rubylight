package com.limelight.console;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.content.Context;
import android.content.Intent;
import android.os.Build;
import android.preference.PreferenceManager;
import android.view.View;
import android.widget.TextView;

import com.limelight.PcViewLifecycleTest;
import com.limelight.R;
import com.limelight.nativeapi.ConsoleSnapshot;
import com.limelight.nativeapi.NativeApiFixtures;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.preferences.GlPreferences;

import org.junit.After;
import org.junit.Before;
import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.GraphicsMode;
import org.robolectric.shadows.ShadowLooper;

import java.io.File;
import java.util.Collections;
import java.util.concurrent.TimeUnit;

/**
 * Where a game's page puts Play: on the first screen, whatever the title's length and the text size. Held upright the
 * page is the sheet under the cover; held sideways it is Home's hero, on a screen as short as a phone's.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34, application = Application.class, shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
// Real text metrics: the title's lines and the buttons' heights decide where Play lands.
@GraphicsMode(GraphicsMode.Mode.NATIVE)
public class GameDetailLayoutTest {
    /** Long enough for two lines at the hero's size on every width tested. */
    private static final String LONG_TITLE = "The Legend of Heroes: Trails through Daybreak";

    @Rule public TemporaryFolder temporary = new TemporaryFolder();

    private Context context;

    @Before
    public void setUp() throws Exception {
        context = RuntimeEnvironment.getApplication();
        PreferenceManager.getDefaultSharedPreferences(context).edit().clear().commit();
        GlPreferences gl = GlPreferences.readPreferences(context);
        gl.savedFingerprint = Build.FINGERPRINT;
        gl.glRenderer = "Test renderer";
        gl.writePreferences();
        File directory = new File(context.getFilesDir(), "console-library");
        assertTrue(directory.isDirectory() || directory.mkdirs());
        assertTrue(new File(directory, "tower.json").createNewFile());
    }

    @After
    public void tearDown() {
        RuntimeEnvironment.setFontScale(1f);
        PreferenceManager.getDefaultSharedPreferences(context).edit().clear().commit();
    }

    private GameDetailModel game() throws Exception {
        ConsoleRig rig = new ConsoleRig(temporary.newFolder());
        try {
            rig.transport.reply("tower", "library", NativeApiFixtures.ok(NativeApiFixtures.library("t1",
                    "{\"install_id\":\"t-game\",\"app_id\":11,\"title\":\"" + LONG_TITLE + "\",\"installed\":true,"
                            + "\"store\":{\"source\":\"steam\",\"id\":\"400\"}}")));
            rig.transport.reply("tower", "session", NativeApiFixtures.ok(NativeApiFixtures.IDLE));
            ConsoleSnapshot snapshot = rig.repository.refresh(
                    Collections.singletonList(NativeApiFixtures.nativeHost("tower", "Tower", true)));
            return GameDetailModel.of(snapshot, "tower", 11, LONG_TITLE, "Tower", true, true, true);
        } finally {
            rig.close();
        }
    }

    /** The page with the game rendered and laid out at the screen's size. */
    private GameDetailShellTest.Detail page(ActivityController<GameDetailShellTest.Detail> controller, float fontScale) throws Exception {
        GameDetailShellTest.Detail page = controller.get();
        org.robolectric.util.ReflectionHelpers.setField(page, "covers", new CoverLoaders(page, "test-device", 200));
        ComputerDetails computer = new ComputerDetails();
        computer.uuid = "tower";
        computer.name = "Tower";
        page.render(game(), computer);
        ShadowLooper.idleMainLooper(300, TimeUnit.MILLISECONDS);
        View root = page.getWindow().getDecorView();
        int width = page.getResources().getDisplayMetrics().widthPixels;
        int height = page.getResources().getDisplayMetrics().heightPixels;
        root.measure(View.MeasureSpec.makeMeasureSpec(width, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(height, View.MeasureSpec.EXACTLY));
        root.layout(0, 0, width, height);
        assertEquals(fontScale, page.getResources().getConfiguration().fontScale, 0.001f);
        return page;
    }

    private ActivityController<GameDetailShellTest.Detail> open(float fontScale) {
        RuntimeEnvironment.setFontScale(fontScale);
        Intent intent = new Intent(context, GameDetailShellTest.Detail.class)
                .putExtra(GameDetailActivity.EXTRA_HOST_UUID, "tower")
                .putExtra(GameDetailActivity.EXTRA_APP_ID, 11)
                .putExtra(GameDetailActivity.EXTRA_TITLE, LONG_TITLE)
                .putExtra(GameDetailActivity.EXTRA_HOST_NAME, "Tower");
        return Robolectric.buildActivity(GameDetailShellTest.Detail.class, intent).setup().visible();
    }

    private static int bottomOnScreen(View view) {
        int[] at = new int[2];
        view.getLocationInWindow(at);
        return at[1] + view.getHeight();
    }

    private void assertPlayOnTheFirstScreen(GameDetailShellTest.Detail page) {
        View play = page.findViewById(R.id.detail_play);
        View scroller = (View) page.findViewById(R.id.detail_poster).getParent();
        while (!(scroller instanceof android.widget.ScrollView)) scroller = (View) scroller.getParent();
        assertEquals("nothing is scrolled", 0, scroller.getScrollY());
        assertTrue(play.getHeight() > 0);
        assertTrue("Play ends at " + bottomOnScreen(play) + " px, the page's view at " + bottomOnScreen(scroller) + " px",
                bottomOnScreen(play) <= bottomOnScreen(scroller));
    }

    /**
     * A phone 360 dp across held sideways leaves 336 dp under its status bar. The title takes two lines and the text is
     * at the largest size Android's slider had for years (1.3): Play still shows without scrolling.
     */
    @Test
    @Config(qualifiers = "w740dp-h336dp-land-xhdpi")
    public void sidewaysOnAShortPhonePlayShowsUnderATwoLineTitleInLargeText() throws Exception {
        ActivityController<GameDetailShellTest.Detail> controller = open(1.3f);
        try {
            GameDetailShellTest.Detail page = page(controller, 1.3f);
            TextView title = page.findViewById(R.id.detail_title);
            assertEquals("the title is long enough to wrap", 2, title.getLineCount());
            assertEquals(View.VISIBLE, page.findViewById(R.id.detail_chips).getVisibility());
            assertPlayOnTheFirstScreen(page);
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    @Config(qualifiers = "w740dp-h336dp-land-xhdpi")
    public void sidewaysOnAShortPhonePlayShowsAtTheUsualTextSize() throws Exception {
        ActivityController<GameDetailShellTest.Detail> controller = open(1f);
        try {
            assertPlayOnTheFirstScreen(page(controller, 1f));
        } finally {
            controller.pause().stop().destroy();
        }
    }

    /** Held upright the page is the sheet on the game's ambient: the cover above it, and Play across the sheet on the first screen. */
    @Test
    @Config(qualifiers = "w360dp-h640dp-port-xhdpi")
    public void uprightThePageIsTheSheetUnderTheCoverWithPlayOnTheFirstScreen() throws Exception {
        ActivityController<GameDetailShellTest.Detail> controller = open(1.3f);
        try {
            GameDetailShellTest.Detail page = page(controller, 1.3f);
            assertNotNull("the page sits on the game's ambient", page.findViewById(R.id.detail_backdrop));
            View sheet = page.findViewById(R.id.detail_sheet);
            View cover = page.findViewById(R.id.detail_poster);
            View play = page.findViewById(R.id.detail_play);
            assertNotNull(sheet);
            assertTrue("the cover is above the sheet", bottomOnScreen(cover) <= bottomOnScreen(sheet) - sheet.getHeight());
            assertTrue(play instanceof com.limelight.ui.GlassButton);
            assertEquals("Play spans the sheet", sheet.getWidth() - sheet.getPaddingLeft() - sheet.getPaddingRight(), play.getWidth());
            assertEquals(View.VISIBLE, page.findViewById(R.id.detail_chips).getVisibility());
            assertEquals(View.VISIBLE, page.findViewById(R.id.detail_store).getVisibility());
            assertPlayOnTheFirstScreen(page);
            // The sheet reaches the foot of the page however little it holds.
            View scroller = (View) sheet.getParent().getParent();
            assertTrue(bottomOnScreen(sheet) >= bottomOnScreen(scroller));
        } finally {
            controller.pause().stop().destroy();
        }
    }

    /**
     * The page handles a turn itself (the manifest says so), and its two layouts differ: turned on its side, a phone's page
     * is laid out again as the sideways page, not left as the sheet.
     */
    @Test
    @Config(qualifiers = "w320dp-h560dp-port-xhdpi")
    public void turnedOnItsSideThePageIsLaidOutAgain() throws Exception {
        android.content.pm.ActivityInfo info = new android.content.pm.ActivityInfo();
        info.name = GameDetailShellTest.Detail.class.getName();
        info.packageName = context.getPackageName();
        info.configChanges = android.content.pm.ActivityInfo.CONFIG_ORIENTATION | android.content.pm.ActivityInfo.CONFIG_SCREEN_SIZE
                | android.content.pm.ActivityInfo.CONFIG_SMALLEST_SCREEN_SIZE | android.content.pm.ActivityInfo.CONFIG_SCREEN_LAYOUT;
        org.robolectric.Shadows.shadowOf(context.getPackageManager()).addOrUpdateActivity(info);
        ActivityController<GameDetailShellTest.Detail> controller = open(1f);
        try {
            GameDetailShellTest.Detail page = page(controller, 1f);
            assertNotNull("upright: the sheet", page.findViewById(R.id.detail_sheet));
            assertEquals(android.content.res.Configuration.ORIENTATION_PORTRAIT,
                    (int) org.robolectric.util.ReflectionHelpers.getField(page, "laidOutFor"));
            // Under 600 dp wide it is a phone's form on its side as well: no change of form lays it out again.
            RuntimeEnvironment.setQualifiers("+w560dp-h320dp-land");
            assertEquals(com.limelight.ui.GlassForm.PHONE, com.limelight.ui.GlassForm.of(RuntimeEnvironment.getApplication()));
            // Play waits for the PC to wake: laying the page out again would end that, so the turn waits for it.
            org.robolectric.util.ReflectionHelpers.setField(page, "playWhenReady", true);
            controller.configurationChange(RuntimeEnvironment.getApplication().getResources().getConfiguration());
            ShadowLooper.idleMainLooper(300, TimeUnit.MILLISECONDS);
            assertEquals("the wait holds the page as it is", android.content.res.Configuration.ORIENTATION_PORTRAIT,
                    (int) org.robolectric.util.ReflectionHelpers.getField(page, "laidOutFor"));
            assertTrue("and still runs", (boolean) org.robolectric.util.ReflectionHelpers.getField(page, "playWhenReady"));
            org.robolectric.util.ReflectionHelpers.callInstanceMethod(page, "cancelWake");
            ShadowLooper.idleMainLooper(300, TimeUnit.MILLISECONDS);
            assertEquals("it asked to be laid out for the side", android.content.res.Configuration.ORIENTATION_LANDSCAPE,
                    (int) org.robolectric.util.ReflectionHelpers.getField(page, "laidOutFor"));
        } finally {
            controller.pause().stop().destroy();
        }
    }
}
