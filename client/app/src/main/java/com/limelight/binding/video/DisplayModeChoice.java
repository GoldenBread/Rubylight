package com.limelight.binding.video;

/**
 * The display mode a stream switches the screen to (GameVideoOutput.prepareDisplayForRendering), kept
 * apart from Android's Display so the Settings screen names the same mode's refresh (SettingRules'
 * fractional V-Sync row) and tests run without a screen. Without VRR it takes the highest refresh that
 * is a whole multiple of the frame rate, even below the current one (60 Hz for 60 fps on a 90 Hz
 * screen), or the lowest such with Lower refresh rate; the frame pacing never changes it (120 Hz for
 * 60 fps on a 60/120 Hz screen in every pacing). The resolution stays unless the stream needs
 * otherwise. With VRR the top rate among the modes that keep the whole stream. The choice is Moonlight
 * Android's (Game.prepareDisplayForRendering, GPL-3.0, Cameron Gutman and contributors), moved here
 * unchanged from GameVideoOutput; Moonlight also asked it for the lowest multiple with its FPS-capped
 * and smoothest pacings.
 */
public final class DisplayModeChoice {
    private DisplayModeChoice() { }

    /** One of the screen's modes, in physical pixels. */
    public static final class Mode {
        public final int width;
        public final int height;
        public final float refreshHz;

        public Mode(int width, int height, float refreshHz) {
            this.width = width;
            this.height = height;
            this.refreshHz = refreshHz;
        }
    }

    /** A refresh at the frame rate or up to 3 Hz above it. */
    public static boolean isRefreshRateEqualMatch(float refreshRate, int fps) {
        return refreshRate >= fps && refreshRate <= fps + 3;
    }

    /** A refresh within 3 Hz above a whole multiple of the frame rate. */
    public static boolean isRefreshRateGoodMatch(float refreshRate, int fps) {
        return fps > 0 && refreshRate >= fps && Math.round(refreshRate) % fps <= 3;
    }

    /**
     * The index in {@code candidates} (the screen's supported modes, in Android's order) of the mode the
     * stream takes, or -1 to keep {@code current}. {@code mayReduceRefreshRate} is Lower refresh rate
     * alone (GameVideoOutput.mayReduceRefreshRate); the Settings screen passes false, where that setting
     * is off.
     */
    public static int choose(Mode current, Mode[] candidates, int streamWidth, int streamHeight, int fps,
                             boolean vrr, boolean nativeResolutionStream, boolean mayReduceRefreshRate) {
        Mode bestMode = current;
        int best = -1;
        boolean refreshRateIsGood = isRefreshRateGoodMatch(bestMode.refreshHz, fps);
        boolean refreshRateIsEqual = isRefreshRateEqualMatch(bestMode.refreshHz, fps);

        for (int i = 0; i < candidates.length; i++) {
            Mode candidate = candidates[i];
            boolean refreshRateReduced = candidate.refreshHz < bestMode.refreshHz;
            boolean resolutionReduced = candidate.width < bestMode.width ||
                    candidate.height < bestMode.height;
            boolean resolutionFitsStream = candidate.width >= streamWidth &&
                    candidate.height >= streamHeight;

            if (vrr) {
                // The top rate among modes that keep the whole stream: the rate the stream
                // was asked at (maxRefreshRate), never a smaller mode for a higher rate.
                if (candidate.refreshHz > bestMode.refreshHz &&
                        DisplayFrameRatePolicy.modeFitsStream(candidate.width, candidate.height,
                                current.width, current.height, streamWidth, streamHeight)) {
                    bestMode = candidate;
                    best = i;
                }
                continue;
            }

            if (candidate.width > 4096 && streamWidth <= 4096) {
                // Avoid resolutions options above 4K to be safe
                continue;
            }

            // On non-4K streams, we force the resolution to never change unless it's above
            // 60 FPS, which may require a resolution reduction due to HDMI bandwidth limitations,
            // or it's a native resolution stream.
            if (streamWidth < 3840 && fps <= 60 && !nativeResolutionStream) {
                if (current.width != candidate.width || current.height != candidate.height) {
                    continue;
                }
            }

            // Make sure the resolution doesn't regress unless if it's over 60 FPS
            // where we may need to reduce resolution to achieve the desired refresh rate.
            if (resolutionReduced && !(fps > 60 && resolutionFitsStream)) {
                continue;
            }

            if (mayReduceRefreshRate && refreshRateIsEqual && !isRefreshRateEqualMatch(candidate.refreshHz, fps)) {
                // If we had an equal refresh rate and this one is not, skip it. In min latency
                // mode, we want to always prefer the highest frame rate even though it may cause
                // microstuttering.
                continue;
            }
            else if (refreshRateIsGood) {
                // We've already got a good match, so if this one isn't also good, it's not
                // worth considering at all.
                if (!isRefreshRateGoodMatch(candidate.refreshHz, fps)) {
                    continue;
                }

                if (mayReduceRefreshRate) {
                    // User asked for the lowest possible refresh rate, so don't raise it if we
                    // have a good match already
                    if (candidate.refreshHz > bestMode.refreshHz) {
                        continue;
                    }
                }
                else {
                    // User asked for the highest possible refresh rate, so don't reduce it if we
                    // have a good match already
                    if (refreshRateReduced) {
                        continue;
                    }
                }
            }
            else if (!isRefreshRateGoodMatch(candidate.refreshHz, fps)) {
                // We didn't have a good match and this match isn't good either, so just don't
                // reduce the refresh rate.
                if (refreshRateReduced) {
                    continue;
                }
            } else {
                // We didn't have a good match and this match is good. Prefer this refresh rate
                // even if it reduces the refresh rate. Lowering the refresh rate can be beneficial
                // when streaming a 60 FPS stream on a 90 Hz device. We want to select 60 Hz to
                // match the frame rate even if the active display mode is 90 Hz.
            }

            bestMode = candidate;
            best = i;
            refreshRateIsGood = isRefreshRateGoodMatch(candidate.refreshHz, fps);
            refreshRateIsEqual = isRefreshRateEqualMatch(candidate.refreshHz, fps);
        }
        return best;
    }
}
