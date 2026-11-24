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

    // Host polling & lifecycle flow policies (`nightjar_policy::hosts`)
    public static native long hostSleepTokenRefreshMs();
    public static native long hostPcPowerRefusalPollMs();
    public static native int hostPcPowerRefusalReads();
    public static native long hostUpdateWaitMs();
    public static native long hostUpdateMaxWaitMs();
    public static native long hostUpdatePollMs();

    public static native int sleepUnavailableReason(boolean sleepSupported, boolean sleepAllowed);

    public static native long pairingWatchCreate();
    public static native void pairingWatchDestroy(long handle);
    public static native void pairingWatchRefreshSoon(long handle);
    public static native void pairingWatchPause(long handle);
    public static native long pairingWatchBeginPoll(
            long handle, long nowMs, boolean hasAskedHosts);
    public static native int pairingWatchFinishPoll(
            long handle, long generation, String idsJson);

    public static native String stripBidiControls(String input);
    public static native boolean pairingRefusedStatus(int status);
    public static native boolean alreadyPaired(
            boolean isPaired, boolean hasLink, boolean hasPinnedCert);
    public static native int classifyPairingFailure(
            int stateOrdinal, boolean fromLink, boolean oneTime, boolean inGame);
    public static native int classifyPairingHttpRefusal(boolean fromLink, boolean oneTime);
    public static native int classifyInviteError(String msg);
    public static native boolean isTunnelAddress(String ip);

    // Recovery, Exit, ContinueHere, InputOnly, Roaming (`protocol/android/src/policy_recovery.rs`)
    public static final int INPUT_ONLY_SMALL_WIDTH = 640;
    public static final int INPUT_ONLY_SMALL_HEIGHT = 360;
    public static final int INPUT_ONLY_SMALL_FPS = 30;
    public static final int INPUT_ONLY_SMALL_BITRATE_KBPS = 500;
    public static final int INPUT_ONLY_MODE_NATIVE = 0;
    public static final int INPUT_ONLY_MODE_SMALL_STREAM = 1;
    public static final int INPUT_ONLY_MODE_OFF = 2;
    public static final int INPUT_ONLY_BLOCKER_NONE = 0;
    public static final int INPUT_ONLY_BLOCKER_PAIR = 1;
    public static final int INPUT_ONLY_BLOCKER_VIEW_PERMISSION = 2;
    public static final int INPUT_ONLY_BLOCKER_INPUT_PERMISSION = 3;
    public static final int RESUME_REFUSAL_NONE = 0;
    public static final int RESUME_REFUSAL_SESSION_CHANGED = 1;
    public static final int RESUME_REFUSAL_BROWSER_STREAM = 2;
    public static final int ROAMING_CAP_IGNORE = 0;
    public static final int ROAMING_CAP_ASK_NOT_METERED = 1;
    public static final int ROAMING_CAP_ASK_METERED = 2;
    public static final int ROAMING_CAP_RETURNED_ASK_NOT_METERED = 3;
    public static final int ROAMING_CAP_RETURNED_ASK_METERED = 4;
    public static final int ROAMING_ADDR_IGNORE = -1;
    public static final int ROAMING_ADDR_ASK_NOT_METERED = 0;
    public static final int ROAMING_ADDR_ASK_METERED = 1;

    public static native int recoveryMaxAttempts();
    public static native long recoveryWindowMs();
    public static native long recoveryTripResumeDelayMs();
    public static native int recoveryPeerTimeoutMs();
    public static native long[] recoveryNextDelay(int attempts, long startedAtMs, long nowMs, long connectedAtMs);
    public static native long[] recoveryResetAfterStableConnection(int attempts, long startedAtMs, long nowMs, long connectedAtMs);
    public static native long[] recoveryAfterUserTrip(int attempts, long startedAtMs, long leftAtMs, long nowMs);
    public static native long recoveryTripConnectedBefore(int attempts, long connectedForMs);
    public static native boolean recoveryRecoversTermination(int code, boolean videoStarted);
    public static native boolean recoveryTransientTermination(int code);
    public static native boolean recoveryTransportTermination(int code);
    public static native boolean recoveryTransientStartup(String stage, int portFlags, int error);
    public static native boolean recoveryReconnectsAtOnce(boolean cannotContinue, boolean metered, boolean mobileAllowed,
                                                          boolean autoReconnect, int networkGraceMs);
    public static native String recoveryGaveUpCause(String lastFailure, String unreachable);
    public static native String recoveryGaveUpText(String cause, String stopped);
    public static native int recoveryAutomaticBitrateStart(int encoderKbps, int resumeKbps);
    public static native int recoveryReconnectBitrateStart(boolean automatic, boolean automaticOwned, boolean manual, int kbps);
    public static native boolean recoveryVideoStarted(boolean decoderReceivedVideo, boolean chainVideoShown);
    public static native boolean recoveryIsAttempt(int attempts, boolean tripResume);
    public static native boolean recoveryIsResumeOnly(int attempts, boolean tripResume, boolean outputResumeOnly);
    public static native boolean recoveryHandOffInPlace(boolean taskRoot, boolean multiWindow, boolean pictureInPicture);
    public static native boolean recoveryCanAutoRecover(
            boolean autoReconnect, boolean displayedFailure, boolean connected, boolean foreground,
            boolean videoStarted, boolean finishingOrDestroyed, boolean handoffOrPending,
            boolean streamRole, boolean controlAction, boolean hasResumeTarget,
            int attempts, long startedAtMs, long nowMs, long connectedAtMs);
    public static native boolean recoveryCanRememberResume(
            boolean alreadyRemembered, boolean autoReconnect, boolean connected, boolean displayedFailure,
            boolean finishingOrDestroyed, boolean cancelledOnPause, boolean handoffInProgress,
            int recoveryAppId, boolean streamRole, boolean controlAction);

    public static native boolean continueHereAsksOnSelect(boolean offered, boolean plainStart, boolean sameGame);
    public static native boolean continueHereReadsMoveAfterEnd(boolean offered, boolean leftOnPurpose, boolean recoveryFollows);
    public static native boolean continueHereRecovers(boolean transientFailure, boolean moved);
    public static native boolean continueHereCameBack(boolean moved, boolean owns);
    public static native boolean continueHereJoinsAfterRead(boolean too, boolean sameGame, boolean pressFailed);
    public static native int continueHereResumeRefusal(boolean join, int code, String message);

    public static native boolean exitQuitsOnLeave(boolean quitOnLeaveChosen, int roleOrdinal, boolean connected, boolean secondScreen);
    public static native boolean exitTerminationNeedsAction(int errorCode, boolean portTestBlocked,
                                                              boolean networkGraceGaveUp, boolean recoveryExhausted);
    public static native boolean exitHostEndedStream(int errorCode);
    public static native boolean exitRefusedInHandshake(String stage);

    public static native int inputOnlyMode(boolean anyHostEnabled, boolean remoteSessionTiles, boolean hostListsInputOnly);
    public static native int inputOnlySmallStreamBlocker(boolean paired, long permission);
    public static native boolean inputOnlyKeepsRunningGame(boolean hostRunningGame, boolean thisGameRunning);
    public static native int inputOnlyEstimatedKbps();
    public static native int inputOnlyEstimatedMegabytesPerHour();

    public static native long roamingCreate(boolean hasStreamNetwork, long streamNetworkId);
    public static native void roamingDestroy(long handle);
    public static native int roamingOnCapabilities(long handle, long networkId, boolean notMetered);
    public static native boolean roamingRecordChange(long handle, long networkId, boolean metered, boolean returned, int change, long nowMs);
    public static native int roamingOnAddresses(long handle, long networkId, String addressesKey, boolean nat64, long nowMs);
    public static native boolean roamingRecordAddressChange(long handle, int change);
    public static native boolean roamingOnLost(long handle, long networkId);
    public static native void roamingStop(long handle);
}
