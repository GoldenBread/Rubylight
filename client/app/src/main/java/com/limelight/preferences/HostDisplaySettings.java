package com.limelight.preferences;

import android.app.Activity;
import android.widget.Toast;
import androidx.appcompat.app.AlertDialog;

import com.limelight.ui.NavigationDialogBuilder;
import com.limelight.R;
import com.limelight.utils.UiHelper;

/** The PC page's Display arrangement picker. Only Save commits a choice for the next stream. */
public final class HostDisplaySettings {
    private HostDisplaySettings() {
    }

    /**
     * The PC page's Display arrangement; created separately so it can restore an unconfirmed selection after rotation.
     * Its first entry, {@code sameAs}, follows the main settings again, as the page's other rows offer; {@code own}
     * says whether this PC has a display choice of its own.
     */
    static AlertDialog create(Activity activity, String hostUuid, String hostName, CharSequence sameAs, boolean own,
                              boolean virtualDisplay, java.util.function.Consumer<Boolean> setVirtualDisplay, Runnable onChanged) {
        // Display arrangement with Virtual display folded in (DisplayArrangementChoice): Save stores the
        // arrangement here and hands the switch to the page, which saves it as this PC's own value (null: none).
        CharSequence[] names = DisplayArrangementChoice.names(activity, true);
        CharSequence[] labels = new CharSequence[names.length + 1];
        labels[0] = sameAs;
        System.arraycopy(names, 0, labels, 1, names.length);
        int checked = own ? 1 + DisplayArrangementChoice.choice(HostDisplayChoice.load(activity, hostUuid), virtualDisplay, true) : 0;
        return new NavigationDialogBuilder(activity)
                .setTitle(activity.getString(R.string.host_display_title, hostName))
                .setSingleChoiceItems(labels, checked, null)
                .setPositiveButton(R.string.host_display_save, (dialog, which) -> {
                    // Read the view state because the pending selection may have been restored.
                    int selected = ((AlertDialog) dialog).getListView().getCheckedItemPosition();
                    if (selected < 0 || selected >= labels.length) return;
                    if (selected == 0) {
                        HostDisplayChoice.forget(activity, hostUuid);
                        setVirtualDisplay.accept(null);
                    } else {
                        HostDisplayChoice.save(activity, hostUuid, DisplayArrangementChoice.ARRANGEMENTS[selected - 1]);
                        Boolean switchValue = DisplayArrangementChoice.switchFor(selected - 1);
                        if (switchValue != null) setVirtualDisplay.accept(switchValue);
                    }
                    Toast.makeText(activity, R.string.host_display_saved, Toast.LENGTH_SHORT).show();
                    if (onChanged != null) onChanged.run();
                })
                .setNeutralButton(R.string.help, null)
                .setNegativeButton(android.R.string.cancel, null)
                .create();
    }

    static void bindHelp(Activity activity, AlertDialog picker) {
        picker.getButton(AlertDialog.BUTTON_NEUTRAL).setOnClickListener(view ->
                UiHelper.showDialog(activity, new NavigationDialogBuilder(activity).setTitle(R.string.host_display_menu)
                        .setMessage(R.string.host_display_explanation)
                        .setPositiveButton(android.R.string.ok, null).create()));
    }
}
