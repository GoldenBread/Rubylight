package com.limelight.nativeapi;

import com.limelight.nvstream.jni.Json;

/**
 * GET /neural?app=: a game's Neural Rendering switches, offered while the host lists
 * {@code neural.frame_generation}. The host downloads and installs on its own after a write, so a
 * client reads again while {@link #working()}. A device never names a program on the PC.
 */
public final class NativeNeural {
    public static final String FRAME_GENERATION = "frame_generation";

    /** One switch; null when the host's answer has no frame generation switch. */
    public static final class Switch {
        /** {@code configurable}, {@code read_only} (no Host control), {@code unsupported}, {@code temporarily_unavailable} or {@code needs_external_setup}. */
        public final String state;
        public final boolean on;
        /** Why it can't change now, in the host's words; null when it can. */
        public final String reason;
        /**
         * Why it can't change now as a code the app says in its own words ({@code busy}, {@code streaming},
         * {@code no_host_control}, {@code gpu}, {@code runtime}, {@code anti_cheat}, {@code no_frame_generation},
         * {@code choose_program}, {@code game}); null from an older host, or when it can change.
         */
        public final String why;

        Switch(String state, boolean on, String reason, String why) {
            this.state = state;
            this.on = on;
            this.reason = reason;
            this.why = why;
        }

        public boolean configurable() {
            return "configurable".equals(state);
        }
    }

    public final String app;
    /** The one-time notice was accepted on the host; until then a write turning it on carries accept_notice. */
    public final boolean accepted;
    public final Switch frameGeneration;
    /** Rubylight 3.0's upscaling is in the game's folder: turning the switch on replaces it. */
    public final boolean replacesEarlierUpscaling;
    /** The job's phase ({@code download}, {@code place}, {@code remove}, {@code done}, {@code failed}), or null. */
    public final String phase;
    public final long done;
    public final long total;
    /** Why the last job failed, in the host's words; null otherwise. */
    public final String error;
    /** Once it is on: {@code full} (the PC's own package) or {@code addon} (added to the user's upscaler mod); null otherwise. */
    public final String mode;
    /** Once it is on: the release in the game's folder; null otherwise or from an older host. */
    public final String release;
    /**
     * When the GPU it runs on is not the one the PC streams with: that it runs only when the game renders on
     * that GPU, in the host's words; null otherwise or from an older host.
     */
    public final String gpuNote;

    NativeNeural(String app, boolean accepted, Switch frameGeneration, boolean replacesEarlierUpscaling,
                 String phase, long done, long total, String error, String mode, String release, String gpuNote) {
        this.app = app;
        this.accepted = accepted;
        this.frameGeneration = frameGeneration;
        this.replacesEarlierUpscaling = replacesEarlierUpscaling;
        this.phase = phase;
        this.done = done;
        this.total = total;
        this.error = error;
        this.mode = mode;
        this.release = release;
        this.gpuNote = gpuNote;
    }

    /** The host is downloading, installing or removing: read again in a moment. */
    public boolean working() {
        return "download".equals(phase) || "place".equals(phase) || "remove".equals(phase);
    }

    public boolean failed() {
        return "failed".equals(phase);
    }

    static NativeNeural parse(Json.Obj json) {
        Switch frameGeneration = null;
        Json.Arr switches = json.optArray("switches");
        for (int i = 0; switches != null && i < switches.length(); i++) {
            Json.Obj s = switches.getObject(i);
            if (FRAME_GENERATION.equals(s.optString("id"))) {
                frameGeneration = new Switch(s.optString("state"), s.optBoolean("on"), text(s, "reason"),
                        text(s, "why"));
            }
        }
        Json.Obj job = json.optObject("job");
        return new NativeNeural(json.getString("app"), json.optBoolean("accepted"), frameGeneration,
                json.optBoolean("replaces_earlier_upscaling"),
                job == null ? null : job.optString("phase", null),
                job == null ? 0 : job.optLong("done"), job == null ? 0 : job.optLong("total"),
                job == null ? null : text(job, "error"), text(json, "mode"), text(json, "release"),
                text(json, "gpu_note"));
    }

    /** POST /neural/settings for the frame generation switch; accept_notice only when the user just accepted it. */
    static String writeJson(String app, boolean on, boolean acceptNotice) {
        Json.Obj write = Json.obj().put("app", app).put("switch", FRAME_GENERATION).put("on", on);
        if (acceptNotice) write.put("accept_notice", true);
        return write.toString();
    }

    private static String text(Json.Obj json, String key) {
        return json.isNull(key) ? null : json.optString(key, null);
    }
}
