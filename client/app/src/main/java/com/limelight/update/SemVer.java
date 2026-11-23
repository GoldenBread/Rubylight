package com.limelight.update;

import com.limelight.nvstream.jni.Json;
import com.limelight.nvstream.jni.NightjarPolicyJni;

/** A release version such as v0.10.0 or 1.2.3-rc.1; build metadata after + is ignored. */
public final class SemVer implements Comparable<SemVer> {
    public final int major;
    public final int minor;
    public final int patch;
    /** The part after the first hyphen, or null for a final release. */
    public final String prerelease;

    private SemVer(int major, int minor, int patch, String prerelease) {
        this.major = major;
        this.minor = minor;
        this.patch = patch;
        this.prerelease = prerelease;
    }

    /** Returns null when the text is not a version. A leading v is ignored. */
    public static SemVer parse(String text) {
        if (text == null) {
            return null;
        }
        String json = NightjarPolicyJni.semVerParse(text);
        if (json == null) {
            return null;
        }
        Json.Obj obj = Json.parseObject(json);
        String pre = obj.isNull("prerelease") ? null : obj.optString("prerelease", null);
        return new SemVer(
                obj.getInt("major"),
                obj.getInt("minor"),
                obj.getInt("patch"),
                pre);
    }

    public boolean isPrerelease() {
        return prerelease != null;
    }

    /** Whether the release tag names a newer version than the running one. Unparseable versions are never newer. */
    public static boolean isNewer(String candidate, String current) {
        return candidate != null && current != null && NightjarPolicyJni.semVerIsNewer(candidate, current);
    }

    /** The version without a leading v, the way file names and the skipped version are written. */
    public static String normalize(String text) {
        return NightjarPolicyJni.semVerNormalize(text);
    }

    @Override
    public int compareTo(SemVer other) {
        return NightjarPolicyJni.semVerCompare(toString(), other.toString());
    }

    @Override
    public boolean equals(Object o) {
        return o instanceof SemVer && compareTo((SemVer) o) == 0;
    }

    @Override
    public int hashCode() {
        return 31 * (31 * (31 * major + minor) + patch) + (prerelease == null ? 0 : prerelease.hashCode());
    }

    @Override
    public String toString() {
        return major + "." + minor + "." + patch + (prerelease == null ? "" : "-" + prerelease);
    }
}
