package com.limelight;

import android.app.Application;
import android.os.Build;
import android.os.Looper;
import android.widget.GridView;
import android.widget.TextView;

import com.limelight.PcViewLifecycleTest.Library;
import com.limelight.computers.ComputerManagerListener;
import com.limelight.computers.ComputerManagerService;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.PairingManager;
import com.limelight.preferences.GlPreferences;

import org.robolectric.Robolectric;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.Shadows;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.shadows.ShadowDialog;
import org.robolectric.util.ReflectionHelpers;

import java.util.concurrent.CountDownLatch;

import static org.junit.Assert.*;

/**
 * Shared Robolectric harness and UI helpers for the {@link PcView} test suite.
 */
final class PcViewTestSupport {
    final ComputerManagerListener[] polling = new ComputerManagerListener[1];
    final java.util.Map<String, String> renamed = new java.util.LinkedHashMap<>();
    final java.util.List<ComputerDetails> removed = new java.util.ArrayList<>();
    final java.util.List<String> invalidated = new java.util.concurrent.CopyOnWriteArrayList<>();
    long wakeClock;

    enum Input { TOUCH_MORE, LONG_PRESS, MOUSE_RIGHT_CLICK, DPAD_RIGHT_THEN_CENTER, MENU_KEY, SHIFT_F10, BUTTON_X }

    ActivityController<Library> pairingScreen(CountDownLatch pollingStopped, int port) {
        ActivityController<Library> controller = emptyScreen(pollingStopped);
        ComputerDetails computer = new ComputerDetails();
        computer.uuid = "host-uuid";
        computer.name = "PC";
        computer.state = ComputerDetails.State.ONLINE;
        computer.pairState = PairingManager.PairState.NOT_PAIRED;
        computer.activeAddress = new ComputerDetails.AddressTuple("127.0.0.1", port);
        ReflectionHelpers.callInstanceMethod(controller.get(), "updateComputer",
                ReflectionHelpers.ClassParameter.from(ComputerDetails.class, computer));
        return controller;
    }

    /** The PCs page, polling a manager that has reported no PC yet. */
    ActivityController<Library> emptyScreen(CountDownLatch pollingStopped) {
        Application application = RuntimeEnvironment.getApplication();
        GlPreferences gl = GlPreferences.readPreferences(application);
        gl.savedFingerprint = Build.FINGERPRINT;
        gl.glRenderer = "Test renderer";
        gl.writePreferences();
        application.getSharedPreferences("FirstRun", 0).edit().putBoolean("pairing_guide_shown", true).commit();
        ActivityController<Library> controller = Robolectric.buildActivity(Library.class).setup().visible();
        ComputerManagerService manager = new ComputerManagerService();
        ReflectionHelpers.setField(controller.get(), "managerBinder", manager.new ComputerManagerBinder() {
            @Override public boolean isPersistent() { return false; }
            @Override public void startPolling(ComputerManagerListener listener) { polling[0] = listener; }
            @Override public void stopPolling() { }
            @Override public void renameComputer(String uuid, String alias) { renamed.put(uuid, alias); }
            @Override public void removeComputer(ComputerDetails computer) { removed.add(computer); }
            @Override public void invalidateStateForComputer(String uuid) { invalidated.add(uuid); }
            @Override public String getUniqueId() { return "client"; }
            @Override public void waitForPollingStopped() {
                try {
                    pollingStopped.await();
                } catch (InterruptedException e) {
                    Thread.currentThread().interrupt();
                }
            }
        });
        ReflectionHelpers.callInstanceMethod(controller.get(), "startComputerUpdates");
        return controller;
    }

    static ComputerDetails pc(String uuid, String name, ComputerDetails.State state, PairingManager.PairState pairing) {
        ComputerDetails computer = new ComputerDetails();
        computer.uuid = uuid;
        computer.name = name;
        computer.state = state;
        computer.pairState = pairing;
        computer.activeAddress = state == ComputerDetails.State.ONLINE ?
                new ComputerDetails.AddressTuple("127.0.0.1", 47989) : null;
        return computer;
    }

    /** Reports a PC the way the computer manager does and waits for the home screen to show it. */
    void report(ComputerDetails computer) {
        polling[0].notifyComputerUpdated(computer);
        Shadows.shadowOf(Looper.getMainLooper()).idle();
    }

    /** The sheet's actions, top to bottom, for the PC named {@code name}. */
    static java.util.List<String> sheetFor(Library screen, String name) {
        GridView grid = screen.findViewById(R.id.fragmentView);
        for (int i = 0; i < grid.getCount(); i++) {
            PcView.ComputerObject computer = (PcView.ComputerObject) grid.getItemAtPosition(i);
            if (computer.details.name.equals(name)) {
                screen.showComputerMenu(computer);
                com.limelight.ui.ActionSheet sheet = (com.limelight.ui.ActionSheet) ShadowDialog.getLatestDialog();
                java.util.List<String> actions = new java.util.ArrayList<>();
                collectActions(sheet.findViewById(android.R.id.content), actions);
                sheet.dismiss();
                Shadows.shadowOf(Looper.getMainLooper()).idle();
                return actions;
            }
        }
        throw new AssertionError("No row for " + name);
    }

