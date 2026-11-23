package com.limelight.nativeapi;

import com.limelight.nvstream.jni.Json;

import java.util.ArrayList;
import java.util.Collections;
import java.util.List;

/**
 * GET /audio: night mode and headphone surround of this device's stream. A change through
 * {@link NativeApiClient#writeAudio} applies from the stream's next audio packet and isn't saved.
 */
public final class NativeAudio {
    /** The reason of a setting turned on mid-stream that applies from the next stream. */
    public static final String NEXT_STREAM = "next_stream";

    /** One switch as the host reports it. */
    public static final class Setting {
        public final String id;
        /** Only a {@code configurable} setting can change now; without a stream it is {@code temporarily_unavailable}. */
        public final boolean configurable;
        /** The stream's choice; null without one. */
        public final Boolean value;
        /** Why this stream doesn't apply it now, and the host's words for it; both null when it does. */
        public final String reason;
        public final String message;

        Setting(String id, boolean configurable, Boolean value, String reason, String message) {
            this.id = id;
            this.configurable = configurable;
            this.value = value;
            this.reason = reason;
            this.message = message;
        }

        /** Shown as a switch: the host lets this device change it and says its state. */
        public boolean switchable() {
            return configurable && value != null;
        }
    }

    public final long revision;
    public final List<Setting> settings;

    NativeAudio(long revision, List<Setting> settings) {
        this.revision = revision;
        this.settings = Collections.unmodifiableList(settings);
    }

    /** The setting with {@code id}, or null when the host's answer has none. */
    public Setting get(String id) {
        for (Setting setting : settings) {
            if (setting.id.equals(id)) return setting;
        }
        return null;
    }

    static NativeAudio parse(Json.Obj json) {
        List<Setting> settings = new ArrayList<>();
        Json.Arr array = json.getArray("settings");
        for (int i = 0; i < array.length(); i++) {
            Json.Obj setting = array.getObject(i);
            Object value = setting.raw("value");
            settings.add(new Setting(setting.getString("id"), "configurable".equals(setting.optString("state")),
                    value instanceof Boolean ? (Boolean) value : null,
                    setting.isNull("reason") ? null : setting.optString("reason", null),
                    setting.isNull("message") ? null : setting.optString("message", null)));
        }
        return new NativeAudio(json.optLong("revision"), settings);
    }

    /** POST /audio/settings turning one switch on or off. */
    static String writeJson(String id, boolean on) {
        Json.Obj change = Json.obj().put("id", id).put("value", on);
        return Json.obj().put("changes", Json.arr().add(change)).toString();
    }
}
