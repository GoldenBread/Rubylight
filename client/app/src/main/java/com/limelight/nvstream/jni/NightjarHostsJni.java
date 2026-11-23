/**
 * Pure-JDK JNI bridge to Rust unified host registry ({@code hosts.json}), per-host metadata
 * ({@code host_meta.json}), and client pairing identity ({@code identity.json}) stores
 * ({@code nightjar_core::hosts}).
 */
package com.limelight.nvstream.jni;

public final class NightjarHostsJni {
    static {
        NightjarJni.ensureLibraryLoaded();
    }

    private NightjarHostsJni() {
    }

    // Unified HostStore (`hosts.json`)
    public static native long hostStoreOpen(String dirPath);
    public static native void hostStoreDestroy(long handle);
    public static native String hostStoreAll(long handle);
    public static native String hostStoreGetByUuid(long handle, String uuid);
    public static native String hostStoreGetByName(long handle, String name);
    public static native boolean hostStoreUpsert(long handle, String hostJson);
    public static native boolean hostStoreDelete(long handle, String uuid);
    public static native boolean hostStoreHasMigrated(long handle);
    public static native boolean hostStoreImportLegacy(long handle, String legacyArrayJson);

    // Per-host metadata (`host_meta.json`)
    public static native long metaStoreOpen(String dirPath);
    public static native void metaStoreDestroy(long handle);
    public static native boolean metaNeedsImport(long handle, String source);
    public static native boolean metaImportLegacy(long handle, String source, String entriesJson);

    public static native boolean metaClaimNotice(long handle, String uuid, String noticeName);
    public static native void metaResetNotice(long handle, String uuid, String noticeName);
    public static native void metaResetNoticeEverywhere(long handle, String noticeName);
    public static native void metaForgetNotices(long handle, String uuid);

    public static native int metaPackedCached(long handle, String uuid);
    public static native boolean metaPackedStore(long handle, String uuid, int describeModes);
    public static native boolean metaPackedMarkRefused(long handle, String uuid);
    public static native boolean metaPackedRefused(long handle, String uuid);
    public static native String metaPackedStamp(String appVersion, String hostVersion);
    public static native void metaPackedRenew(long handle, String uuid, String stamp);
    public static native void metaPackedClearRefusals(long handle);
    public static native int metaPackedKnown(int serverInfoModes, int cachedModes);

    public static native String metaCapLoad(long handle, String uuid);
    public static native boolean metaCapSave(long handle, String uuid, String capsJson);
    public static native void metaCapForget(long handle, String uuid);

    public static native String metaAliasGet(long handle, String uuid);
    public static native String metaAliasNormalize(String typed, String ownName);
    public static native void metaAliasSet(long handle, String uuid, String alias);

    public static native String metaPinnedGet(long handle, String uuid);
    public static native void metaPinnedSet(long handle, String uuid, String address, int port);

    public static native void metaForget(long handle, String uuid);

    // Client pairing identity (`identity.json`)
    public static native String identityLoadOrCreate(
            String dirPath, String legacyUniqueId, String legacyCertPem, byte[] legacyKeyPkcs8);
}
