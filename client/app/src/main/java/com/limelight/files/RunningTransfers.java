package com.limelight.files;

import android.app.Activity;
import android.view.Gravity;
import android.view.View;
import android.widget.Button;
import android.widget.FrameLayout;

import com.limelight.R;

import java.util.ArrayList;
import java.util.List;

/**
 * The Files page's transfers after the page closed with some unfinished, as on desktop: they go on while the app
 * runs, one PC at a time, and end when they finish or are cancelled on that PC's Files page opened again. No
 * foreground service keeps them: in the background they may end with the process, as before. The screens that lead
 * to Files show how many run ({@link #showOn}). Main thread only.
 */
public final class RunningTransfers {
    private static FileTransferActivity.TransferWork held;
    private static final List<Runnable> listeners = new ArrayList<>();

    private RunningTransfers() {}

    static void hold(FileTransferActivity.TransferWork work) {
        held = work;
        changed();
    }

    static FileTransferActivity.TransferWork held() {
        return held;
    }

    /** The held work, which a Files page for its PC shows again; nothing is held after. */
    static FileTransferActivity.TransferWork take() {
        FileTransferActivity.TransferWork work = held;
        held = null;
        changed();
        return work;
    }

    static void release(FileTransferActivity.TransferWork work) {
        if (held != work) return;
        held = null;
        changed();
    }

    /** Unfinished transfers a closed Files page left running; 0 for none. */
    public static int count() {
        FileTransferActivity.TransferWork work = held;
        if (work == null || work.controller == null) return 0;
        int count = 0;
        for (FileRow row : work.controller.rows()) {
            if (!row.isFinished()) count++;
        }
        return count;
    }

    /** The PC they run with, or null. */
    public static String pcUuid() {
        return held == null ? null : held.uuid;
    }

    static void changed() {
        for (Runnable listener : new ArrayList<>(listeners)) listener.run();
    }

    /**
     * Shows "File transfers running: n" over the activity's page while there are any, as desktop's button over the
     * launcher; a press opens that PC's Files page, where each one can be cancelled. Call from onResume; the
     * returned Runnable, from onPause, takes the button away again.
     */
    public static Runnable showOn(Activity activity) {
        View root = activity.findViewById(android.R.id.content);
        if (!(root instanceof FrameLayout)) return () -> {};
        FrameLayout content = (FrameLayout) root;
        Button button = new com.google.android.material.button.MaterialButton(activity, null,
                com.google.android.material.R.attr.materialButtonOutlinedStyle);
        int margin = Math.round(24 * activity.getResources().getDisplayMetrics().density);
        FrameLayout.LayoutParams params = new FrameLayout.LayoutParams(FrameLayout.LayoutParams.WRAP_CONTENT,
                FrameLayout.LayoutParams.WRAP_CONTENT, Gravity.BOTTOM | Gravity.START);
        params.setMargins(margin, margin, margin, margin);
        button.setOnClickListener(v -> {
            String pc = pcUuid();
            if (pc != null) activity.startActivity(FileTransferActivity.forPc(activity, pc));
        });
        content.addView(button, params);
        Runnable update = () -> {
            int count = count();
            button.setVisibility(count > 0 ? View.VISIBLE : View.GONE);
            if (count > 0) {
                String text = activity.getString(R.string.files_running, count);
                if (!text.contentEquals(button.getText())) button.setText(text);
            }
        };
        update.run();
        listeners.add(update);
        return () -> {
            listeners.remove(update);
            content.removeView(button);
        };
    }
}
