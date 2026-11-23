/**
 * Pure-JDK JNI declarations for {@code nightjar-policy} and {@code nightjar-core} policy
 * controllers (`protocol/android/src/policy.rs`). Loads {@code nightjar_core} via
 * {@link NightjarJni#ensureLibraryLoaded()}.
 */
package com.limelight.nvstream.jni;

public final class NightjarPolicyJni {
    static {
        NightjarJni.ensureLibraryLoaded();
    }

    private NightjarPolicyJni() {
    }

    // AdaptiveBitrateController
    public static native int abrBuiltInFloorKbps(int width, int height, int fps, int codecOrdinal);
    public static native long abrCreate(int ceilingKbps, int currentKbps, int fps, long nowMs,
                                        int width, int height, int codecOrdinal);
    public static native void abrDestroy(long handle);
    public static native int abrSample(long handle, long nowMs, boolean recentVideo,
                                       boolean poorConnection, float lossPercent,
                                       float decodeMs, float jitterMs, float queueMs);
    public static native String abrLastReason(long handle);
    public static native void abrSetThermalLevel(long handle, int level);
    public static native void abrSetFrameRate(long handle, int fps);
    public static native void abrSetStreamSize(long handle, int width, int height);
    public static native boolean abrHasRoomBeforeQualityReduction(long handle);
    public static native void abrApplied(long handle, int requestedKbps, int appliedKbps, long nowMs);
    public static native void abrSuspend(long handle);
    public static native int abrCeilingKbps(long handle);
    public static native int abrFloorKbps(long handle);

    // ClientHintBitrateController
    public static native int[] clientHintRange(int initialKbps);
    public static native long clientHintCreate(int initialKbps, int limitKbps, long nowMs);
    public static native void clientHintDestroy(long handle);
    public static native void clientHintResumeFrom(long handle, int kbps);
    public static native int clientHintSample(long handle, long nowMs, boolean recentVideo,
                                              float lossPercent, float queueMs);
    public static native void clientHintApplied(long handle, int kbps, long nowMs);
    public static native void clientHintRefused(long handle);
    public static native void clientHintOverridden(long handle, int kbps, long nowMs);
    public static native int[] clientHintState(long handle);

    // PyroWaveBitrateFloor
    public static native int pyroFloorMinimumKbps(int width, int height, int fps);
    public static native int pyroFloorDefaultKbps(int width, int height, int fps, int linkMbps);
    public static native int pyroFloorStartKbps(int configured, boolean automatic, int width,
                                                int height, int fps, int linkMbps);
    public static native int[] pyroFloorLinkNotice(int width, int height, int fps, int linkMbps,
                                                   int configuredKbps);

    // PyroWaveBitrateController
    public static native long pyroAbrCreate(int ceilingKbps, int currentKbps, int fps, long nowMs);
    public static native void pyroAbrDestroy(long handle);
    public static native int pyroAbrSample(long handle, long nowMs, boolean recentVideo,
                                           boolean poorConnection, float lossPercent,
                                           float queueMs, float decodeMs, float jitterMs,
                                           int thermalLevel);
    public static native void pyroAbrApplied(long handle, int requestedKbps, int appliedKbps, long nowMs);
    public static native void pyroAbrSuspend(long handle);

    // AdaptiveQualityController
    public static native double qualityDecodeBudgetMs(int fps);
    public static native long qualityCreate(int modeOrdinal, int width, int height, int fps,
                                            int minShortEdge, int minFps, int alignWidth,
                                            int alignHeight, long nowMs);
    public static native void qualityDestroy(long handle);
    public static native void qualityResumeAt(long handle, int width, int height, int fps);
    public static native int[] qualityCeiling(long handle);
    public static native int[] qualityCurrent(long handle);
    public static native int[] qualityLevels(long handle);
    public static native int[] qualityResizeCeiling(long handle, int width, int height, int fps, long nowMs);
    public static native int[] qualitySample(long handle, long nowMs, boolean recentVideo,
                                             boolean poorConnection, float lossPercent,
                                             float decodeMs, float jitterMs, float queueMs,
                                             int thermalLevel, boolean bitrateHasRoom,
                                             boolean observeTail, boolean hasTiming,
                                             long timingGeneration, long timingWindowStartNs,
                                             long timingWindowEndNs, long timingFirstSampleNs,
                                             long timingLastSampleNs, int timingFps,
                                             int timingSamples, int timingLateFrames);
    public static native void qualityApplied(long handle, int width, int height, int fps, long nowMs);
    public static native void qualitySuspend(long handle);

    // FoundationAbr
    public static native long foundationAbrCreate(int initialKbps, int streamKbps, long nowMs);
    public static native void foundationAbrDestroy(long handle);
    public static native void foundationAbrAdd(long handle, int completed, int drops, int decoded);
    public static native void foundationAbrAddRate(long handle, double lossPercent, double fps);
    public static native String foundationAbrDue(long handle, long nowMs, double rttMs);
    public static native int foundationAbrAnswered(long handle, boolean hasAction, int newBitrate,
                                                   boolean bitrateApplied);
    public static native int foundationAbrCurrentKbps(long handle);

    // PyroWavePace + PaceSearch
    public static native int paceWireKbps(int imageKbps, int fps, int packetSize, int fec, int minParity);
    public static native int paceProbeKbps(int imageKbps, int fps, int packetSize, int fec, int minParity);
    public static native int paceFloorKbps(int probeKbps);
    public static native int pacePacketSizeFor(int offered);
    public static native long paceExpectedPackets(int kbps, int packetSize);
    public static native long paceBurstGroupPackets(int paceKbps, int packetSize, boolean ipv6);
    public static native String paceSummarize(long expected, long sent, double senderMs, boolean[] arrived,
                                              int fps, long groupPackets);
    public static native boolean paceMeasured(String measurementJson);
    public static native boolean paceQualified(String measurementJson);
    public static native int paceJumboProbeSize(int configured);
    public static native boolean paceJumboPasses(String jumboJson, String controlJson);
    public static native long paceSearchCreate(int linkKbps, int floorKbps);
    public static native void paceSearchDestroy(long handle);
    public static native int paceSearchNextPace(long handle);
    public static native boolean paceSearchIsDone(long handle);
    public static native void paceSearchRecord(long handle, int paceKbps, String measurementJson);
    public static native String paceSearchOutcome(long handle);

    // Update
    public static native String semVerParse(String text);
    public static native int semVerCompare(String a, String b);
    public static native boolean semVerIsNewer(String candidate, String current);
    public static native String semVerNormalize(String text);
    public static native String releaseInfoParse(String json);
    public static native boolean releaseInfoIsUpdateFor(String releaseJson, String currentVersion);
    public static native String releaseInfoApkAsset(String releaseJson, boolean root);
    public static native String releaseInfoChecksumAsset(String releaseJson);
    public static native String checksumFileFind(String sums, String fileName);
    public static native String checksumFileHex(byte[] bytes);
    public static native String releaseNotesPlain(String markdown, int maxChars);
    public static native boolean updatePolicyIsCheckDue(boolean autoCheck, long lastCheckMs, long nowMs);
    public static native boolean updatePolicyShouldOffer(String releaseJson, String currentVersion,
                                                         String skippedVersion);
    public static native boolean updatePolicyIsManagedByStore(String installerPackage);
    public static native boolean updatePolicyIsSelfUpdateAvailable(String installerPackage,
                                                                   boolean githubDistribution);
}
