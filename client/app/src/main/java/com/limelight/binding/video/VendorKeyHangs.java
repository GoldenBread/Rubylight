package com.limelight.binding.video;

import android.content.Context;
import android.content.SharedPreferences;
import android.os.Build;

import com.limelight.BuildConfig;
import com.limelight.LimeLog;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;

/**
 * Decoders that took no input with their vendor low-latency keys, also after a reset, and then ran
 * well without them: the next streams start such a decoder without the keys. One record per decoder
 * name and MIME type, in a preferences file of its own, for one app version on one system build. An
 * update of either drops every record, so the keys are tried again once the keys we send or the
 * decoder may have changed. The vendor-keys setting does not clear them: its own switch, the
 * low-latency master switch, presets and profiles all write it, and turning it off and on again would
 * be no way back a user could know of.
 */
final class VendorKeyHangs {
    static final String PREFS = "vendor-key-hangs";
    static final String STAMP = "stamp";
    private static final String SEPARATOR = ", ";

    private VendorKeyHangs() {}

    /**
     * The record's name: the decoder and the MIME type it decodes, not the stream's depth or HDR. A
     * hang in an HDR Main10 stream drops the keys for SDR on that decoder too: the TCL C6K hung in both,
     * and the user is spared a second hang.
     */
    static String key(String decoderName, String mimeType) {
        return decoderName + "|" + mimeType;
    }

    /** What the records hold for: this app version on this system build. */
    static String stamp() {
        return BuildConfig.VERSION_CODE + "|" + Build.FINGERPRINT;
    }

    /** The keys the decoder of this key took no input with in an earlier stream, or null without a record. */
    static List<String> recorded(Context context, String key) {
        String keys = prefs(context).getString(key, null);
        if (keys == null) return null;
        return keys.isEmpty() ? new ArrayList<>() : new ArrayList<>(Arrays.asList(keys.split(SEPARATOR)));
    }

    /** Records that the decoder of this key took no input with these keys and ran well without them. */
    static void record(Context context, String key, List<String> keys) {
        prefs(context).edit().putString(key, String.join(SEPARATOR, keys)).apply();
    }

    private static SharedPreferences prefs(Context context) {
        SharedPreferences prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE);
        String stamp = stamp();
        String stored = prefs.getString(STAMP, null);
        if (!stamp.equals(stored)) {
            if (prefs.getAll().size() > (stored == null ? 0 : 1)) {
                LimeLog.info("Decoders that took no input with their vendor low-latency keys get them again" +
                        " after an app or system update");
            }
            prefs.edit().clear().putString(STAMP, stamp).apply();
        }
        return prefs;
    }
}
