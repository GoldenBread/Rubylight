package com.limelight.ui;

import android.graphics.Rect;
import android.view.MotionEvent;
import android.view.TouchDelegate;
import android.view.View;
import android.view.ViewConfiguration;
import android.view.ViewGroup;

/**
 * Touch areas of at least 48dp around small touch-only controls, without changing how they look or where they stand. A
 * touch near a control, inside the 48dp area centred on it and kept within the host, goes to the control, as a
 * {@link TouchDelegate} would send it; one host serves several controls, which a single TouchDelegate cannot.
 */
public final class TouchTargets extends TouchDelegate {
    static final int MIN_DP = 48;

    private final ViewGroup host;
    private final View[] controls;
    private final int min;
    private final int slop;
    private final Rect area = new Rect();
    /** The control the touch under way went to, or null. */
    private View target;

    private TouchTargets(ViewGroup host, View[] controls) {
        super(new Rect(), host);
        this.host = host;
        this.controls = controls;
        min = Math.round(MIN_DP * host.getResources().getDisplayMetrics().density);
        slop = ViewConfiguration.get(host.getContext()).getScaledTouchSlop();
    }

    /** Gives each control, a descendant of {@code host}, a touch area of at least 48dp; a host keeps the first it was given. */
    public static void apply(ViewGroup host, View... controls) {
        if (!(host.getTouchDelegate() instanceof TouchTargets)) {
            host.setTouchDelegate(new TouchTargets(host, controls));
        }
    }

    /** The control's area in the host: its bounds grown to 48dp each way, moved to stay within the host; null when it has none. */
    private Rect areaOf(View control) {
        if (control.getVisibility() != View.VISIBLE || !control.isEnabled()) return null;
        area.set(0, 0, control.getWidth(), control.getHeight());
        try {
            host.offsetDescendantRectToMyCoords(control, area);
        } catch (IllegalArgumentException notInHost) {
            return null;
        }
        int dx = Math.max(0, min - area.width());
        int dy = Math.max(0, min - area.height());
        area.left -= dx / 2;
        area.right += dx - dx / 2;
        area.top -= dy / 2;
        area.bottom += dy - dy / 2;
        // The host only sees touches within its bounds.
        if (area.left < 0) area.offset(-area.left, 0);
        if (area.top < 0) area.offset(0, -area.top);
        if (area.right > host.getWidth()) area.offset(host.getWidth() - area.right, 0);
        if (area.bottom > host.getHeight()) area.offset(0, host.getHeight() - area.bottom);
        return area;
    }

    @Override
    public boolean onTouchEvent(MotionEvent event) {
        int x = (int) event.getX();
        int y = (int) event.getY();
        int action = event.getActionMasked();
        if (action == MotionEvent.ACTION_DOWN) {
            target = null;
            for (View control : controls) {
                Rect bounds = areaOf(control);
                if (bounds != null && bounds.contains(x, y)) {
                    target = control;
                    break;
                }
            }
        }
        View control = target;
        if (control == null) return false;
        if (action == MotionEvent.ACTION_UP || action == MotionEvent.ACTION_CANCEL) target = null;
        // Within the area and the touch slop around it the control is touched at its centre; beyond, far off its bounds,
        // so it lets go of its press as it would under a finger that slid off.
        Rect bounds = areaOf(control);
        boolean within = bounds != null && x >= bounds.left - slop && x < bounds.right + slop
                && y >= bounds.top - slop && y < bounds.bottom + slop;
        MotionEvent moved = MotionEvent.obtain(event);
        if (within) {
            moved.setLocation(control.getWidth() / 2f, control.getHeight() / 2f);
        } else {
            moved.setLocation(-(slop * 2), -(slop * 2));
        }
        boolean handled = control.dispatchTouchEvent(moved);
        moved.recycle();
        if (action == MotionEvent.ACTION_DOWN && !handled) target = null;
        return handled;
    }
}
