package com.limelight.update;

import com.limelight.nvstream.jni.Json;
import com.limelight.nvstream.jni.NightjarHttpJni;

import java.io.IOException;

/** Asks GitHub for the latest published release via nightjar-core (webpki-roots + rustls). */
public final class UpdateChecker {
    private static final int TARGET_NIGHTJAR = 0;
    private static final int TARGET_RUBYLIGHT = 1;

    private UpdateChecker() {
    }

    /** Blocks on the network; call from a background thread. */
    public static ReleaseInfo fetchLatest() throws IOException {
        return fetchTarget(TARGET_NIGHTJAR);
    }

    static ReleaseInfo fetchLatestHost() throws IOException {
        return fetchTarget(TARGET_RUBYLIGHT);
    }

    private static ReleaseInfo fetchTarget(int target) throws IOException {
        String raw = NightjarHttpJni.updateFetchLatest(target);
        if (raw == null) {
            throw new IOException("Release check failed");
        }
        Json.Obj reply;
        try {
            reply = Json.parseObject(raw);
        } catch (Json.SyntaxException e) {
            throw new IOException("Unreadable release", e);
        }
        if (reply.has("ok")) {
            return ReleaseInfo.fromJsonObj(reply.getObject("ok"));
        }
        Json.Obj err = reply.optObject("error");
        if (err != null && "rate_limited".equals(err.optString("kind", ""))) {
            throw new UpdateException.RateLimited();
        }
        String message = err == null ? "Release check failed" : err.optString("message", "Release check failed");
        throw new IOException(message);
    }
}
