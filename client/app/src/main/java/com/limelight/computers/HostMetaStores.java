/**
 * Shared {@code HostMetaStore} handle cache and one-time {@link SharedPreferences} importer for
 * {@link HostNotices}, {@link HostPackedModes}, {@link HostCapabilityCache}, {@link HostNames},
 * and {@link PinnedAddresses}.
 */
package com.limelight.computers;

import android.content.Context;
import android.content.SharedPreferences;

import androidx.annotation.VisibleForTesting;

import com.limelight.nvstream.jni.Json;
import com.limelight.nvstream.jni.NightjarHostsJni;

import java.io.File;
import java.util.ArrayList;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;

final class HostMetaStores {
    private static final ConcurrentHashMap<String, Long> HANDLES = new ConcurrentHashMap<>();

    private HostMetaStores() {
    }

    static long open(Context context) {
        File dir = context.getApplicationContext().getFilesDir();
        String key = dir.getAbsolutePath();
        synchronized (HANDLES) {
            return HANDLES.computeIfAbsent(key, NightjarHostsJni::metaStoreOpen);
        }
    }

    static long openSource(Context context, String source) {
        Context app = context.getApplicationContext();
        long handle = open(app);
        if (NightjarHostsJni.metaNeedsImport(handle, source)) {
            SharedPreferences prefs = app.getSharedPreferences(source, Context.MODE_PRIVATE);
            Map<String, ?> all = prefs.getAll();
            Json.Obj entries = Json.obj();
            for (Map.Entry<String, ?> entry : all.entrySet()) {
                Object val = entry.getValue();
                if (val instanceof String || val instanceof Number || val instanceof Boolean) {
                    entries.put(entry.getKey(), val);
                }
            }
            NightjarHostsJni.metaImportLegacy(handle, source, entries.toString());
            if (!all.isEmpty()) {
                prefs.edit().clear().commit();
            }
        }
        return handle;
    }

    @VisibleForTesting
    static void closeForTests(Context context) {
        if (context == null) return;
        File dir = context.getApplicationContext().getFilesDir();
        if (dir == null) return;
        synchronized (HANDLES) {
            Long h = HANDLES.remove(dir.getAbsolutePath());
            if (h != null && h != 0L) {
                NightjarHostsJni.metaStoreDestroy(h);
            }
        }
    }

    @VisibleForTesting
    static void resetForTests() {
        synchronized (HANDLES) {
            for (Long h : new ArrayList<>(HANDLES.values())) {
                if (h != null && h != 0L) {
                    NightjarHostsJni.metaStoreDestroy(h);
                }
            }
            HANDLES.clear();
        }
    }
}
