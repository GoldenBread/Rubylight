package com.limelight.console;

import com.limelight.nativeapi.ConsoleRepository;
import com.limelight.nativeapi.HostEndpoint;
import com.limelight.nativeapi.NativeApiFixtures;
import com.limelight.nativeapi.RecentsStore;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.jni.Json;
import com.limelight.nvstream.jni.NightjarConsoleJni;

import java.io.File;
import java.util.AbstractList;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;

/**
 * A repository over scripted hosts for the screen model tests, backed by Rust stores in
 * {@code cacheDirectory}.
 */
final class ConsoleRig {
    final NativeApiFixtures.FakeTransport transport = new NativeApiFixtures.FakeTransport();
    final Map<String, List<NvApp>> classicApps = new HashMap<>();
    final List<RecentsStore.Recent> recents;
    final ConsoleRepository repository;
    private final File baseDir;

    ConsoleRig(File cacheDirectory) {
        this.baseDir = cacheDirectory;
        this.recents = new RecentsList(cacheDirectory);
        this.repository = new ConsoleRepository(cacheDirectory, transport,
                computer -> HostEndpoint.of(computer, "unique", NativeApiFixtures.crypto()),
                computer -> classicApps.get(computer.uuid), () -> 5000L, null);
    }

    void close() {
        ConsoleRepository.closeForTests(baseDir);
    }

    private static final class RecentsList extends AbstractList<RecentsStore.Recent> {
        private final File baseDir;
        private final List<RecentsStore.Recent> items = new ArrayList<>();

        RecentsList(File baseDir) {
            this.baseDir = baseDir;
        }

        @Override public RecentsStore.Recent get(int index) {
            return items.get(index);
        }

        @Override public int size() {
            return items.size();
        }

        @Override public void add(int index, RecentsStore.Recent element) {
            items.add(index, element);
            sync();
        }

        @Override public void clear() {
            items.clear();
            sync();
        }

        private void sync() {
            File dir = new File(baseDir, "console");
            File file = new File(dir, "console_recents.json");
            try {
                if (items.isEmpty()) {
                    file.delete();
                    return;
                }
                dir.mkdirs();
                Json.Arr arr = Json.arr();
                for (RecentsStore.Recent r : items) {
                    arr.add(Json.obj()
                            .put("host", r.hostUuid)
                            .put("install_id", r.stableId)
                            .put("app_id", r.appId)
                            .put("title", r.title)
                            .put("at", r.playedAtMs));
                }
                java.nio.file.Files.write(
                        file.toPath(),
                        arr.toString().getBytes(java.nio.charset.StandardCharsets.UTF_8));
            } catch (java.io.IOException e) {
                throw new RuntimeException(e);
            }
        }
    }
}
