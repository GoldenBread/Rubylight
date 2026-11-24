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

    /** Native API envelope transport callback for Sleep, Restart/Shut down, and Update flows. */
    public interface FlowTransport {
        String call(String op, String argument);
    }

    /** Interruptible sleeper callback for PC power refusal polling and host update wait loops. */
    public interface FlowSleeper {
        boolean sleep(long ms);
    }

    /** JSON step listener invoked by Rust host-flow loops. */
    public interface FlowStepListener {
        void onStep(String stepJson);
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

    // Unified HostManager + HostClientPool + ApplistPollerHandle + RNG pairing + Host flows
    public static native long hostManagerOpen(
            String configDir, String uniqueId, String certPem, String keyPem);
    public static native void hostManagerDestroy(long handle);
    public static native void hostManagerResetForTests();
    public static native void hostManagerStartPolling(long handle);
    public static native void hostManagerStopPolling(long handle);
    public static native void hostManagerWaitForPollingStopped(long handle);
    public static native long hostManagerNowMs(long handle);
    public static native String hostManagerNextEvent(long handle, long timeoutMs);
    public static native String hostManagerGetSnapshot(long handle, String uuid);
    public static native String hostManagerAddComputerBlocking(long handle, String dtoJson);
    public static native String hostManagerAddComputerForLink(
            long handle, String dtoJson, byte[] linkCertSha256);
    public static native boolean hostManagerAddInvitedComputer(
            long handle, String dtoJson, String internetPlayReply);
    public static native boolean hostManagerInviteClaimsKnownPc(
            long handle, String uuid, byte[] linkFpSha256);
    public static native void hostManagerRemoveComputer(long handle, String uuid);
    public static native void hostManagerPinAddress(
            long handle, String uuid, String address, int port);
    public static native void hostManagerForgetCertificate(long handle, String uuid);
    public static native void hostManagerSetServerCertificate(
            long handle, String uuid, byte[] certDer);
    public static native void hostManagerRenameComputer(long handle, String uuid, String alias);
    public static native void hostManagerResetNetworkState(long handle, boolean available, long atMs);
    public static native void hostManagerInvalidateState(long handle, String uuid);
    public static native void hostManagerSetVpnActive(long handle, boolean active);
    public static native void hostManagerSetRemoteEndpoint(
            long handle, String uuid, String address, int port);
    public static native void hostManagerImportInternetPlay(
            long handle, String uuid, String replyJson);
    public static native String hostManagerPollDetails(
            long handle, String dtoJson, String state, String activeAddress, int activePort);

    public static native long applistPollerStart(
            long hostManagerHandle, String address, int httpPort, int httpsPort, byte[] certDer);
    public static native void applistPollerPollNow(long handle);
    public static native String applistPollerNextUpdate(long handle);
    public static native void applistPollerStop(long handle);

    public static native String randomPairingPin();
    public static native String hostFlowPair(
            long httpHandle,
            long managerHandle,
            int pairingType,
            String pin,
            String passphrase,
            String linkUri,
            boolean hasPinnedCert,
            boolean inGame);
    public static native int hostFlowUnpair(
            String address,
            int httpPort,
            int httpsPort,
            String uniqueId,
            byte[] clientCertDer,
            byte[] clientKeyPkcs8,
            byte[] serverCertDer,
            String deviceName);
    public static native String hostFlowRedeemInvite(
            String link,
            String uniqueId,
            String certPem,
            String keyPem,
            String deviceName,
            long nowSec);
    public static native String hostFlowSleepOffer(FlowTransport transport);
    public static native String hostFlowSleepSpend(
            FlowTransport transport,
            String token,
            long monoStartMs,
            long wallStartMs,
            long monoNowMs,
            long wallNowMs);
    public static native String hostFlowPcPowerOffer(FlowTransport transport, boolean isRestart);
    public static native void hostFlowPcPowerSpend(
            FlowTransport transport,
            boolean isRestart,
            String token,
            long monoStartMs,
            long wallStartMs,
            long monoNowMs,
            long wallNowMs,
            FlowSleeper sleeper,
            FlowStepListener listener);
    public static native void hostFlowUpdateRun(
            FlowTransport transport, FlowSleeper sleeper, FlowStepListener listener);
}
