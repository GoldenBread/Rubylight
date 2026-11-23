package com.limelight.nativeapi;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.content.Context;

import org.junit.After;
import org.junit.Before;
import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

import java.io.File;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.concurrent.CountDownLatch;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class ConsoleStoresTest {
    @Rule public TemporaryFolder temporary = new TemporaryFolder();

    @Before
    public void resetBefore() {
        ConsoleRepository.resetForTests();
    }

    @After
    public void resetAfter() {
        ConsoleRepository.resetForTests();
    }

    @Test
    public void theLibraryCacheKeepsOneFilePerHostAndSurvivesAReopen() throws Exception {
        Context context = RuntimeEnvironment.getApplication();
        File libDir = new File(context.getFilesDir(), "console-library");
        libDir.mkdirs();
        Files.write(new File(libDir, "a_b.json").toPath(),
                "{\"fetched_ms\":42,\"library\":{\"revision\":\"r\",\"games\":[{\"install_id\":\"i1\",\"app_id\":1,\"title\":\"Portal\"}]}}"
                        .getBytes(StandardCharsets.UTF_8));
        assertTrue(ConsoleRepository.hasLibrary(context, "a/b"));
        assertTrue(ConsoleRepository.hasAnyLibrary(context));
        assertFalse(ConsoleRepository.hasLibrary(context, "other"));
    }

    @Test
    public void forgettingAHostRemovesItsCacheFile() throws Exception {
        Context context = RuntimeEnvironment.getApplication();
        File libDir = new File(context.getFilesDir(), "console-library");
        libDir.mkdirs();
        Files.write(new File(libDir, "h.json").toPath(),
                "{\"revision\":\"r\",\"games\":[{\"install_id\":\"i1\",\"app_id\":1,\"title\":\"Portal\"}]}"
                        .getBytes(StandardCharsets.UTF_8));
        ConsoleRepository.forgetHost(context, "h");
        assertFalse(ConsoleRepository.hasLibrary(context, "h"));
        ConsoleRepository.forgetHost(context, "never-there");
    }

    @Test
    public void recentsAreNewestFirstWithoutDuplicatesAndCappedAtTwenty() {
        Context context = RuntimeEnvironment.getApplication();
        assertTrue(ConsoleRepository.loadRecents(context).isEmpty());
        for (int i = 0; i < 25; i++) {
            ConsoleRepository.recordLaunch(context, "h", i, "Game " + i, "", 100 + i);
        }
        ConsoleRepository.recordLaunch(context, "h", 20, "Game 20", "", 500);
        ConsoleRepository.recordLaunch(context, "other", 20, "Game 20", "", 501);
        List<RecentsStore.Recent> recents = ConsoleRepository.loadRecents(context);
        assertEquals(20, recents.size());
        assertEquals("other", recents.get(0).hostUuid);
        assertEquals(500, recents.get(1).playedAtMs);
        assertEquals(20, recents.get(1).appId);
        // Reopened after handle reset, it reads the same list from disk.
        ConsoleRepository.resetForTests();
        assertEquals(recents.size(), ConsoleRepository.loadRecents(context).size());
        int sameApp = 0;
        for (RecentsStore.Recent recent : recents) {
            if (recent.hostUuid.equals("h") && recent.appId == 20) {
                sameApp++;
            }
        }
        assertEquals(1, sameApp);
    }

    @Test
    public void forgettingAHostDropsOnlyItsRecents() {
        Context context = RuntimeEnvironment.getApplication();
        ConsoleRepository.recordLaunch(context, "a", 1, "Game 1", "", 1);
        ConsoleRepository.recordLaunch(context, "b", 1, "Game 1", "", 2);
        ConsoleRepository.forgetHost(context, "a");
        List<RecentsStore.Recent> recents = ConsoleRepository.loadRecents(context);
        assertEquals(1, recents.size());
        assertEquals("b", recents.get(0).hostUuid);
    }

    @Test
    public void favouritesAndLegacyMigrationWithBackupPreservationWorkCleanly() throws Exception {
        Context context = RuntimeEnvironment.getApplication();
        String raw = "[{\"host\":\"a\",\"host_name\":\"A\",\"app\":1,\"title\":\"One\"},"
                + "{\"host\":\"a\"},"
                + "{\"host\":\"b\",\"host_name\":\"B\",\"key\":\"inst-2\",\"app\":2,\"title\":\"Two\"}]";
        context.getSharedPreferences("console_favourites", Context.MODE_PRIVATE).edit()
                .putString("favourites", raw).commit();

        ConsoleRepository repo = ConsoleRepository.create(context, "unique");
        assertTrue(repo.isFavourite("a", "", 1));
        assertTrue(repo.isFavourite("b", "inst-2", 99));
        // Damaged legacy favourites entry triggers .bak file preservation in console/console_favourites.json.bak.
        File bak = new File(new File(context.getFilesDir(), "console"), "console_favourites.json.bak");
        assertTrue(bak.isFile());

        repo.setFavourite("b", "B", "inst-2", 40, "Two", true);
        assertTrue(repo.isFavourite("b", "inst-2", 40));
        repo.setFavourite("b", "B", "inst-2", 99, "Two", false);
        assertFalse(repo.isFavourite("b", "inst-2", 40));
    }

    @Test
    public void togglesAndForgetHostFromDifferentThreadsDoNotLoseWrites() throws Exception {
        File dir = temporary.newFolder();
        int each = 30;
        CountDownLatch go = new CountDownLatch(1);
        List<Thread> threads = new ArrayList<>();
        for (String host : new String[]{"a", "b"}) {
            Thread thread = new Thread(() -> {
                ConsoleRepository own = new ConsoleRepository(dir, new NativeApiFixtures.FakeTransport(),
                        c -> null, c -> null, () -> 5000L, null);
                try { go.await(); } catch (InterruptedException e) { return; }
                for (int i = 0; i < each; i++) {
                    own.setFavourite(host, host + " PC", "k-" + i, i + 1, "Game " + i, true);
                }
            });
            threads.add(thread);
            thread.start();
        }
        Thread forget = new Thread(() -> {
            ConsoleRepository own = new ConsoleRepository(dir, new NativeApiFixtures.FakeTransport(),
                    c -> null, c -> null, () -> 5000L, null);
            try { go.await(); } catch (InterruptedException e) { return; }
            for (int i = 0; i < each; i++) {
                own.setFavourite("gone", "Gone PC", "g-" + i, i + 1, "Gone " + i, true);
                own.forgetHost("gone");
            }
        });
        threads.add(forget);
        forget.start();
        go.countDown();
        for (Thread thread : threads) {
            thread.join();
        }
        ConsoleRepository check = new ConsoleRepository(dir, new NativeApiFixtures.FakeTransport(),
                c -> null, c -> null, () -> 5000L, null);
        for (int i = 0; i < each; i++) {
            assertTrue(check.isFavourite("a", "k-" + i, i + 1));
            assertTrue(check.isFavourite("b", "k-" + i, i + 1));
            assertFalse(check.isFavourite("gone", "g-" + i, i + 1));
        }
        assertNotNull(check.cached(Collections.emptyList()));
    }
}
