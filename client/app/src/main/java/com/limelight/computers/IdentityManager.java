/**
 * Client pairing unique ID facade backed by Rust {@code IdentityStore} ({@code identity.json})
 * via {@link NightjarHostsJni}, with one-time import of legacy {@code uniqueid}, {@code client.crt},
 * and {@code client.key} files.
 */
package com.limelight.computers;

import android.content.Context;

import com.limelight.LimeLog;
import com.limelight.nvstream.jni.Json;
import com.limelight.nvstream.jni.NightjarHostsJni;

import java.io.ByteArrayOutputStream;
import java.io.File;
import java.io.FileInputStream;
import java.io.IOException;
import java.nio.charset.StandardCharsets;

public class IdentityManager {
    private static final String UNIQUE_ID_FILE_NAME = "uniqueid";
    private static final int UID_SIZE_IN_BYTES = 8;

    private final String uniqueId;

    public IdentityManager(Context c) {
        File filesDir = c.getApplicationContext().getFilesDir();
        String legacyUid = readLegacyUniqueId(new File(filesDir, UNIQUE_ID_FILE_NAME));
        String legacyCert = readLegacyText(new File(filesDir, "client.crt"));
        byte[] legacyKey = readLegacyBytes(new File(filesDir, "client.key"));

        String raw = NightjarHostsJni.identityLoadOrCreate(
                filesDir.getAbsolutePath(), legacyUid, legacyCert, legacyKey);
        Json.Obj reply = Json.parseObject(raw);
        if (reply.has("ok")) {
            uniqueId = reply.getObject("ok").getString("unique_id");
        } else {
            LimeLog.severe("Failed to load/create identity: " + reply.optString("error", "unknown"));
            uniqueId = legacyUid != null ? legacyUid : "0000000000000000";
        }
        LimeLog.info("UID is now: " + uniqueId);
    }

    public String getUniqueId() {
        return uniqueId;
    }

    static String readLegacyUniqueId(File file) {
        byte[] bytes = readLegacyBytes(file);
        if (bytes == null || bytes.length < UID_SIZE_IN_BYTES * 2) {
            return null;
        }
        return new String(bytes, 0, UID_SIZE_IN_BYTES * 2, StandardCharsets.US_ASCII);
    }

    static String readLegacyText(File file) {
        byte[] bytes = readLegacyBytes(file);
        return bytes == null ? null : new String(bytes, StandardCharsets.US_ASCII);
    }

    static byte[] readLegacyBytes(File file) {
        if (file == null || !file.isFile()) {
            return null;
        }
        try (FileInputStream in = new FileInputStream(file);
             ByteArrayOutputStream out = new ByteArrayOutputStream()) {
            byte[] buf = new byte[4096];
            int n;
            while ((n = in.read(buf)) >= 0) {
                out.write(buf, 0, n);
            }
            return out.toByteArray();
        } catch (IOException e) {
            return null;
        }
    }
}
