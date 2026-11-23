package com.limelight.computers;

import android.content.Context;

import com.limelight.nvstream.jni.NightjarHostsJni;

/**
 * The names people give their PCs, kept on this device only, backed by Rust {@code HostMetaStore}
 * ({@code host_meta.json}) via {@link NightjarHostsJni}.
 */
public final class HostNames {
    /** The longest alias, in characters the way people count them. */
    public static final int MAX_LENGTH = 40;

    static final String PREFERENCES = "HostNames";

    private HostNames() {
    }

    /** The alias for this PC, or null when it shows its own name. */
    public static String get(Context context, String hostUuid) {
        if (context == null || hostUuid == null) {
            return null;
        }
        long handle = HostMetaStores.openSource(context, PREFERENCES);
        String alias = NightjarHostsJni.metaAliasGet(handle, hostUuid);
        return alias == null || alias.isEmpty() ? null : alias;
    }

    /**
     * What to store for text typed in the rename dialog: trimmed and cut to {@link #MAX_LENGTH}, or null
     * when it is empty or just the PC's own name, which both mean "use the PC's own name".
     */
    public static String normalize(String typed, String ownName) {
        return NightjarHostsJni.metaAliasNormalize(typed, ownName);
    }

    /** Stores the alias, or drops it when {@code alias} is null. */
    public static synchronized void set(Context context, String hostUuid, String alias) {
        if (context == null || hostUuid == null) {
            return;
        }
        long handle = HostMetaStores.openSource(context, PREFERENCES);
        NightjarHostsJni.metaAliasSet(handle, hostUuid, alias);
    }

    public static void forget(Context context, String hostUuid) {
        set(context, hostUuid, null);
    }
}
