package com.limelight.update;

import com.limelight.nvstream.jni.NightjarPolicyJni;

/** Turns a release's Markdown body into short plain text for the update dialog. */
public final class ReleaseNotes {
    private ReleaseNotes() {
    }

    public static String plain(String markdown, int maxChars) {
        if (markdown == null) {
            return "";
        }
        return NightjarPolicyJni.releaseNotesPlain(markdown, Math.max(0, maxChars));
    }
}
