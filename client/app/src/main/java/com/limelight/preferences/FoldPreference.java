package com.limelight.preferences;

import android.content.Context;
import android.os.Bundle;
import android.preference.Preference;
import android.util.AttributeSet;
import android.view.View;

import androidx.core.view.AccessibilityDelegateCompat;
import androidx.core.view.ViewCompat;
import androidx.core.view.accessibility.AccessibilityNodeInfoCompat;

import com.limelight.R;

/**
 * A row that unfolds the rows under it and folds them again (the overview's More settings). It stores nothing. Its
 * icon points down, or up once it is open, and a screen reader hears which it is: the row offers Expand while it is
 * folded and Collapse while it is open, which TalkBack announces as "collapsed" and "expanded" in the user's language.
 */
public class FoldPreference extends Preference {
    private boolean expanded;

    public FoldPreference(Context context, AttributeSet attrs) {
        super(context, attrs);
        setPersistent(false);
        setIcon(R.drawable.ic_expand_more);
    }

    public boolean isExpanded() {
        return expanded;
    }

    /** What the row shows and says; the rows themselves are the owner's to show and remove. */
    public void setExpanded(boolean expanded) {
        this.expanded = expanded;
        // A changed icon rebinds the row, so its accessibility node follows.
        setIcon(expanded ? R.drawable.ic_expand_less : R.drawable.ic_expand_more);
    }

    @Override
    protected void onBindView(View view) {
        super.onBindView(view);
        ViewCompat.setAccessibilityDelegate(view, new AccessibilityDelegateCompat() {
            @Override
            public void onInitializeAccessibilityNodeInfo(View host, AccessibilityNodeInfoCompat info) {
                super.onInitializeAccessibilityNodeInfo(host, info);
                info.addAction(expanded ? AccessibilityNodeInfoCompat.AccessibilityActionCompat.ACTION_COLLAPSE
                        : AccessibilityNodeInfoCompat.AccessibilityActionCompat.ACTION_EXPAND);
            }

            @Override
            public boolean performAccessibilityAction(View host, int action, Bundle args) {
                boolean fold = action == AccessibilityNodeInfoCompat.ACTION_COLLAPSE;
                if ((fold || action == AccessibilityNodeInfoCompat.ACTION_EXPAND) && fold == expanded) {
                    OnPreferenceClickListener listener = getOnPreferenceClickListener();
                    return listener != null && listener.onPreferenceClick(FoldPreference.this);
                }
                return super.performAccessibilityAction(host, action, args);
            }
        });
    }
}
