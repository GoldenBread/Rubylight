package com.limelight;

import android.app.Application;
import android.os.Looper;
import android.widget.GridView;
import android.widget.TextView;

import androidx.appcompat.app.AlertDialog;

import com.limelight.PcViewLifecycleTest.Library;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.PairingManager;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.Shadows;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;

import java.util.concurrent.CountDownLatch;

import static com.limelight.PcViewTestSupport.*;
import static org.junit.Assert.*;

/**
 * Tests {@link PcView} gamepad/D-pad focus restoration, TV overscan/tile layout, selection rings,
 * footer navigation, and options hints.
 */
@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class, shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class PcViewFocusAndTvTest {
    private final PcViewTestSupport support = new PcViewTestSupport();

    @Test
    public void backFromAStreamAPadDroveThePcThatRunsTheGameHoldsFocus() {
        // A real stream on the Retroid: after Disconnect no card had focus, so a pad or a remote could not act at once.
        ActivityController<Library> controller = support.pairingScreen(new CountDownLatch(0), 47989);
        try {
            Library screen = controller.get();
            support.report(pc("ready", "Office", ComputerDetails.State.ONLINE, PairingManager.PairState.PAIRED));
            ComputerDetails running = pc("running", "Den", ComputerDetails.State.ONLINE, PairingManager.PairState.PAIRED);
            running.runningGameId = 7;
            support.report(running);
            GridView grid = screen.findViewById(R.id.fragmentView);
            assertEquals("Den", ((PcView.ComputerObject) grid.getItemAtPosition(0)).details.name);
            grid.setSelection(1);

            // The stream: this screen goes away, the pad leaves touch mode, and it comes back with focus off the list.
            controller.pause().stop();
            com.limelight.ui.TvScreensPreviewTest.leaveTouchMode(screen);
            assertTrue(screen.findViewById(R.id.shell_bar).requestFocus());
            assertFalse("the list starts without focus", grid.hasFocus());
            // This test's manager knows no PCs, so a resume with it would drop them; the real one keeps them.
            Object manager = ReflectionHelpers.getField(screen, "managerBinder");
            ReflectionHelpers.setField(screen, "managerBinder", null);
            controller.restart().resume();
            ReflectionHelpers.setField(screen, "managerBinder", manager);
            assertEquals(3, grid.getCount());
            screen.onWindowFocusChanged(true);
            Shadows.shadowOf(Looper.getMainLooper()).idle();

            assertTrue("the list holds focus", grid.hasFocus());
            assertFalse(grid.isInTouchMode());
            assertEquals("the PC that runs the game", "Den",
                    ((PcView.ComputerObject) grid.getSelectedItem()).details.name);
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void backFromAPadsDisconnectInTouchModeThePcThatRunsTheGameShowsTheRing() {
        // On the Retroid this window kept the touch mode of the tap that started the stream, the list focused with nothing
        // selected: no ring until a first pad press, which only brought the ring.
        ActivityController<Library> controller = support.pairingScreen(new CountDownLatch(0), 47989);
        try {
            Library screen = controller.get();
            support.report(pc("ready", "Office", ComputerDetails.State.ONLINE, PairingManager.PairState.PAIRED));
            ComputerDetails running = pc("running", "Den", ComputerDetails.State.ONLINE, PairingManager.PairState.PAIRED);
            running.runningGameId = 7;
            support.report(running);
            GridView grid = screen.findViewById(R.id.fragmentView);
            controller.pause().stop();
            // The stream menu's sheet saw the pad's A on Disconnect.
            com.limelight.ui.InputSource.remember(com.limelight.ui.InputSource.PAD);
            Object manager = ReflectionHelpers.getField(screen, "managerBinder");
            ReflectionHelpers.setField(screen, "managerBinder", null);
            controller.restart().resume();
            ReflectionHelpers.setField(screen, "managerBinder", manager);
            Object root = ReflectionHelpers.callInstanceMethod(screen.getWindow().getDecorView(), "getViewRootImpl");
            ReflectionHelpers.callInstanceMethod(root, "ensureTouchMode", ReflectionHelpers.ClassParameter.from(boolean.class, true));
            grid.requestFocus();
            assertTrue(grid.isInTouchMode());
            assertTrue(grid.hasFocus());
            screen.onWindowFocusChanged(true);
            Shadows.shadowOf(Looper.getMainLooper()).idle();

            assertFalse("the ring shows", grid.isInTouchMode());
            assertTrue(grid.hasFocus());
            assertEquals("Den", ((PcView.ComputerObject) grid.getSelectedItem()).details.name);
        } finally {
            com.limelight.ui.InputSource.remember(null);
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void rightFromThePcCardStepsOntoItsOverflowButtonThenOntoAddPc() {
        rightStepsOntoTheOverflowButtonThenOntoAddPc();
    }

    /** A TV's PC tile (layout-television/pc_grid_item) and its amber Add PC pill keep the same way out. */
    @Test
    @Config(qualifiers = "w960dp-h540dp-land-television-xhdpi")
    public void rightFromATelevisionPcTileStepsOntoItsOverflowButtonThenOntoAddPc() {
        rightStepsOntoTheOverflowButtonThenOntoAddPc();
    }

    private void rightStepsOntoTheOverflowButtonThenOntoAddPc() {
        ActivityController<Library> controller = support.pairingScreen(new CountDownLatch(0), 47989);
        try {
            Library screen = controller.get();
            GridView grid = screen.findViewById(R.id.fragmentView);
            android.view.View add = screen.findViewById(R.id.manuallyAddPc);
            assertNotNull(add);
            assertEquals(1, grid.getCount());
            assertTrue(grid.requestFocusFromTouch());
            grid.setSelection(0);
            android.view.View root = screen.findViewById(android.R.id.content);
            int w = screen.getResources().getDisplayMetrics().widthPixels;
            int h = screen.getResources().getDisplayMetrics().heightPixels;
            root.measure(android.view.View.MeasureSpec.makeMeasureSpec(w, android.view.View.MeasureSpec.EXACTLY),
                    android.view.View.MeasureSpec.makeMeasureSpec(h, android.view.View.MeasureSpec.EXACTLY));
            root.layout(0, 0, w, h);
            android.view.View more = grid.getSelectedView().findViewById(R.id.grid_more);
            assertNotNull(more);
            assertFalse(more.isHovered());
            grid.dispatchKeyEvent(new android.view.KeyEvent(android.view.KeyEvent.ACTION_DOWN, android.view.KeyEvent.KEYCODE_DPAD_RIGHT));
            assertTrue("the first Right marks the overflow button", more.isHovered());
            assertTrue(grid.hasFocus());
            grid.dispatchKeyEvent(new android.view.KeyEvent(android.view.KeyEvent.ACTION_DOWN, android.view.KeyEvent.KEYCODE_DPAD_RIGHT));
            assertTrue("the second Right reaches Add PC", add.hasFocus());
            assertFalse(more.isHovered());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    /**
     * The footer under the PCs (still looking, How to connect) was placed with a translation, which focus search ignores:
     * Down from the last PC skipped to Add PC and the link was out of the D-pad's reach. It is laid out by its top margin
     * now, and while it shows, the list, its link and Add PC follow each other.
     */
    @Test
    @Config(qualifiers = "w411dp-h891dp-xxhdpi")
    public void theListFooterIsLaidOutUnderTheLastPcAndItsLinkIsOnTheDpadPath() {
        ActivityController<Library> controller = support.pairingScreen(new CountDownLatch(0), 47989);
        try {
            Library screen = controller.get();
            GridView grid = screen.findViewById(R.id.fragmentView);
            android.view.View footer = screen.findViewById(R.id.pc_list_footer);
            android.view.View help = screen.findViewById(R.id.pc_list_help);
            android.view.View add = screen.findViewById(R.id.manuallyAddPc);
            android.view.View root = screen.getWindow().getDecorView();
            int w = screen.getResources().getDisplayMetrics().widthPixels;
            int h = screen.getResources().getDisplayMetrics().heightPixels;
            for (int pass = 0; pass < 2; pass++) {
                root.measure(android.view.View.MeasureSpec.makeMeasureSpec(w, android.view.View.MeasureSpec.EXACTLY),
                        android.view.View.MeasureSpec.makeMeasureSpec(h, android.view.View.MeasureSpec.EXACTLY));
                root.layout(0, 0, w, h);
                screen.placeListFooter();
            }
            assertEquals(android.view.View.VISIBLE, footer.getVisibility());
            assertEquals("placed by layout, not drawn somewhere else", 0f, footer.getTranslationY(), 0f);
            android.view.View last = grid.getChildAt(grid.getChildCount() - 1);
            int[] lastAt = new int[2];
            int[] footerAt = new int[2];
            last.getLocationInWindow(lastAt);
            footer.getLocationInWindow(footerAt);
            assertEquals(lastAt[1] + last.getHeight() + screen.getResources().getDimensionPixelSize(R.dimen.space_l), footerAt[1]);

            assertEquals(R.id.pc_list_help, grid.getNextFocusDownId());
            assertEquals(R.id.fragmentView, help.getAccessibilityTraversalAfter());
            assertTrue(grid.requestFocus());
            assertSame("Down from the last PC reaches the link", help, grid.focusSearch(android.view.View.FOCUS_DOWN));
            assertSame(grid, help.focusSearch(android.view.View.FOCUS_UP));
            assertTrue("the + stays a stop", add.isFocusable());

            // Hidden (here: the list is replaced by the empty state), the link drops out of the path again.
            screen.findViewById(R.id.pcFragmentContainer).setVisibility(android.view.View.GONE);
            screen.placeListFooter();
            assertEquals(android.view.View.INVISIBLE, footer.getVisibility());
            assertEquals(android.view.View.NO_ID, grid.getNextFocusDownId());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    /**
     * TV run on a TCL C6K: Right on Settings, the last tab, jumped to the PC tile below it, and the next Select asked
     * the PC to pair. Right there stays on Settings; Down still reaches the tiles.
     */
    @Test
    @Config(qualifiers = "w960dp-h540dp-land-television-xhdpi")
    public void onATelevisionRightOnTheLastShellTabStaysThere() {
        ActivityController<Library> controller = support.pairingScreen(new CountDownLatch(0), 47989);
        try {
            Library screen = controller.get();
            com.limelight.ui.TvScreensPreviewTest.leaveTouchMode(screen);
            android.view.View root = screen.getWindow().getDecorView();
            int w = screen.getResources().getDisplayMetrics().widthPixels;
            int h = screen.getResources().getDisplayMetrics().heightPixels;
            root.measure(android.view.View.MeasureSpec.makeMeasureSpec(w, android.view.View.MeasureSpec.EXACTLY),
                    android.view.View.MeasureSpec.makeMeasureSpec(h, android.view.View.MeasureSpec.EXACTLY));
            root.layout(0, 0, w, h);
            android.view.View settings = screen.findViewById(R.id.shell_settings);
            android.view.View pcs = screen.findViewById(R.id.shell_pcs);
            assertTrue(settings.requestFocus());
            android.view.View plus = screen.findViewById(R.id.manuallyAddPc);
            if (plus.getVisibility() == android.view.View.VISIBLE) {
                assertSame("Right on the last tab reaches +, never a tile", plus, settings.focusSearch(android.view.View.FOCUS_RIGHT));
                assertSame("+ ends the row", plus, plus.focusSearch(android.view.View.FOCUS_RIGHT));
                assertSame(settings, plus.focusSearch(android.view.View.FOCUS_LEFT));
            } else {
                assertSame("with no + the bar ends in a wall", settings, settings.focusSearch(android.view.View.FOCUS_RIGHT));
            }
            assertSame("Left still walks the bar", pcs, settings.focusSearch(android.view.View.FOCUS_LEFT));
            android.view.View below = settings.focusSearch(android.view.View.FOCUS_DOWN);
            assertNotNull(below);
            assertNotSame("Down leaves the bar", settings, below);
            assertEquals("the other tabs still lead on", android.view.View.NO_ID, pcs.getNextFocusRightId());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    /** The row started at x=330 with the title at x=96, and the capsule 8 px below Home's. */
    @Test
    @Config(qualifiers = "w960dp-h540dp-land-television-xhdpi")
    public void onATelevisionTheCapsuleIsHomesAndTheRowStartsUnderTheTitle() {
        ActivityController<Library> controller = support.pairingScreen(new CountDownLatch(0), 47989);
        try {
            Library screen = controller.get();
            support.report(pc("b", "Beta", ComputerDetails.State.ONLINE, PairingManager.PairState.PAIRED));
            assertEquals(com.limelight.console.HomeShellTest.tvCapsule(), com.limelight.ui.TvAudit.topCapsule(screen));
            GridView grid = screen.findViewById(R.id.fragmentView);
            // Where the two are laid out: the first PC has the focus, and a focused tile is drawn larger around its
            // middle.
            assertEquals(96, laidOutLeft(screen.findViewById(R.id.pc_focus_title)));
            assertEquals(96, laidOutLeft(grid.getChildAt(0).findViewById(R.id.grid_card)));
        } finally {
            controller.pause().stop().destroy();
        }
    }

    /** Bigger hosts' words: the update line wraps inside its card instead of ending in dots. */
    @Test
    @Config(qualifiers = "de-w960dp-h540dp-land-television-xhdpi")
    public void aLongUpdateLineWrapsInsideItsCard() {
        android.view.LayoutInflater inflater = android.view.LayoutInflater.from(new android.view.ContextThemeWrapper(
                RuntimeEnvironment.getApplication(), R.style.AppTheme));
        android.view.View card = inflater.inflate(R.layout.pc_grid_card, null);
        android.widget.TextView update = card.findViewById(R.id.grid_update);
        assertEquals(2, update.getMaxLines());
    }

    /** The tabs draw the ring themselves: when focus left for the page below, the ring stayed on Settings beside the new one. */
    @Test
    @Config(qualifiers = "w960dp-h540dp-land-television-xhdpi")
    public void theTabsRingGoesWithFocus() {
        ActivityController<Library> controller = support.pairingScreen(new CountDownLatch(0), 47989);
        try {
            Library screen = controller.get();
            Object root = ReflectionHelpers.callInstanceMethod(screen.getWindow().getDecorView(), "getViewRootImpl");
            ReflectionHelpers.callInstanceMethod(root, "ensureTouchMode", ReflectionHelpers.ClassParameter.from(boolean.class, false));
            android.view.View tabs = screen.findViewById(R.id.shell_bar_items);
            android.view.View plus = screen.findViewById(R.id.manuallyAddPc);
            plus.setVisibility(android.view.View.VISIBLE);
            assertTrue(screen.findViewById(R.id.shell_settings).requestFocus());
            org.robolectric.Shadows.shadowOf(tabs).clearWasInvalidated();
            assertTrue(plus.requestFocus());
            assertTrue(org.robolectric.Shadows.shadowOf(tabs).wasInvalidated());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    /**
     * Shield TV, Android 11: the PCs page opened with the title naming the first PC and the hints offering OK, but its tile
     * had no ring, glow or scale until Right ringed the second. The grid, handed an empty adapter, took no focus until the
     * adapter reported the PCs, so the focus came from inside its first layout, which had already left the tile unmarked.
     */
    @Test
    @Config(qualifiers = "w960dp-h540dp-land-television-xhdpi")
    public void onATelevisionTheFirstPcWearsTheRingBeforeAnyKey() {
        ActivityController<Library> controller = support.emptyScreen(new CountDownLatch(0));
        try {
            Library screen = controller.get();
            com.limelight.ui.TvScreensPreviewTest.leaveTouchMode(screen);
            Shadows.shadowOf(Looper.getMainLooper()).idle();
            // As polling starts, the manager reports every PC it knows, one after the other, before the list is laid out.
            support.polling[0].notifyComputerUpdated(pc("a", "Alpha", ComputerDetails.State.ONLINE, PairingManager.PairState.PAIRED));
            support.polling[0].notifyComputerUpdated(pc("b", "Beta", ComputerDetails.State.ONLINE, PairingManager.PairState.PAIRED));
            Shadows.shadowOf(Looper.getMainLooper()).idle();

            GridView grid = screen.findViewById(R.id.fragmentView);
            assertEquals(2, grid.getCount());
            assertFalse(grid.isInTouchMode());
            assertTrue("the list holds focus", grid.isFocused());
            assertEquals(0, grid.getSelectedItemPosition());
            assertEquals("Alpha", ((TextView) screen.findViewById(R.id.pc_focus_title)).getText().toString());
            assertTrue("the first tile wears the ring with no key pressed", wearsRing(grid, 0));
            assertFalse(wearsRing(grid, 1));

            assertTrue("both tiles share a row", grid.getNumColumns() > 1);
            grid.dispatchKeyEvent(new android.view.KeyEvent(android.view.KeyEvent.ACTION_DOWN, android.view.KeyEvent.KEYCODE_DPAD_RIGHT));
            Shadows.shadowOf(Looper.getMainLooper()).idle();
            assertEquals(1, grid.getSelectedItemPosition());
            assertTrue("Right moves the ring to the second tile", wearsRing(grid, 1));
            assertFalse(wearsRing(grid, 0));
        } finally {
            controller.pause().stop().destroy();
        }
    }

    /** A PC that changes group moves in the sorted list; the ring moves with it, so OK acts on the PC it shows. */
    @Test
    @Config(qualifiers = "w960dp-h540dp-land-television-xhdpi")
    public void theRingFollowsItsPcWhenARefreshSortsItBehindAnother() {
        ActivityController<Library> controller = support.emptyScreen(new CountDownLatch(0));
        try {
            Library screen = controller.get();
            com.limelight.ui.TvScreensPreviewTest.leaveTouchMode(screen);
            Shadows.shadowOf(Looper.getMainLooper()).idle();
            support.report(pc("a", "Alpha", ComputerDetails.State.ONLINE, PairingManager.PairState.PAIRED));
            support.report(pc("b", "Beta", ComputerDetails.State.ONLINE, PairingManager.PairState.PAIRED));
            GridView grid = screen.findViewById(R.id.fragmentView);
            assertTrue("the list holds focus", grid.hasFocus());
            assertEquals(0, grid.getSelectedItemPosition());
            assertEquals("Alpha", ((PcView.ComputerObject) grid.getSelectedItem()).details.name);

            // Alpha stops answering: the PCs that answer come first, so Beta overtakes it.
            support.report(pc("a", "Alpha", ComputerDetails.State.OFFLINE, PairingManager.PairState.PAIRED));
            assertEquals("Beta", ((PcView.ComputerObject) grid.getItemAtPosition(0)).details.name);
            assertEquals(1, grid.getSelectedItemPosition());
            assertEquals("Alpha", ((PcView.ComputerObject) grid.getSelectedItem()).details.name);
            assertTrue("the ring is on Alpha's tile", wearsRing(grid, 1));
            assertFalse(wearsRing(grid, 0));
        } finally {
            controller.pause().stop().destroy();
        }
    }

    /** A touched phone keeps no selection as its first PCs arrive: no row wears the ring until a key is pressed. */
    @Test
    @Config(qualifiers = "w411dp-h891dp-xxhdpi")
    public void onATouchedPhoneTheFirstPcsWearNoRing() {
        ActivityController<Library> controller = support.emptyScreen(new CountDownLatch(0));
        try {
            Library screen = controller.get();
            Object root = ReflectionHelpers.callInstanceMethod(screen.getWindow().getDecorView(), "getViewRootImpl");
            ReflectionHelpers.callInstanceMethod(root, "ensureTouchMode", ReflectionHelpers.ClassParameter.from(boolean.class, true));
            Shadows.shadowOf(Looper.getMainLooper()).idle();
            support.polling[0].notifyComputerUpdated(pc("a", "Alpha", ComputerDetails.State.ONLINE, PairingManager.PairState.PAIRED));
            support.polling[0].notifyComputerUpdated(pc("b", "Beta", ComputerDetails.State.ONLINE, PairingManager.PairState.PAIRED));
            Shadows.shadowOf(Looper.getMainLooper()).idle();

            GridView grid = screen.findViewById(R.id.fragmentView);
            assertTrue(grid.isInTouchMode());
            assertTrue(grid.getChildCount() > 0);
            assertTrue("a phone lists PCs as rows", grid.getChildAt(0) instanceof com.limelight.ui.PcRow);
            assertFalse(grid.isFocused());
            assertEquals(android.widget.AdapterView.INVALID_POSITION, grid.getSelectedItemPosition());
            for (int i = 0; i < grid.getChildCount(); i++) {
                assertFalse(grid.getChildAt(i).isSelected());
            }
        } finally {
            controller.pause().stop().destroy();
        }
    }

    /** With a remote the sheet opens with the ring on its first row. */
    @Test
    @Config(qualifiers = "w960dp-h540dp-land-television-xhdpi")
    public void theAddSheetOpensWithItsFirstRowFocused() {
        org.robolectric.Shadows.shadowOf(RuntimeEnvironment.getApplication().getPackageManager())
                .setSystemFeature(android.content.pm.PackageManager.FEATURE_CAMERA_ANY, true);
        ActivityController<Library> controller = support.pairingScreen(new CountDownLatch(0), 47989);
        try {
            Library screen = controller.get();
            Object root = ReflectionHelpers.callInstanceMethod(screen.getWindow().getDecorView(), "getViewRootImpl");
            ReflectionHelpers.callInstanceMethod(root, "ensureTouchMode", ReflectionHelpers.ClassParameter.from(boolean.class, false));
            screen.findViewById(R.id.manuallyAddPc).performClick();
            com.limelight.ui.ActionSheet sheet = ReflectionHelpers.getField(screen, "addPcSheet");
            org.robolectric.Shadows.shadowOf(android.os.Looper.getMainLooper()).idle();
            android.view.View dropped = sheet.getCurrentFocus();
            if (dropped != null) dropped.clearFocus();
            sheet.getWindow().getDecorView().clearFocus();
            sheet.onWindowFocusChanged(true);
            android.view.View focus = sheet.getCurrentFocus();
            assertNotNull(focus);
            assertTrue(focus.getClass().getName(), focus.isClickable());
            sheet.dismiss();
        } finally {
            controller.pause().stop().destroy();
        }
    }

    /** The link wears the ring every control wears, not a Material stroke of its own. */
    @Test
    public void howToConnectWearsTheGlassRing() {
        ActivityController<Library> controller = support.pairingScreen(new CountDownLatch(0), 47989);
        try {
            assertTrue(controller.get().findViewById(R.id.pc_list_help) instanceof com.limelight.ui.GlassButton);
        } finally {
            controller.pause().stop().destroy();
        }
    }

    /** The words of "How to connect" start where the words above them start, not a few dots beside them. */
    @Test
    public void howToConnectLinesUpWithTheLineAboveIt() {
        ActivityController<Library> controller = support.pairingScreen(new CountDownLatch(0), 47989);
        try {
            Library screen = controller.get();
            android.view.View help = screen.findViewById(R.id.pc_list_help);
            android.view.ViewGroup row = (android.view.ViewGroup) screen.findViewById(R.id.pc_list_spinner).getParent();
            android.view.ViewGroup footer = (android.view.ViewGroup) help.getParent();
            footer.measure(android.view.View.MeasureSpec.makeMeasureSpec(1000, android.view.View.MeasureSpec.EXACTLY),
                    android.view.View.MeasureSpec.makeMeasureSpec(0, android.view.View.MeasureSpec.UNSPECIFIED));
            footer.layout(0, 0, 1000, footer.getMeasuredHeight());
            assertEquals(row.getLeft() + row.getChildAt(1).getLeft(), help.getLeft() + help.getPaddingLeft());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    /** Three PCs on a 1080p TV: one row from the page's margin, inside the overscan, with Add PC clear of it and still reachable. */
    @Test
    @Config(qualifiers = "w960dp-h540dp-land-television-xhdpi")
    public void onATelevisionThreePcsSitInOneRowAndAddPcStaysClearAndReachable() {
        ActivityController<Library> controller = support.pairingScreen(new CountDownLatch(0), 47989);
        try {
            Library screen = controller.get();
            support.report(pc("b", "Beta", ComputerDetails.State.ONLINE, PairingManager.PairState.PAIRED));
            support.report(pc("c", "Gamma", ComputerDetails.State.OFFLINE, PairingManager.PairState.PAIRED));
            GridView grid = screen.findViewById(R.id.fragmentView);
            assertEquals(3, grid.getCount());
            android.view.View root = screen.findViewById(android.R.id.content);
            int w = screen.getResources().getDisplayMetrics().widthPixels;
            int h = screen.getResources().getDisplayMetrics().heightPixels;
            root.measure(android.view.View.MeasureSpec.makeMeasureSpec(w, android.view.View.MeasureSpec.EXACTLY),
                    android.view.View.MeasureSpec.makeMeasureSpec(h, android.view.View.MeasureSpec.EXACTLY));
            root.layout(0, 0, w, h);
            assertEquals("one row", grid.getChildAt(0).getTop(), grid.getChildAt(2).getTop());
            int[] at = new int[2];
            grid.getLocationInWindow(at);
            int overscan = screen.getResources().getDimensionPixelSize(R.dimen.shell_overscan_horizontal);
            int glow = screen.getResources().getDimensionPixelSize(R.dimen.tile_glow);
            assertTrue("the grid stays inside the overscan, past a card's own glow", at[0] >= overscan - glow && at[0] + grid.getWidth() <= w - overscan + glow);
            android.view.View add = screen.findViewById(R.id.manuallyAddPc);
            android.graphics.Rect addRect = new android.graphics.Rect();
            add.getGlobalVisibleRect(addRect);
            for (int i = 0; i < grid.getChildCount(); i++) {
                android.view.View card = grid.getChildAt(i).findViewById(R.id.grid_card);
                android.graphics.Rect cardRect = new android.graphics.Rect();
                card.getGlobalVisibleRect(cardRect);
                assertFalse("Add PC clear of tile " + i, android.graphics.Rect.intersects(addRect, cardRect));
            }
            assertTrue(grid.requestFocusFromTouch());
            grid.setSelection(2);
            root.measure(android.view.View.MeasureSpec.makeMeasureSpec(w, android.view.View.MeasureSpec.EXACTLY),
                    android.view.View.MeasureSpec.makeMeasureSpec(h, android.view.View.MeasureSpec.EXACTLY));
            root.layout(0, 0, w, h);
            grid.dispatchKeyEvent(new android.view.KeyEvent(android.view.KeyEvent.ACTION_DOWN, android.view.KeyEvent.KEYCODE_DPAD_RIGHT));
            grid.dispatchKeyEvent(new android.view.KeyEvent(android.view.KeyEvent.ACTION_DOWN, android.view.KeyEvent.KEYCODE_DPAD_RIGHT));
            assertTrue("Right past the last tile reaches Add PC", add.hasFocus());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void theOptionsHintStaysInItsPlaceWhilePointerAndKeysTakeTurns() {
        ActivityController<Library> controller = support.pairingScreen(new CountDownLatch(0), 47989);
        try {
            Library screen = controller.get();
            support.report(pc("a", "Alpha", ComputerDetails.State.ONLINE, PairingManager.PairState.PAIRED));
            android.view.View hint = screen.findViewById(R.id.pc_options_hint);
            long now = android.os.SystemClock.uptimeMillis();
            screen.dispatchKeyEvent(new android.view.KeyEvent(now, now, android.view.KeyEvent.ACTION_DOWN,
                    android.view.KeyEvent.KEYCODE_DPAD_DOWN, 0));
            assertEquals(android.view.View.VISIBLE, hint.getVisibility());
            screen.dispatchTouchEvent(android.view.MotionEvent.obtain(now, now, android.view.MotionEvent.ACTION_DOWN, 1, 1, 0));
            // Invisible, not gone: the list above must not change size on every switch.
            assertEquals(android.view.View.INVISIBLE, hint.getVisibility());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    /** The PC list and its pairing PIN on a 1080p TV: readable text, nothing read or focused in the overscan band. */
    @Test
    @Config(qualifiers = "w960dp-h540dp-land-television-xhdpi")
    @org.robolectric.annotation.GraphicsMode(org.robolectric.annotation.GraphicsMode.Mode.NATIVE)
    public void onATelevisionThePcListAndItsPinKeepTheirTextReadableAndInsideTheOverscan() throws Exception {
        ActivityController<Library> controller = support.pairingScreen(new CountDownLatch(1), 47989);
        Thread worker = null;
        try {
            Library screen = controller.get();
            // The PIN for the one unpaired PC first, then the list once two more PCs have joined it.
            GridView grid = screen.findViewById(R.id.fragmentView);
            grid.performItemClick(null, 0, 0);
            worker = ReflectionHelpers.getField(screen.hostActions(), "pairingThread");
            AlertDialog dialog = ReflectionHelpers.getField(screen.hostActions(), "pairingDialog");
            assertTrue(dialog.isShowing());
            assertEquals(java.util.Collections.emptyList(), com.limelight.ui.TvAudit.findings(dialog));
            dialog.getButton(AlertDialog.BUTTON_NEGATIVE).performClick();
            Shadows.shadowOf(Looper.getMainLooper()).idle();
            support.report(pc("b", "Beta", ComputerDetails.State.ONLINE, PairingManager.PairState.PAIRED));
            support.report(pc("c", "Gamma", ComputerDetails.State.OFFLINE, PairingManager.PairState.PAIRED));
            Shadows.shadowOf(Looper.getMainLooper()).idle();
            assertEquals(java.util.Collections.emptyList(), com.limelight.ui.TvAudit.findings(screen));
            com.limelight.ui.TvScreensPreviewTest.previewIfRequested(screen, "PcView-three");
        } finally {
            controller.pause().stop().destroy();
            if (worker != null) worker.join(5000);
        }
    }
}