    static void collectActions(android.view.View view, java.util.List<String> actions) {
        if (view instanceof com.google.android.material.button.MaterialButton) {
            com.google.android.material.button.MaterialButton button = (com.google.android.material.button.MaterialButton) view;
            actions.add(button.getText() + (button.isEnabled() ? "" : " (disabled)")
                    + (button.getAlpha() < 1 ? " (dimmed)" : ""));
        } else if (view instanceof android.view.ViewGroup) {
            android.view.ViewGroup group = (android.view.ViewGroup) view;
            for (int i = 0; i < group.getChildCount(); i++) {
                collectActions(group.getChildAt(i), actions);
            }
        }
    }

    static java.util.List<String> labels(Library screen, int... ids) {
        java.util.List<String> labels = new java.util.ArrayList<>();
        for (int id : ids) labels.add(screen.getString(id));
        return labels;
    }

    /** Starts a wake as the menu does, with a recorded sender and the test's clock. */
    void startWake(Library screen, ComputerDetails computer, com.limelight.console.HostWaker.Sender sender) {
        ReflectionHelpers.callInstanceMethod(screen.hostActions(), "startWake",
                ReflectionHelpers.ClassParameter.from(ComputerDetails.class, computer),
                ReflectionHelpers.ClassParameter.from(com.limelight.console.HostWaker.Sender.class, sender),
                ReflectionHelpers.ClassParameter.from(java.util.concurrent.Executor.class, (java.util.concurrent.Executor) Runnable::run),
                ReflectionHelpers.ClassParameter.from(java.util.function.LongSupplier.class,
                        (java.util.function.LongSupplier) () -> wakeClock));
    }

    ComputerDetails asleep(String uuid, String name) {
        ComputerDetails computer = pc(uuid, name, ComputerDetails.State.OFFLINE, PairingManager.PairState.PAIRED);
        computer.macAddress = "02:00:00:00:00:0" + name.length();
        report(computer);
        return computer;
    }

    static void toHandheld(Library screen) {
        org.robolectric.RuntimeEnvironment.setQualifiers("w900dp-h700dp-land");
        screen.onConfigurationChanged(screen.getResources().getConfiguration());
        Shadows.shadowOf(Looper.getMainLooper()).idle();
        assertEquals(com.limelight.ui.GlassForm.HANDHELD, com.limelight.ui.GlassForm.of(screen));
    }

    static android.view.View button(android.view.View view, String label) {
        if (view instanceof com.google.android.material.button.MaterialButton
                && label.contentEquals(((com.google.android.material.button.MaterialButton) view).getText())) {
            return view;
        }
        if (view instanceof android.view.ViewGroup) {
            android.view.ViewGroup group = (android.view.ViewGroup) view;
            for (int i = 0; i < group.getChildCount(); i++) {
                android.view.View found = button(group.getChildAt(i), label);
                if (found != null) return found;
            }
        }
        return null;
    }

    static void press(android.view.View view, int code, int meta) {
        long now = android.os.SystemClock.uptimeMillis();
        view.dispatchKeyEvent(new android.view.KeyEvent(now, now, android.view.KeyEvent.ACTION_DOWN, code, 0, meta));
        view.dispatchKeyEvent(new android.view.KeyEvent(now, now, android.view.KeyEvent.ACTION_UP, code, 0, meta));
    }

    /** A mouse's right button pressed and released over {@code card}. */
    static void rightClick(GridView grid, android.view.View card) {
        float x = card.getLeft() + card.getWidth() / 2f;
        float y = card.getTop() + card.getHeight() / 2f;
        long now = android.os.SystemClock.uptimeMillis();
        for (int action : new int[] {android.view.MotionEvent.ACTION_DOWN, android.view.MotionEvent.ACTION_UP}) {
            android.view.MotionEvent.PointerProperties properties = new android.view.MotionEvent.PointerProperties();
            properties.id = 0;
            properties.toolType = android.view.MotionEvent.TOOL_TYPE_MOUSE;
            android.view.MotionEvent.PointerCoords coords = new android.view.MotionEvent.PointerCoords();
            coords.x = x;
            coords.y = y;
            android.view.MotionEvent event = android.view.MotionEvent.obtain(now, now, action, 1,
                    new android.view.MotionEvent.PointerProperties[] {properties},
                    new android.view.MotionEvent.PointerCoords[] {coords}, 0,
                    action == android.view.MotionEvent.ACTION_DOWN ? android.view.MotionEvent.BUTTON_SECONDARY : 0,
                    1, 1, 0, 0, android.view.InputDevice.SOURCE_MOUSE, 0);
            assertTrue(grid.dispatchTouchEvent(event));
            event.recycle();
        }
    }

