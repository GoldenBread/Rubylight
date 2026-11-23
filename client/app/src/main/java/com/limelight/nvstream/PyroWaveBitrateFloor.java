/**
 * Thin JNI delegate for {@code nightjar_policy::pyrowave} bitrate floor and link-notice helpers.
 */
package com.limelight.nvstream;

import com.limelight.nvstream.jni.NightjarPolicyJni;

public final class PyroWaveBitrateFloor {
    public static final int UNKNOWN_LINK_DEFAULT_CAP_KBPS = 900_000;

    private PyroWaveBitrateFloor() {}

    public static int minimumKbps(int width, int height, int fps) {
        return NightjarPolicyJni.pyroFloorMinimumKbps(width, height, fps);
    }

    public static int defaultKbps(int width, int height, int fps, int linkMbps) {
        return NightjarPolicyJni.pyroFloorDefaultKbps(width, height, fps, linkMbps);
    }

    public static int startKbps(int configured, boolean automatic, int width, int height, int fps, int linkMbps) {
        return NightjarPolicyJni.pyroFloorStartKbps(configured, automatic, width, height, fps, linkMbps);
    }

    public static LinkNotice linkNotice(int width, int height, int fps, int linkMbps, int configuredKbps) {
        int[] n = NightjarPolicyJni.pyroFloorLinkNotice(width, height, fps, linkMbps, configuredKbps);
        return n == null ? null : new LinkNotice(n[0], n[1], n[2]);
    }

    public static final class LinkNotice {
        public final int askedKbps;
        public final int minimumKbps;
        public final int capKbps;

        LinkNotice(int askedKbps, int minimumKbps, int capKbps) {
            this.askedKbps = askedKbps;
            this.minimumKbps = minimumKbps;
            this.capKbps = capKbps;
        }
    }
}
