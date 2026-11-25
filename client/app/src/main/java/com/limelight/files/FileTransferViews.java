package com.limelight.files;

import android.content.Context;
import android.text.format.DateUtils;
import android.text.format.Formatter;
import android.util.TypedValue;
import android.view.View;
import android.widget.Button;
import android.widget.LinearLayout;
import android.widget.ProgressBar;
import android.widget.ScrollView;
import android.widget.TextView;

import com.google.android.material.button.MaterialButton;
import com.limelight.R;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.PairingManager;

import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.function.BiConsumer;
import java.util.function.Consumer;
import java.util.function.LongConsumer;

/**
 * View construction and focus-preserving list rendering (PC picker, active/finished transfers and remote folder
 * entries) for {@link FileTransferActivity}. Rebuilds button groups only when their keys change so D-pad focus
 * and in-progress clicks are not dropped during polling or transfer progress updates.
 */
final class FileTransferViews {
    private final Context context;

    TextView status;
    LinearLayout pcs;
    Button send;
    Button refresh;
    TextView transfersHeading;
    LinearLayout transfers;
    Button clearFinished;
    TextView filesHeading;
    TextView folder;
    LinearLayout files;

    /**
     * What {@link #pcs}, {@link #transfers} and {@link #files} show, one key per button set: every PC is polled
     * every 1.5 s, a transfer reports its progress many times a second and the folder is read after every upload.
     * Buttons made again on each of those lost a press before it was let go (a remote's OK button, a tap) and the
     * D-pad focus with it (issue 56).
     */
    private final List<String> shownPcs = new ArrayList<>();
    private final List<String> shownRows = new ArrayList<>();
    private final List<String> shownFiles = new ArrayList<>();

    FileTransferViews(Context context) {
        this.context = context;
    }

    View buildPage(Runnable onSend, Runnable onRefresh, Runnable onClearFinished) {
        ScrollView scroll = new ScrollView(context);
        LinearLayout page = new LinearLayout(context);
        page.setOrientation(LinearLayout.VERTICAL);
        int pad = dp(16);
        page.setPadding(pad, pad, pad, pad);
        scroll.addView(page);

        status = text(page, android.R.style.TextAppearance_Material_Body1);
        pcs = new LinearLayout(context);
        pcs.setOrientation(LinearLayout.VERTICAL);
        page.addView(pcs);

        LinearLayout actions = new LinearLayout(context);
        actions.setOrientation(LinearLayout.HORIZONTAL);
        page.addView(actions);
        send = button(actions, R.string.files_send);
        send.setOnClickListener(v -> onSend.run());
        refresh = button(actions, R.string.files_refresh);
        refresh.setOnClickListener(v -> onRefresh.run());

        transfersHeading = text(page, android.R.style.TextAppearance_Material_Title);
        transfersHeading.setText(R.string.files_transfers);
        transfers = new LinearLayout(context);
        transfers.setOrientation(LinearLayout.VERTICAL);
        page.addView(transfers);
        clearFinished = button(page, R.string.files_clear_finished);
        clearFinished.setOnClickListener(v -> onClearFinished.run());

        filesHeading = text(page, android.R.style.TextAppearance_Material_Title);
        filesHeading.setText(R.string.files_on_pc);
        folder = text(page, android.R.style.TextAppearance_Material_Body1);
        files = new LinearLayout(context);
        files.setOrientation(LinearLayout.VERTICAL);
        page.addView(files);
        return scroll;
    }

    void clearPcs() {
        pcs.removeAllViews();
        shownPcs.clear();
    }

