package com.limelight.update;

import com.limelight.nvstream.jni.NightjarPolicyJni;

/** When to look for an update, and whether to tell the user about what was found. */
public final class UpdatePolicy {
    public static final long CHECK_INTERVAL_MS = 12L * 60 * 60 * 1000;

    private UpdatePolicy() {
    }

    /** An automatic check runs when it is on and none ran in the last 12 hours. A clock set back counts as due. */
    public static boolean isCheckDue(boolean autoCheck, long lastCheckMs, long nowMs) {
        return NightjarPolicyJni.updatePolicyIsCheckDue(autoCheck, lastCheckMs, nowMs);
    }

    /** Whether an automatic check should show the release: a final release newer than this build, not skipped. */
    public static boolean shouldOffer(ReleaseInfo release, String currentVersion, String skippedVersion) {
        if (release == null || currentVersion == null) {
            return false;
        }
        return NightjarPolicyJni.updatePolicyShouldOffer(release.toJson(), currentVersion, skippedVersion);
    }

    /** Google Play updates the app it installed. */
    public static boolean isManagedByStore(String installerPackage) {
        return NightjarPolicyJni.updatePolicyIsManagedByStore(installerPackage);
    }

    /** Whether this build updates itself: not installed from a store and not built for one. */
    public static boolean isSelfUpdateAvailable(String installerPackage, boolean githubDistribution) {
        return NightjarPolicyJni.updatePolicyIsSelfUpdateAvailable(installerPackage, githubDistribution);
    }
}
