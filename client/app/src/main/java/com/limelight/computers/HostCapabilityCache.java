package com.limelight.computers;

import android.content.Context;

import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.jni.NightjarHostsJni;

/**
 * The last capabilities each PC reported, keyed by host ID, backed by Rust {@code HostMetaStore}
 * ({@code host_meta.json}) via {@link NightjarHostsJni}.
 */
public final class HostCapabilityCache {
    static final String PREFERENCES = "HostCapabilities";

    private HostCapabilityCache() {
    }

    /** What the PC last reported, or {@link HostCapabilities#UNKNOWN} if it never answered. */
    public static HostCapabilities load(Context context, String hostUuid) {
        if (context == null || hostUuid == null) {
            return HostCapabilities.UNKNOWN;
        }
        long handle = HostMetaStores.openSource(context, PREFERENCES);
        return HostCapabilities.fromJson(NightjarHostsJni.metaCapLoad(handle, hostUuid));
    }

    /** Remembers known capabilities; polling repeats them, so unchanged ones are not written again. */
    public static void save(Context context, String hostUuid, HostCapabilities caps) {
        if (context == null || hostUuid == null || caps == null || !caps.isKnown()) {
            return;
        }
        long handle = HostMetaStores.openSource(context, PREFERENCES);
        NightjarHostsJni.metaCapSave(handle, hostUuid, caps.toJson());
    }

    public static void forget(Context context, String hostUuid) {
        if (context != null && hostUuid != null) {
            long handle = HostMetaStores.openSource(context, PREFERENCES);
            NightjarHostsJni.metaCapForget(handle, hostUuid);
        }
    }
}
