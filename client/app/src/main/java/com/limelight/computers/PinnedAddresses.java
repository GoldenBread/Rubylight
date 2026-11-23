package com.limelight.computers;

import android.content.Context;

import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.jni.Json;
import com.limelight.nvstream.jni.NightjarHostsJni;

/**
 * The address each PC is pinned to, kept on this device only beside the database, backed by Rust
 * {@code HostMetaStore} ({@code host_meta.json}) via {@link NightjarHostsJni}.
 */
public final class PinnedAddresses {
    static final String PREFERENCES = "PinnedAddresses";

    private PinnedAddresses() {
    }

    /** The address this PC is pinned to, or null for the automatic pick. */
    public static ComputerDetails.AddressTuple get(Context context, String hostUuid) {
        if (context == null || hostUuid == null) {
            return null;
        }
        long handle = HostMetaStores.openSource(context, PREFERENCES);
        String raw = NightjarHostsJni.metaPinnedGet(handle, hostUuid);
        if (raw == null) {
            return null;
        }
        Json.Obj obj = Json.parseObject(raw);
        String address = obj.optString("address", null);
        int port = obj.optInt("port", 0);
        if (address == null || port <= 0 || port > 65535) {
            return null;
        }
        return new ComputerDetails.AddressTuple(address, port);
    }

    /** Pins the PC to {@code address}, or brings back the automatic pick when it is null. */
    public static synchronized void set(Context context, String hostUuid, ComputerDetails.AddressTuple address) {
        if (context == null || hostUuid == null) {
            return;
        }
        long handle = HostMetaStores.openSource(context, PREFERENCES);
        if (address == null) {
            NightjarHostsJni.metaPinnedSet(handle, hostUuid, null, 0);
        } else {
            NightjarHostsJni.metaPinnedSet(handle, hostUuid, address.address, address.port);
        }
    }
}
