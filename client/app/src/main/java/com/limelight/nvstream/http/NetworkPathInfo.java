/**
 * Host route category ({@code GET /api/network-path}), parsed in Rust via {@link NightjarHttpJni}.
 */
package com.limelight.nvstream.http;

import com.limelight.nvstream.jni.Json;
import com.limelight.nvstream.jni.NightjarHttpJni;

import java.io.IOException;

public final class NetworkPathInfo {
    public enum Kind { DIRECT, RELAY, PEER_RELAY, UNKNOWN }

    public final Kind kind;

    NetworkPathInfo(Kind kind) {
        this.kind = kind;
    }

    public static NetworkPathInfo parse(String response) throws IOException {
        if (response == null) {
            throw new IOException("Invalid route response");
        }
        try {
            Json.Obj reply = Json.parseObject(NightjarHttpJni.parseNetworkPath(response));
            if (reply.has("error")) {
                throw new IOException(reply.getObject("error").optString("message", "Invalid route response"));
            }
            return new NetworkPathInfo(Kind.valueOf(reply.getString("ok")));
        } catch (Json.SyntaxException | IllegalArgumentException e) {
            throw new IOException("Invalid route response", e);
        }
    }
}