    /**
     * The PCs a share can go to; false when there is none. The buttons are made again only when the PCs or their
     * names change, and the PC that had the focus keeps it.
     */
    boolean showPcs(Map<String, ComputerDetails> computers, BiConsumer<String, ComputerDetails> onPick) {
        List<ComputerDetails> online = new ArrayList<>();
        List<String> keys = new ArrayList<>();
        for (ComputerDetails details : computers.values()) {
            if (details.state != ComputerDetails.State.ONLINE || details.pairState != PairingManager.PairState.PAIRED
                    || !details.caps.has(HostCapabilities.Feature.NATIVE_API)) {
                continue;
            }
            online.add(details);
            keys.add(details.uuid + "\n" + details.displayName());
        }
        if (keys.equals(shownPcs)) return !keys.isEmpty();
        FileTransferActivity.Focus focused = focusIn(pcs);
        boolean appeared = shownPcs.isEmpty();
        pcs.removeAllViews();
        shownPcs.clear();
        shownPcs.addAll(keys);
        for (ComputerDetails details : online) {
            // By name: the label used to be the string resource 0, which crashed every share.
            Button pick = button(pcs, details.displayName());
            pick.setTag("pc:" + details.uuid);
            pick.setOnClickListener(v -> onPick.accept(details.uuid, details));
        }
        // A PC that went away hands the focus to its neighbour. When the list first shows, the first PC takes
        // the focus: it was on the Up button, and a remote's first OK closed the screen.
        restoreFocus(pcs, focused, null);
        if (focused == null && appeared && pcs.getChildCount() > 0) takesFocus(pcs.getChildAt(0));
        return !keys.isEmpty();
    }

    /**
     * The transfers and the downloads left behind. While the same transfers keep the same buttons, a change only
     * updates their texts and progress bars; otherwise the rows are made again, and the focus stays on its button,
     * or goes to the same row's other button or a neighbouring row's (see {@link #restoreFocus}).
     */
    void showTransfers(FileTransferActivity.TransferWork work, List<FileRow> rows,
                       LongConsumer onCancel, Consumer<FileRow> onRetry) {
        work.seen(rows);
        List<String> keys = new ArrayList<>();
        boolean finished = false;
        for (FileRow row : rows) {
            keys.add(rowKey(row));
            finished |= row.isFinished();
        }
        for (String name : work.leftovers) keys.add("left\n" + name);
        boolean shown = !rows.isEmpty() || !work.leftovers.isEmpty();
        transfersHeading.setVisibility(shown ? View.VISIBLE : View.GONE);
        clearFinished.setVisibility(finished || !work.leftovers.isEmpty() ? View.VISIBLE : View.GONE);
        if (keys.equals(shownRows)) {
            for (int i = 0; i < rows.size(); i++) updateRow((RowViews) transfers.getChildAt(i).getTag(), rows.get(i));
        } else {
            FileTransferActivity.Focus focused = focusIn(transfers);
            transfers.removeAllViews();
            shownRows.clear();
            shownRows.addAll(keys);
            for (FileRow row : rows) transfers.addView(transferRow(row, onCancel, onRetry));
            for (String name : work.leftovers) {
                text(transfers, android.R.style.TextAppearance_Material_Body1)
                        .setText(context.getString(R.string.files_partial_left, name));
            }
            restoreFocus(transfers, focused, clearFinished.getVisibility() == View.VISIBLE ? clearFinished : send,
                    otherButton(focused));
        }
    }

