/**
 * Pure-JDK JNI bridge to Rust mDNS discovery tracking, Wake-on-LAN planning/sending/storage/poll,
 * and RFC 5389 STUN packet encode/decode ({@code nightjar_core::{mdns, wol, stun}}).
 */
package com.limelight.nvstream.jni;

public final class NightjarDiscoveryJni {
    static {
        NightjarJni.ensureLibraryLoaded();
    }

    private NightjarDiscoveryJni() {
    }

    // mDNS discovery deduplication & IPv6 ranking
    public static native long mdnsCreate();
    public static native void mdnsDestroy(long handle);
    public static native long mdnsReset(long handle);
    public static native long mdnsGeneration(long handle);
    public static native int[] mdnsSelectIpv6(String addrsJson);
    public static native int mdnsLinkLocalIpv6(String addrsJson);
    public static native int mdnsBeginCandidate(
            long handle, String name, int port, String localAddr, String v6Addr, long generation);
    public static native void mdnsMarkAnswered(
            long handle, String name, int port, String localAddr, String v6Addr);
    public static native void mdnsForgetAnswered(long handle);
    public static native String mdnsComputers(long handle);

    // Wake-on-LAN primitives, planner, UDP sender, store, poll cooldown & retry schedule
    public static native byte[] wolMagicPacket(String mac);
    public static native int[] wolPorts(int httpPort);
    public static native String wolParseMacs(String text);
    public static native byte[] wolIpv4Literal(String literal);
    public static native String wolPlan(String requestJson);
    public static native String wolSend(String requestJson);

    public static native long wakeTargetsOpen(String dirPath);
    public static native void wakeTargetsDestroy(long handle);
    public static native boolean wakeTargetsImportLegacy(long handle, String legacyJson);
    public static native String wakeTargetsLoad(long handle, String uuid);
    public static native String wakeTargetsManual(long handle, String uuid);
    public static native void wakeTargetsStoreManual(long handle, String uuid, String macsJson);
    public static native void wakeTargetsStoreWake(
            long handle, String uuid, String primary, String targetsJson);
    public static native void wakeTargetsForget(long handle, String uuid);
    public static native void wakeTargetsForgetHost(long handle, String uuid);
    public static native boolean wakeTargetsHasMac(long handle, String uuid, String primaryMac);

    public static native long wakePollCreate();
    public static native void wakePollDestroy(long handle);
    public static native long wakePollReserve(
            long handle, String uuid, boolean online, boolean hasNativeApi, long nowMs);
    public static native boolean wakePollIsActive(long handle, String uuid, long token);
    public static native void wakePollDrop(long handle, String uuid, long token);
    public static native void wakePollForget(long handle, String uuid);
    public static native void wakePollShutdown(long handle);
    public static native int wakePollPendingCount(long handle);

    public static native long wakeLaunchCreate(long startMs, long timeoutMs, long resendMs);
    public static native void wakeLaunchDestroy(long handle);
    public static native int wakeLaunchNext(long handle, boolean online, long nowMs);

    // STUN RFC 5389 packet codec
    public static native byte[] stunBindingRequest(byte[] transaction);
    public static native String stunMappedAddress(byte[] response, byte[] transaction);
}
