package com.limelight.nativeapi;

import static com.limelight.nativeapi.NativeApiFixtures.ok;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;
import static org.junit.Assert.fail;

import android.app.Application;

import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.jni.Json;
import com.limelight.nvstream.jni.NightjarConsoleJni;

import org.junit.After;
import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

import java.util.Arrays;
import java.util.Collections;

/** The library and capability fields past the first API version: each optional, unknown ones ignored. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class NativeLibraryFieldsTest {
    private static final String BARE = "{\"install_id\":\"i1\",\"app_id\":5,\"title\":\"Portal\"}";
    private static final String FULL = "{\"install_id\":\"i2\",\"app_id\":6,\"title\":\"Hades\",\"game_id\":\"epic:h\","
            + "\"store\":{\"source\":\"epic\",\"id\":\"h\",\"name\":\"Epic Games\",\"region\":\"eu\"},"
            + "\"installed\":false,\"running\":false,\"added\":true,\"last_played\":1761300000,"
            + "\"playtime_minutes\":125,\"launchable\":false,\"future\":{\"x\":1},"
            + "\"artwork\":{\"cover\":\"/c/i2\",\"version\":\"v3\",\"hero\":\"/h/i2\",\"logo\":\"/l/i2\","
            + "\"landscape\":\"/w/i2\",\"icon\":\"/i/i2\"}}";
    private static final String UNADDED = "{\"install_id\":null,\"title\":\"Celeste\",\"game_id\":\"gog:c\","
            + "\"store\":{\"source\":\"gog\",\"id\":\"c\",\"name\":\"GOG\"},\"installed\":true,\"added\":false}";

    @Rule public TemporaryFolder temporary = new TemporaryFolder();
    private final NativeApiFixtures.FakeTransport transport = new NativeApiFixtures.FakeTransport();

    @After
    public void tearDown() {
        ConsoleRepository.resetForTests();
    }

    private NativeApiClient client() {
        ComputerDetails host = NativeApiFixtures.nativeHost("pc", "PC", true);
        return new NativeApiClient(transport, HostEndpoint.of(host, "unique", NativeApiFixtures.crypto()));
    }

    private ConsoleSnapshot.Source parseViaRepo(String gameJson) throws Exception {
        java.io.File dir = temporary.newFolder();
        transport.reply("pc", "library", ok("{\"revision\":\"r\",\"games\":[" + gameJson + "]}"));
        transport.reply("pc", "session", ok(NativeApiFixtures.IDLE));
        ComputerDetails host = NativeApiFixtures.nativeHost("pc", "PC", true);
        ConsoleRepository repo = new ConsoleRepository(dir, transport,
                c -> HostEndpoint.of(c, "unique", NativeApiFixtures.crypto()), c -> null, () -> 5000L, null);
        return repo.refresh(Collections.singletonList(host)).host("pc").sources.get(0);
    }

    private static ConsoleSnapshot.Source sourceNamed(ConsoleSnapshot.Host host, String title) {
        for (ConsoleSnapshot.Source s : host.sources) {
            if (title.equals(s.title)) return s;
        }
        throw new AssertionError("Missing title: " + title);
    }

    @Test
    public void collectionsAreReadAndStoredAndAbsentMeansNone() throws Exception {
        String named = "{\"revision\":\"r\",\"games\":[{\"install_id\":\"i1\",\"app_id\":5,\"title\":\"Portal\","
                + "\"collections\":[\"VR\",\"  \",null,\"Couch\"]},{\"install_id\":\"i2\",\"app_id\":6,\"title\":\"Hades\","
                + "\"collections\":\"not a list\"}," + BARE + "]}";
        java.io.File dir = temporary.newFolder();
        transport.reply("pc", "library", ok(named));
        transport.reply("pc", "session", ok(NativeApiFixtures.IDLE));
        ComputerDetails host = NativeApiFixtures.nativeHost("pc", "PC", true);
        ConsoleRepository repo = new ConsoleRepository(dir, transport,
                c -> HostEndpoint.of(c, "unique", NativeApiFixtures.crypto()), c -> null, () -> 5000L, null);
        repo.refresh(Collections.singletonList(host));
        ConsoleRepository.evictCacheForTests(dir);
        ConsoleSnapshot cached = repo.cached(Collections.singletonList(host));
        ConsoleSnapshot.Host h = cached.host("pc");
        assertEquals(Arrays.asList("VR", "Couch"), sourceNamed(h, "Portal").collections);
        assertTrue(sourceNamed(h, "Hades").collections.isEmpty());
    }

    @Test
    public void aGameWithoutTheNewFieldsReadsAsAddedAndUntold() throws Exception {
        ConsoleSnapshot.Source game = parseViaRepo(BARE);
        assertEquals("i1", game.installId);
        assertEquals(5, game.appId);
        assertTrue(game.added);
        assertNull(game.lastPlayed);
        assertNull(game.playtimeMinutes);
        assertNull(game.launchable);
        assertNull(game.storeSource);
        assertNull(game.storeId);
        assertNull(game.storeName);
        assertNull(game.installed);
        assertNull(game.coverPath);
        assertNull(game.heroPath);
        assertNull(game.logoPath);
        assertNull(game.landscapePath);
        assertEquals("", game.coverVersion);
    }

    @Test
    public void everyNewFieldIsReadAndUnknownOnesAreIgnored() throws Exception {
        ConsoleSnapshot.Source game = parseViaRepo(FULL);
        assertEquals("epic", game.storeSource);
        assertEquals("h", game.storeId);
        assertEquals("Epic Games", game.storeName);
        assertEquals(Boolean.FALSE, game.installed);
        assertTrue(game.added);
        assertEquals(Long.valueOf(1761300000L), game.lastPlayed);
        assertEquals(Integer.valueOf(125), game.playtimeMinutes);
        assertEquals(Boolean.FALSE, game.launchable);
        assertEquals("/c/i2", game.coverPath);
        assertEquals("v3", game.coverVersion);
        assertEquals("/h/i2", game.heroPath);
        assertEquals("/l/i2", game.logoPath);
        assertEquals("/w/i2", game.landscapePath);
    }

    @Test
    public void aGameTheHostHasNotAddedNeedsNoIds() throws Exception {
        ConsoleSnapshot.Source game = parseViaRepo(UNADDED);
        assertFalse(game.added);
        assertEquals("", game.installId);
        assertEquals(0, game.appId);
        assertEquals("gog:c", game.gameId);
        assertEquals("gog", game.storeSource);
    }

    @Test
    public void theCacheRoundTripKeepsEveryField() throws Exception {
        java.io.File dir = temporary.newFolder();
        transport.reply("pc", "library", ok("{\"revision\":\"r1\",\"games\":[" + BARE + "," + FULL + "," + UNADDED + "]}"));
        transport.reply("pc", "session", ok(NativeApiFixtures.IDLE));
        ComputerDetails host = NativeApiFixtures.nativeHost("pc", "PC", true);
        ConsoleRepository repo = new ConsoleRepository(dir, transport,
                c -> HostEndpoint.of(c, "unique", NativeApiFixtures.crypto()), c -> null, () -> 5000L, null);
        repo.refresh(Collections.singletonList(host));
        ConsoleRepository.evictCacheForTests(dir);
        ConsoleSnapshot.Host again = repo.cached(Collections.singletonList(host)).host("pc");
        ConsoleSnapshot.Source full = sourceNamed(again, "Hades");
        assertEquals("Epic Games", full.storeName);
        assertEquals(Long.valueOf(1761300000L), full.lastPlayed);
        assertEquals(Integer.valueOf(125), full.playtimeMinutes);
        assertEquals(Boolean.FALSE, full.launchable);
        assertEquals("/w/i2", full.landscapePath);
        ConsoleSnapshot.Source unadded = sourceNamed(again, "Celeste");
        assertFalse(unadded.added);
        assertEquals("", unadded.installId);
        ConsoleSnapshot.Source bare = sourceNamed(again, "Portal");
        assertTrue(bare.added);
        assertNull(bare.storeSource);
    }

    @Test
    public void theUnaddedGamesAreAskedForOnlyWhenWanted() throws Exception {
        transport.reply("pc", "library", ok("{\"revision\":\"r\",\"games\":[" + UNADDED + "]}"));
        client().library();
        client().library(true);
        assertEquals("", transport.arguments.get(0));
        assertEquals("include=unadded", transport.arguments.get(1));
    }

    @Test
    public void addingAGameSendsItsIdAndReadsTheEntryTheHostAdded() throws Exception {
        transport.reply("pc", "library_add", ok("{\"install_id\":\"i9\",\"app_id\":9,\"title\":\"Celeste\","
                + "\"game_id\":\"gog:c\",\"added\":true}"));
        ConsoleSnapshot.AddedGame game = client().addToLibrary("gog:c");
        assertNotNull(game);
        assertEquals("pc|library_add", transport.calls.get(0));
        assertEquals("gog:c", Json.parseObject(transport.arguments.get(0)).getString("game_id"));
        assertEquals("i9", game.installId);
        assertEquals(9, game.appId);
        assertEquals("Celeste", game.title);
    }

    private static ConsoleSnapshot.AddedGame parseAdded(String rawJson, String gameId) {
        Json.Obj env = Json.parseObject(NightjarConsoleJni.parseAdded(rawJson, gameId));
        Json.Obj ok = env.optObject("ok");
        return ok == null ? null : ConsoleSnapshot.AddedGame.fromJson(ok);
    }

    @Test
    public void anAddedEntryNeedsOnlyItsIdsAndMayComeWrappedOrAsTheLibrary() {
        ConsoleSnapshot.AddedGame bare = parseAdded("{\"install_id\":\"i9\",\"app_id\":9}", "gog:c");
        assertNotNull(bare);
        assertEquals(9, bare.appId);
        assertEquals("no title: the card's is shown", "", bare.title);
        ConsoleSnapshot.AddedGame wrapped = parseAdded("{\"game\":" + BARE + "}", "x");
        assertNotNull(wrapped);
        assertEquals(5, wrapped.appId);
        ConsoleSnapshot.AddedGame listed = parseAdded("{\"revision\":\"r\",\"games\":["
                + BARE + ",{\"install_id\":\"i9\",\"app_id\":9,\"title\":\"Celeste\",\"game_id\":\"gog:c\"}]}", "gog:c");
        assertNotNull(listed);
        assertEquals(9, listed.appId);
        assertNull("a library that does not list it as added",
                parseAdded("{\"revision\":\"r\",\"games\":[" + UNADDED + "]}", "gog:c"));
        assertNull(parseAdded(UNADDED, "gog:c"));
    }

    @Test
    public void anAddReplyWithoutItsIdsIsMalformed() {
        transport.reply("pc", "library_add", ok("{\"title\":\"Celeste\",\"added\":true}"));
        try {
            client().addToLibrary("gog:c");
            fail("an added entry without ids");
        } catch (NativeApiException e) {
            assertEquals(NativeApiException.Kind.MALFORMED, e.getKind());
        }
    }

    @Test
    public void capabilitiesWithoutFeaturesHaveNone() {
        NativeCapabilities caps = NativeCapabilities.parse(Json.parseObject("{\"api_version\":1,\"permissions\":{}}"));
        assertTrue(caps.features.isEmpty());
        assertFalse(caps.has(NativeCapabilities.LIBRARY_UNADDED));
        assertFalse(caps.has(NativeCapabilities.LIBRARY_ADD));
    }

    @Test
    public void theLibraryFeaturesAreReadAndUnreadableEntriesSkipped() {
        NativeCapabilities caps = NativeCapabilities.parse(Json.parseObject("{\"api_version\":1,\"permissions\":{},"
                + "\"features\":[\"library\",7,null,{\"x\":1},\"library.unadded\",\"library.add\",\"later\"]}"));
        assertTrue(caps.has(NativeCapabilities.LIBRARY_UNADDED));
        assertTrue(caps.has(NativeCapabilities.LIBRARY_ADD));
        assertTrue(caps.has("later"));
        assertEquals(4, caps.features.size());
    }

    @Test
    public void usbForwardingNeedsTheFeatureAndThePermission() {
        assertTrue(NativeCapabilities.parse(Json.parseObject("{\"api_version\":1,\"permissions\":{\"usb\":true},"
                + "\"features\":[\"usb\"]}")).offersUsbForwarding());
        assertFalse("without the permission", NativeCapabilities.parse(Json.parseObject("{\"api_version\":1,"
                + "\"permissions\":{\"usb\":false},\"features\":[\"usb\"]}")).offersUsbForwarding());
        assertFalse("a string is not a grant", NativeCapabilities.parse(Json.parseObject("{\"api_version\":1,"
                + "\"permissions\":{\"usb\":\"true\"},\"features\":[\"usb\"]}")).offersUsbForwarding());
        assertFalse("without the feature", NativeCapabilities.parse(Json.parseObject("{\"api_version\":1,"
                + "\"permissions\":{\"usb\":true}}")).offersUsbForwarding());
    }

    @Test
    public void nullsAndNonsenseReadAsUntold() throws Exception {
        ConsoleSnapshot.Source game = parseViaRepo("{\"install_id\":\"i1\",\"app_id\":5,\"title\":\"Portal\","
                + "\"added\":null,\"last_played\":null,\"playtime_minutes\":null,\"launchable\":null,\"store\":null,"
                + "\"artwork\":{\"cover\":null,\"hero\":\"\"}}");
        assertTrue(game.added);
        assertNull(game.lastPlayed);
        assertNull(game.playtimeMinutes);
        assertNull(game.launchable);
        assertNull(game.storeSource);
        assertNull(game.storeId);
        assertNull(game.storeName);
        assertNull(game.coverPath);
        assertNull(game.heroPath);

        assertNull(parseViaRepo("{\"install_id\":\"i2\",\"app_id\":6,\"title\":\"Soon\",\"last_played\":\"soon\"}").lastPlayed);
        assertEquals(Long.valueOf(0L), parseViaRepo("{\"install_id\":\"i3\",\"app_id\":7,\"title\":\"Zero\",\"last_played\":0}").lastPlayed);
        assertNull(parseViaRepo("{\"install_id\":\"i4\",\"app_id\":8,\"title\":\"Neg\",\"last_played\":-1}").lastPlayed);
    }
}