    /**
     * The PC's folder. It is read again after every upload and on Check again; its rows are made again only when
     * its files change, and the focus stays on the Download it was on or goes to a neighbouring one.
     */
    void showFiles(RemoteFiles list, FileProblems.Access downloadAccess, Consumer<RemoteFiles.Entry> onDownload) {
        List<String> keys = new ArrayList<>();
        if (list != null && list.problem != null) {
            folder.setText(FileProblems.text(list.problem));
        } else if (list != null) {
            folder.setText(context.getString(R.string.files_folder, list.folder,
                    Formatter.formatShortFileSize(context, list.freeBytes)));
            if (downloadAccess == FileProblems.Access.READY) {
                if (list.files.isEmpty()) keys.add("empty");
                for (RemoteFiles.Entry entry : list.files) {
                    keys.add(entry.name + "\n" + entry.size + "\n" + entry.modifiedUnixMs);
                }
            }
        }
        if (keys.equals(shownFiles)) return;
        FileTransferActivity.Focus focused = focusIn(files);
        files.removeAllViews();
        shownFiles.clear();
        shownFiles.addAll(keys);
        if (keys.isEmpty() || list.files.isEmpty()) {
            if (!keys.isEmpty()) text(files, android.R.style.TextAppearance_Material_Body1).setText(R.string.files_empty);
            restoreFocus(files, focused, refresh);
            return;
        }
        for (RemoteFiles.Entry entry : list.files) {
            LinearLayout item = new LinearLayout(context);
            item.setOrientation(LinearLayout.VERTICAL);
            TextView name = text(item, android.R.style.TextAppearance_Material_Subhead);
            name.setText(entry.name);
            TextView detail = text(item, android.R.style.TextAppearance_Material_Body1);
            detail.setText(context.getString(R.string.files_entry, Formatter.formatShortFileSize(context, entry.size),
                    DateUtils.formatDateTime(context, entry.modifiedUnixMs,
                            DateUtils.FORMAT_SHOW_DATE | DateUtils.FORMAT_SHOW_TIME)));
            LinearLayout actions = new LinearLayout(context);
            item.addView(actions);
            Button download = button(actions, R.string.files_download);
            download.setTag("download:" + entry.name);
            download.setOnClickListener(v -> onDownload.accept(entry));
            files.addView(item);
        }
        restoreFocus(files, focused, refresh);
    }

    /** What decides a transfer row's views: the transfer, whether it has a progress bar, and its buttons. */
    static String rowKey(FileRow row) {
        return row.id + "\n" + (row.state == FileRow.State.RUNNING) + "\n" + (!row.isFinished() && !row.cancelling)
                + "\n" + row.canRetry;
    }

    /** The other button of a focused transfer button's row: Try again for Cancel and the other way round. */
    static Object otherButton(FileTransferActivity.Focus focus) {
        if (focus == null || !(focus.tag instanceof String)) return null;
        String tag = (String) focus.tag;
        if (tag.startsWith("cancel:")) return "retry:" + tag.substring("cancel:".length());
        if (tag.startsWith("retry:")) return "cancel:" + tag.substring("retry:".length());
        return null;
    }

    static boolean takesFocus(View view) {
        return view.getVisibility() == View.VISIBLE && view.requestFocus();
    }

    /** The state line of a transfer. */
    static String describe(Context context, FileRow row) {
        switch (row.state) {
            case QUEUED:
                return context.getString(R.string.files_queued);
            case RUNNING:
                if (row.cancelling) return context.getString(R.string.files_cancelling);
                if (row.percent() < 0) {
                    return context.getString(R.string.files_receiving_unknown, Formatter.formatShortFileSize(context, row.done));
                }
                return context.getString(row.upload ? R.string.files_sending : R.string.files_receiving, row.percent(),
                        Formatter.formatShortFileSize(context, row.done), Formatter.formatShortFileSize(context, row.total));
            case DONE:
                if (!row.upload) return context.getString(R.string.files_saved);
                return row.savedName != null && !row.savedName.equals(row.name) ?
                        context.getString(R.string.files_sent_as, row.savedName) : context.getString(R.string.files_sent);
            case CANCELLED:
                return context.getString(R.string.files_cancelled);
            case FAILED:
            default:
                return context.getString(FileProblems.text(row.problem));
        }
    }

    private int dp(int value) {
        return (int) TypedValue.applyDimension(TypedValue.COMPLEX_UNIT_DIP, value, context.getResources().getDisplayMetrics());
    }

    private TextView text(LinearLayout parent, int appearance) {
        TextView view = new TextView(context);
        view.setTextAppearance(context, appearance);
        view.setPadding(0, dp(8), 0, dp(4));
        parent.addView(view);
        return view;
    }

    private Button button(LinearLayout parent, int label) {
        return button(parent, context.getText(label));
    }

