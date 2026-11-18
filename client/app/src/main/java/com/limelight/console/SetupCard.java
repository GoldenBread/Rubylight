package com.limelight.console;

import android.content.Context;
import android.content.SharedPreferences;

import androidx.annotation.VisibleForTesting;

import com.limelight.LimeLog;
import com.limelight.R;
import com.limelight.computers.ComputerDatabaseManager;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.preferences.SimpleProfile;

/**
 * Home's set-up card: after the first pairing it says what the automatic settings will use for this device and that
 * PC (resolution, frame rate, bitrate, HDR and the codec), so "is it set up right?" has an answer without opening
 * Settings. There is nothing to press but its close button, and once closed it never returns.
 *
 * <p>The card speaks for Recommended's Auto only: while the stream's settings are the player's own (Custom) it does
 * not show. Auto leaves the codec to the stream, which negotiates the best one both ends decode when it starts
 * (nightjar-policy's simple_profile), so before a stream ran there is no codec to name and the card says so.
 */
public final class SetupCard {
    /** State, not settings: it is no part of a backup of preferences, a profile or a reset. */
    private static final String PREFS = "home_setup_card";
    /** The PC the card is for; absent when there is no card to show. */
    private static final String HOST = "host";
    /** A pairing was seen: the card was offered, or this was not the first PC, and no later pairing offers it. */
    private static final String SEEN = "seen";

    /** What the card says. */
    public static final class Text {
        public final String title;
        public final String values;

        Text(String title, String values) {
            this.title = title;
            this.values = values;
        }
    }

    private SetupCard() {
    }

    private static SharedPreferences state(Context context) {
        return context.getSharedPreferences(PREFS, Context.MODE_PRIVATE);
    }

    /** A pairing succeeded. The first one this device ever makes offers the card; with another PC already paired it is not the first. */
    public static void offer(Context context, ComputerDetails computer) {
        if (computer == null || computer.uuid == null || state(context).getBoolean(SEEN, false)) return;
        int others = otherPairedPcs(context, computer.uuid);
        if (others != UNREAD) offer(context, computer.uuid, others);
    }

    @VisibleForTesting
    static void offer(Context context, String uuid, int otherPairedPcs) {
        SharedPreferences state = state(context);
        if (state.getBoolean(SEEN, false)) return;
        SharedPreferences.Editor edit = state.edit().putBoolean(SEEN, true);
        if (otherPairedPcs == 0) edit.putString(HOST, uuid);
        edit.apply();
    }

    /**
     * The app starts. A device with a PC paired and no pairing seen came from a version before the card: its first
     * pairing is long done, so no later one (the same PC paired again, say) offers it.
     */
    public static void atStart(Context context) {
        if (state(context).getBoolean(SEEN, false)) return;
        if (otherPairedPcs(context, "") > 0) state(context).edit().putBoolean(SEEN, true).apply();
    }

    /** For the debug build's demo and its captures: the card as the first pairing with this PC leaves it. */
    public static void show(Context context, String uuid) {
        state(context).edit().putString(HOST, uuid).putBoolean(SEEN, true).apply();
    }

    /** The saved list could not be read: nothing is known, so no card is offered and nothing is marked. */
    private static final int UNREAD = -1;

    /** The PCs paired besides this one, from the saved list, or {@link #UNREAD}. */
    private static int otherPairedPcs(Context context, String uuid) {
        ComputerDatabaseManager database = null;
        try {
            database = new ComputerDatabaseManager(context);
            int others = 0;
            for (ComputerDetails saved : database.getAllComputers()) {
                if (saved.serverCert != null && !uuid.equals(saved.uuid)) others++;
            }
            return others;
        } catch (RuntimeException e) {
            LimeLog.warning("Set-up card: the saved PCs could not be read: " + e);
            return UNREAD;
        } finally {
            if (database != null) database.close();
        }
    }

    /** The PC the card is for, or null when there is none to show. */
    static String host(Context context) {
        return state(context).getString(HOST, null);
    }

    /** The card was closed: it does not come back. */
    static void dismiss(Context context) {
        state(context).edit().remove(HOST).putBoolean(SEEN, true).apply();
    }

    /** What the card says for {@code host}, or null while the stream's settings are not Recommended's Auto. */
    static Text text(Context context, ComputerDetails host) {
        if (!SimpleProfile.allAuto(context)) return null;
        PreferenceConfiguration config = PreferenceConfiguration.readPreferences(context, host.uuid, host.caps, null, 0, null);
        return new Text(context.getString(R.string.home_setup_title, host.displayName()),
                values(context, config.width, config.height, config.fps, config.bitrate,
                        // Recommended's HDR on Auto is on where the screen and the decoder show HDR, as its row says.
                        config.enableHdr || SimpleProfile.hdrOnAuto(context), config.videoFormat));
    }

    /** "2560×1440 · 120 FPS · 60 Mbps · HDR · codec chosen when the stream starts": the values a stream to the PC starts with. */
    static String values(Context context, int width, int height, int fps, int bitrateKbps, boolean hdr,
                         PreferenceConfiguration.FormatOption format) {
        return StreamProfileSummary.values(context, width, height, fps, bitrateKbps)
                + " · " + (hdr ? "HDR" : "SDR")
                + " · " + codec(context, format);
    }

    private static String codec(Context context, PreferenceConfiguration.FormatOption format) {
        switch (format) {
            case FORCE_AV1: return "AV1";
            case FORCE_HEVC: return "HEVC";
            case FORCE_H264: return "H.264";
            case FORCE_PYROWAVE: return "PyroWave";
            default: return context.getString(R.string.home_setup_codec_auto);
        }
    }
}