    /** Opens the options of {@code row} the way {@code input} does and returns the sheet, or null when nothing opened. */
    static com.limelight.ui.ActionSheet openOptionsWith(Library screen, int row, Input input) {
        GridView grid = screen.findViewById(R.id.fragmentView);
        Shadows.shadowOf(Looper.getMainLooper()).idle();
        android.view.View card = grid.getChildAt(row - grid.getFirstVisiblePosition());
        assertNotNull("Row " + row + " is on screen", card);
        switch (input) {
            case TOUCH_MORE:
                assertTrue(card.findViewById(R.id.grid_more).performClick());
                break;
            case LONG_PRESS:
                assertTrue(grid.getOnItemLongClickListener().onItemLongClick(grid, card, row, grid.getItemIdAtPosition(row)));
                break;
            case MOUSE_RIGHT_CLICK:
                rightClick(grid, card);
                break;
            default:
                grid.requestFocus();
                grid.setSelection(row);
                Shadows.shadowOf(Looper.getMainLooper()).idle();
                assertEquals(row, grid.getSelectedItemPosition());
                if (input == Input.DPAD_RIGHT_THEN_CENTER) {
                    // Every row here is the last in its line, so Right marks the three dots and Center presses them.
                    press(grid, android.view.KeyEvent.KEYCODE_DPAD_RIGHT, 0);
                    press(grid, android.view.KeyEvent.KEYCODE_DPAD_CENTER, 0);
                } else if (input == Input.MENU_KEY) {
                    press(grid, android.view.KeyEvent.KEYCODE_MENU, 0);
                } else if (input == Input.SHIFT_F10) {
                    press(grid, android.view.KeyEvent.KEYCODE_F10, android.view.KeyEvent.META_SHIFT_ON);
                } else {
                    press(grid, android.view.KeyEvent.KEYCODE_BUTTON_X, 0);
                }
        }
        Shadows.shadowOf(Looper.getMainLooper()).idle();
        android.app.Dialog dialog = ShadowDialog.getLatestDialog();
        return dialog instanceof com.limelight.ui.ActionSheet && dialog.isShowing() ? (com.limelight.ui.ActionSheet) dialog : null;
    }

    /** How often the grid was told to sort and bind its rows again since {@code grid} was handed over. */
    static int[] refreshesOf(GridView grid) {
        int[] refreshes = new int[1];
        grid.getAdapter().registerDataSetObserver(new android.database.DataSetObserver() {
            @Override public void onChanged() {
                refreshes[0]++;
            }
        });
        return refreshes;
    }

    static PcView.ComputerObject rowOf(GridView grid, String uuid) {
        for (int i = 0; i < grid.getCount(); i++) {
            PcView.ComputerObject computer = (PcView.ComputerObject) grid.getItemAtPosition(i);
            if (computer.details.uuid.equals(uuid)) {
                return computer;
            }
        }
        throw new AssertionError("No row for " + uuid);
    }

    /** A view's left edge in its window as laid out, whatever scale it or a parent is drawn at. */
    static int laidOutLeft(android.view.View view) {
        int left = 0;
        for (android.view.View at = view; at.getParent() instanceof android.view.View; at = (android.view.View) at.getParent()) {
            left += at.getLeft() - ((android.view.View) at.getParent()).getScrollX();
        }
        return left;
    }

    /** The grid marks the tile it selects; its ring takes that state from it. */
    static boolean wearsRing(GridView grid, int position) {
        android.view.View tile = grid.getChildAt(position - grid.getFirstVisiblePosition());
        if (tile == null || !tile.isSelected()) return false;
        for (int state : tile.findViewById(R.id.grid_ring).getDrawableState()) {
            if (state == android.R.attr.state_selected) return true;
        }
        return false;
    }

    static void saveName(Library screen, ComputerDetails computer, String typed) {
        ReflectionHelpers.callInstanceMethod(screen.hostActions(), "saveName",
                ReflectionHelpers.ClassParameter.from(ComputerDetails.class, computer),
                ReflectionHelpers.ClassParameter.from(String.class, typed));
    }

    static void showResult(com.limelight.computers.HostActions actions, int generation, boolean paired,
                           String failure, ComputerDetails computer) {
        ReflectionHelpers.callInstanceMethod(actions, "showPairingResult",
                ReflectionHelpers.ClassParameter.from(int.class, generation),
                ReflectionHelpers.ClassParameter.from(boolean.class, paired),
                ReflectionHelpers.ClassParameter.from(String.class, failure),
                ReflectionHelpers.ClassParameter.from(ComputerDetails.class, computer),
                ReflectionHelpers.ClassParameter.from(String.class, null));
    }

    static void findByText(android.view.View view, String text, java.util.List<android.view.View> found) {
        if (view instanceof TextView && text.contentEquals(((TextView) view).getText())) {
            found.add(view);
        } else if (view instanceof android.view.ViewGroup) {
            android.view.ViewGroup group = (android.view.ViewGroup) view;
            for (int i = 0; i < group.getChildCount(); i++) findByText(group.getChildAt(i), text, found);
        }
    }
}