    private Button button(LinearLayout parent, CharSequence label) {
        Button button = new MaterialButton(context, null, com.google.android.material.R.attr.materialButtonOutlinedStyle);
        button.setText(label);
        LinearLayout.LayoutParams params = new LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.WRAP_CONTENT, LinearLayout.LayoutParams.WRAP_CONTENT);
        params.setMarginEnd(dp(8));
        parent.addView(button, params);
        return button;
    }

    /** The focus in {@code group}, or null when it is elsewhere. */
    private static FileTransferActivity.Focus focusIn(LinearLayout group) {
        View focused = group.findFocus();
        if (focused == null) return null;
        View row = focused;
        while (row.getParent() != group) row = (View) row.getParent();
        return new FileTransferActivity.Focus(focused.getTag(), group.indexOfChild(row));
    }

    /**
     * Gives the focus back after {@code group} was made again: to the view with the same tag, else to one of
     * {@code others}, else to the row now at the old index or the nearest one after or before it that takes the
     * focus, else to {@code last}. Without a place it falls to the toolbar's Up button, and the next OK of a
     * remote closes the screen and cancels the transfers.
     */
    private static void restoreFocus(LinearLayout group, FileTransferActivity.Focus focus, View last, Object... others) {
        if (focus == null) return;
        if (focus.tag != null && focusTagged(group, focus.tag)) return;
        for (Object other : others) {
            if (other != null && focusTagged(group, other)) return;
        }
        int count = group.getChildCount();
        for (int step = 0; step < count; step++) {
            int after = focus.index + step;
            int before = focus.index - step - 1;
            if (after < count && takesFocus(group.getChildAt(after))) return;
            if (before >= 0 && before < count && takesFocus(group.getChildAt(before))) return;
        }
        if (last != null && last.isShown()) last.requestFocus();
    }

    private static boolean focusTagged(LinearLayout group, Object tag) {
        View view = group.findViewWithTag(tag);
        return view != null && takesFocus(view);
    }

    /** A transfer row's views that change while it runs. */
    private static final class RowViews {
        TextView title;
        TextView detail;
        ProgressBar bar;
    }

    private String rowTitle(FileRow row) {
        return row.upload && !row.name.equals(row.localName) ?
                context.getString(R.string.files_upload_name, row.localName, row.name) : row.upload ? row.name : row.localName;
    }

    private void updateRow(RowViews views, FileRow row) {
        views.title.setText(rowTitle(row));
        views.detail.setText(describe(context, row));
        if (views.bar != null) {
            int percent = row.percent();
            views.bar.setIndeterminate(percent < 0);
            if (percent >= 0) views.bar.setProgress(percent);
        }
    }

    private View transferRow(FileRow row, LongConsumer onCancel, Consumer<FileRow> onRetry) {
        LinearLayout item = new LinearLayout(context);
        item.setOrientation(LinearLayout.VERTICAL);
        item.setPadding(0, dp(6), 0, dp(6));
        RowViews views = new RowViews();
        item.setTag(views);
        views.title = text(item, android.R.style.TextAppearance_Material_Subhead);
        views.detail = text(item, android.R.style.TextAppearance_Material_Body1);
        if (row.state == FileRow.State.RUNNING) {
            views.bar = new ProgressBar(context, null, android.R.attr.progressBarStyleHorizontal);
            views.bar.setMax(100);
            item.addView(views.bar);
        }
        updateRow(views, row);
        LinearLayout actions = new LinearLayout(context);
        item.addView(actions);
        if (!row.isFinished() && !row.cancelling) {
            Button cancel = button(actions, android.R.string.cancel);
            cancel.setTag("cancel:" + row.id);
            cancel.setOnClickListener(v -> onCancel.accept(row.id));
        }
        if (row.canRetry) {
            Button retry = button(actions, R.string.files_retry);
            retry.setTag("retry:" + row.id);
            retry.setOnClickListener(v -> onRetry.accept(row));
        }
        return item;
    }
}
