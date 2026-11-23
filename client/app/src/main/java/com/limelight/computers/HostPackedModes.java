package com.limelight.computers;

import android.content.Context;

import com.limelight.nvstream.jni.NightjarHostsJni;

/**
 * The packed full-chroma modes each PC offered in its last RTSP DESCRIBE, backed by Rust
 * {@code HostMetaStore} ({@code host_meta.json}) via {@link NightjarHostsJni}.
 */
public final class HostPackedModes {
    static final String PREFERENCES = "HostPacked444Modes";
    // The modes this client knows; bits a newer host adds are not its business.
    static final int ALL_MODES = 0xF;

    private HostPackedModes() {
    }

    /** The modes this PC's last DESCRIBE offered, or -1 when none was seen. */
    public static int cached(Context context, String hostUuid) {
        if (context == null || hostUuid == null) return -1;
        long handle = HostMetaStores.openSource(context, PREFERENCES);
        return NightjarHostsJni.metaPackedCached(handle, hostUuid);
    }

    /**
     * Remembers what a DESCRIBE offered. No modes (or an attribute the client could not read)
     * forgets the PC instead: that is a host with full chroma off, which serverinfo then omits too.
     * A negative value is no DESCRIBE at all and changes nothing.
     * Mode bits this client does not know are left out.
     */
    public static void store(Context context, String hostUuid, int describeModes) {
        if (context == null || hostUuid == null || describeModes < 0) return;
        long handle = HostMetaStores.openSource(context, PREFERENCES);
        NightjarHostsJni.metaPackedStore(handle, hostUuid, describeModes);
    }

    /**
     * This PC refused a stream that asked for full chroma.
     */
    public static void markRefused(Context context, String hostUuid) {
        if (context == null || hostUuid == null) return;
        long handle = HostMetaStores.openSource(context, PREFERENCES);
        NightjarHostsJni.metaPackedMarkRefused(handle, hostUuid);
    }

    /**
     * Whether this PC's last answer to a full chroma stream was a refusal.
     */
    public static boolean refused(Context context, String hostUuid) {
        if (context == null || hostUuid == null) return false;
        long handle = HostMetaStores.openSource(context, PREFERENCES);
        return NightjarHostsJni.metaPackedRefused(handle, hostUuid);
    }

    /**
     * What a PC's answer is given under: this app's version and the PC's own.
     */
    public static String stamp(String appVersion, String hostVersion) {
        return NightjarHostsJni.metaPackedStamp(appVersion, hostVersion);
    }

    /**
     * A launch that asks about full chroma under {@code stamp}.
     */
    public static void renew(Context context, String hostUuid, String stamp) {
        if (context == null || hostUuid == null || stamp == null) return;
        long handle = HostMetaStores.openSource(context, PREFERENCES);
        NightjarHostsJni.metaPackedRenew(handle, hostUuid, stamp);
    }

    /** Every PC is asked again: the user switched the setting. */
    public static void clearRefusals(Context context) {
        if (context == null) return;
        long handle = HostMetaStores.openSource(context, PREFERENCES);
        NightjarHostsJni.metaPackedClearRefusals(handle);
    }

    /** What the next launch takes the PC to offer: its serverinfo when it says, else the cache, else -1. */
    public static int known(int serverInfoModes, int cachedModes) {
        return NightjarHostsJni.metaPackedKnown(serverInfoModes, cachedModes);
    }
}
