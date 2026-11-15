package com.limelight.ui;

import android.app.Activity;
import android.app.Application;
import android.os.SystemClock;
import android.view.MotionEvent;
import android.view.View;
import android.view.ViewGroup;
import android.widget.FrameLayout;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowLooper;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertTrue;

/** Small touch-only controls take touches within 48dp of their middle, and keep their size and place. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class)
public class TouchTargetsTest {
    private final List<String> clicks = new ArrayList<>();

    private View control(FrameLayout host, String name, int left, int top, int width, int height) {
        View control = new View(host.getContext());
        control.setOnClickListener(v -> clicks.add(name));
        FrameLayout.LayoutParams params = new FrameLayout.LayoutParams(width, height);
        params.leftMargin = left;
        params.topMargin = top;
        host.addView(control, params);
        return control;
    }

    private static void tap(View host, float x, float y) {
        long now = SystemClock.uptimeMillis();
        for (int action : new int[] {MotionEvent.ACTION_DOWN, MotionEvent.ACTION_UP}) {
            MotionEvent event = MotionEvent.obtain(now, now, action, x, y, 0);
            host.dispatchTouchEvent(event);
            event.recycle();
        }
        ShadowLooper.idleMainLooper();
    }

    @Test
    public void aTapNearASmallControlReachesItAndAFarOneDoesNot() {
        ActivityController<Activity> controller = Robolectric.buildActivity(Activity.class).setup();
        try {
            Activity activity = controller.get();
            float dp = activity.getResources().getDisplayMetrics().density;
            FrameLayout host = new FrameLayout(activity);
            activity.setContentView(host, new ViewGroup.LayoutParams(Math.round(300 * dp), Math.round(300 * dp)));
            // A 40dp-high button, and a 44dp More in the host's corner.
            View button = control(host, "button", Math.round(100 * dp), Math.round(100 * dp), Math.round(120 * dp), Math.round(40 * dp));
            View more = control(host, "more", Math.round(256 * dp), 0, Math.round(44 * dp), Math.round(44 * dp));
            TouchTargets.apply(host, button, more);
            ShadowLooper.idleMainLooper();
            assertEquals("its size stays", Math.round(40 * dp), button.getHeight());

            tap(host, 160 * dp, 143 * dp);
            assertEquals("3dp under the button", Arrays.asList("button"), clicks);
            tap(host, 160 * dp, 97 * dp);
            assertEquals("3dp above it", Arrays.asList("button", "button"), clicks);
            tap(host, 160 * dp, 150 * dp);
            assertEquals("10dp under it is not the button", 2, clicks.size());
            // The corner's area stays within the host: 48dp from its edges, all of it reachable.
            tap(host, 254 * dp, 46 * dp);
            assertEquals(Arrays.asList("button", "button", "more"), clicks);

            button.setVisibility(View.GONE);
            ShadowLooper.idleMainLooper();
            tap(host, 160 * dp, 143 * dp);
            assertEquals("a hidden control takes nothing", 3, clicks.size());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void aSettingsChipIsTouchedOver48dpAndDrawnAsBefore() throws java.io.IOException {
        String chip = new String(java.nio.file.Files.readAllBytes(
                java.nio.file.Paths.get("src/main/res/layout/settings_chip.xml")), java.nio.charset.StandardCharsets.UTF_8);
        assertTrue(chip.contains("android:layout_height=\"" + TouchTargets.MIN_DP + "dp\""));
        // The 48dp are touch area; the chip is drawn 40dp high, as it was.
        assertTrue(chip.contains("@drawable/settings_chip_touch"));
        String look = new String(java.nio.file.Files.readAllBytes(
                java.nio.file.Paths.get("src/main/res/drawable/settings_chip_touch.xml")), java.nio.charset.StandardCharsets.UTF_8);
        assertTrue(look.contains("android:insetTop=\"4dp\"") && look.contains("android:insetBottom=\"4dp\""));
    }
}
