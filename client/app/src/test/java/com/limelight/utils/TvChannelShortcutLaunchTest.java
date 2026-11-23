package com.limelight.utils;

import android.app.Activity;
import android.app.Application;
import android.content.ContentProvider;
import android.content.ContentUris;
import android.content.ContentValues;
import android.content.Context;
import android.content.Intent;
import android.content.ServiceConnection;
import android.content.pm.PackageManager;
import android.content.pm.ProviderInfo;
import android.database.Cursor;
import android.database.MatrixCursor;
import android.graphics.Bitmap;
import android.media.tv.TvContract;
import android.net.Uri;
import android.os.ParcelFileDescriptor;

import com.limelight.AppView;
import com.limelight.Game;
import com.limelight.PosterContentProvider;
import com.limelight.ShortcutTrampoline;
import com.limelight.computers.ComputerManagerListener;
import com.limelight.computers.ComputerManagerService;
import com.limelight.grid.assets.DiskAssetLoader;
import com.limelight.nativeapi.FileLibraryCache;
import com.limelight.nativeapi.LibraryCache;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.http.PairingManager;
import com.limelight.pin.PinLockActivity;

import org.json.JSONArray;
import org.json.JSONObject;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.Shadows;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowContentResolver;
import org.robolectric.shadows.ShadowLooper;
import org.robolectric.util.ReflectionHelpers;

import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.OutputStream;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.StandardCopyOption;
import java.util.Collections;
import java.util.LinkedHashMap;
import java.util.Map;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotEquals;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

/**
 * nightjar#130: launching a game a subsequent time through its Android TV home screen channel card
 * must keep the card's cover art and metadata instead of overwriting it with the title-only poster.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 30, application = Application.class)
public class TvChannelShortcutLaunchTest {
    private static final String PC_UUID = "0123abcd-0000-0000-0000-000000000001";
    private static final String PC_NAME = "Living Room PC";
    private static final int APP_ID = 831329605;
    private static final String APP_NAME = "Hollow Knight";
    private static final String APP_UUID = "6f1c2a2e-4f4b-4f6e-9a65-0f3cbb0d4d11";
    private static final String COVER_VERSION_V1 = "C:\\covers\\hollow.png|4096|1700000001";
    private static final String COVER_VERSION_V2 = "C:\\covers\\hollow.png|8192|1700000099";

    private Application application;
    private FakeTvProvider tvProvider;
    private PosterContentProvider posterProvider;

    public static class TestTrampoline extends ShortcutTrampoline {
        ServiceConnection connection;
        Intent[] started;

        @Override
        public boolean bindService(Intent intent, ServiceConnection connection, int flags) {
            this.connection = connection;
            return true;
        }

        @Override
        public void unbindService(ServiceConnection connection) {
        }

        @Override
        public void startActivities(Intent[] intents) {
            this.started = intents;
        }
    }

    /** In-memory Android TV provider backing TvContract.Channels and TvContract.PreviewPrograms. */
    static class FakeTvProvider extends ContentProvider {
        final Map<Long, ContentValues> channels = new LinkedHashMap<>();
        final Map<Long, ContentValues> programs = new LinkedHashMap<>();
        long nextChannelId = 1;
        long nextProgramId = 100;
        int updateCount;
        int insertCount;

        @Override
        public boolean onCreate() {
            return true;
        }

        @Override
        public Cursor query(Uri uri, String[] projection, String selection, String[] selectionArgs, String sortOrder) {
            String path = uri.getPath();
            if ("/channel".equals(path)) {
                MatrixCursor cursor = new MatrixCursor(new String[] {
                        TvContract.Channels._ID, TvContract.Channels.COLUMN_INTERNAL_PROVIDER_ID
                });
                for (Map.Entry<Long, ContentValues> entry : channels.entrySet()) {
                    cursor.addRow(new Object[] {
                            entry.getKey(),
                            entry.getValue().getAsString(TvContract.Channels.COLUMN_INTERNAL_PROVIDER_ID)
                    });
                }
                return cursor;
            }
            if (path != null && path.startsWith("/preview_program")) {
                String channelParam = uri.getQueryParameter("channel");
                MatrixCursor cursor = new MatrixCursor(new String[] {
                        TvContract.PreviewPrograms._ID,
                        TvContract.PreviewPrograms.COLUMN_INTERNAL_PROVIDER_ID,
                        TvContract.PreviewPrograms.COLUMN_BROWSABLE,
                        TvContract.PreviewPrograms.COLUMN_POSTER_ART_URI
                });
                for (Map.Entry<Long, ContentValues> entry : programs.entrySet()) {
                    ContentValues values = entry.getValue();
                    if (channelParam != null && !channelParam.equals(
                            String.valueOf(values.getAsLong(TvContract.PreviewPrograms.COLUMN_CHANNEL_ID)))) {
                        continue;
                    }
                    cursor.addRow(new Object[] {
                            entry.getKey(),
                            values.getAsString(TvContract.PreviewPrograms.COLUMN_INTERNAL_PROVIDER_ID),
                            values.containsKey(TvContract.PreviewPrograms.COLUMN_BROWSABLE)
                                    ? values.getAsInteger(TvContract.PreviewPrograms.COLUMN_BROWSABLE) : 1,
                            values.getAsString(TvContract.PreviewPrograms.COLUMN_POSTER_ART_URI)
                    });
                }
                return cursor;
            }
            return null;
        }

        @Override
        public Uri insert(Uri uri, ContentValues values) {
            String path = uri.getPath();
            if ("/channel".equals(path)) {
                long id = nextChannelId++;
                channels.put(id, new ContentValues(values));
                return TvContract.buildChannelUri(id);
            }
            if ("/preview_program".equals(path)) {
                long id = nextProgramId++;
                ContentValues stored = new ContentValues(values);
                stored.put(TvContract.PreviewPrograms.COLUMN_BROWSABLE, 1);
                programs.put(id, stored);
                insertCount++;
                return TvContract.buildPreviewProgramUri(id);
            }
            return null;
        }

        @Override
        public int update(Uri uri, ContentValues values, String selection, String[] selectionArgs) {
            String path = uri.getPath();
            if (path != null && path.startsWith("/channel/")) {
                long id = ContentUris.parseId(uri);
                ContentValues existing = channels.get(id);
                if (existing == null) return 0;
                existing.putAll(values);
                return 1;
            }
            if (path != null && path.startsWith("/preview_program/")) {
                long id = ContentUris.parseId(uri);
                ContentValues existing = programs.get(id);
                if (existing == null) return 0;
                existing.putAll(values);
                updateCount++;
                return 1;
            }
            return 0;
        }

        @Override
        public int delete(Uri uri, String selection, String[] selectionArgs) {
            String path = uri.getPath();
            if (path != null && path.startsWith("/preview_program/")) {
                return programs.remove(ContentUris.parseId(uri)) != null ? 1 : 0;
            }
            if (path != null && path.startsWith("/channel/")) {
                return channels.remove(ContentUris.parseId(uri)) != null ? 1 : 0;
            }
            return 0;
        }

        @Override
        public String getType(Uri uri) {
            return null;
        }

        @Override
        public ParcelFileDescriptor openFile(Uri uri, String mode) {
            try {
                File tmp = File.createTempFile("channel-logo-", ".png", getContext().getCacheDir());
                return ParcelFileDescriptor.open(tmp, ParcelFileDescriptor.MODE_READ_WRITE);
            } catch (IOException e) {
                throw new RuntimeException(e);
            }
        }
    }

    @Before
    public void setUp() {
        application = RuntimeEnvironment.getApplication();
        Shadows.shadowOf(application.getPackageManager())
                .setSystemFeature(PackageManager.FEATURE_LEANBACK, true);

        ProviderInfo tvInfo = new ProviderInfo();
        tvInfo.authority = TvContract.AUTHORITY;
        tvProvider = new FakeTvProvider();
        tvProvider.attachInfo(application, tvInfo);
        ShadowContentResolver.registerProviderInternal(TvContract.AUTHORITY, tvProvider);

        posterProvider = Robolectric.buildContentProvider(PosterContentProvider.class).create().get();
    }

    private ComputerDetails onlineComputer() {
        ComputerDetails computer = new ComputerDetails();
        computer.uuid = PC_UUID;
        computer.name = PC_NAME;
        computer.state = ComputerDetails.State.ONLINE;
        computer.pairState = PairingManager.PairState.PAIRED;
        computer.activeAddress = new ComputerDetails.AddressTuple("192.168.1.10", 47989);
        computer.httpsPort = 47984;
        return computer;
    }

    private File writeRealCover(String cacheKey, int width, int height) throws IOException {
        File file = new DiskAssetLoader(application).getFile(PC_UUID, cacheKey);
        assertTrue(file.getParentFile().isDirectory() || file.getParentFile().mkdirs());
        Bitmap cover = Bitmap.createBitmap(width, height, Bitmap.Config.ARGB_8888);
        try (FileOutputStream out = new FileOutputStream(file)) {
            assertTrue(cover.compress(Bitmap.CompressFormat.PNG, 100, out));
        } finally {
            cover.recycle();
        }
        return file;
    }

    private File writePlaceholderCover(String cacheKey) throws IOException {
        File file = new DiskAssetLoader(application).getFile(PC_UUID, cacheKey);
        assertTrue(file.getParentFile().isDirectory() || file.getParentFile().mkdirs());
        Files.copy(new File("../protocol/client/tests/fixtures/box-placeholder.png").toPath(),
                file.toPath(), StandardCopyOption.REPLACE_EXISTING);
        return file;
    }

    private void storeNativeLibrary(int appId, String title, String coverPath, String coverVersion) throws Exception {
        JSONObject artwork = new JSONObject().put("version", coverVersion);
        if (coverPath != null) {
            artwork.put("cover", coverPath);
        }
        JSONObject game = new JSONObject()
                .put("install_id", "install-" + appId)
                .put("app_id", appId)
                .put("title", title)
                .put("running", false)
                .put("added", true)
                .put("artwork", artwork);
        JSONObject library = new JSONObject()
                .put("revision", "rev-1")
                .put("games", new JSONArray().put(game));
        File dir = new File(application.getFilesDir(), "console-library");
        new FileLibraryCache(dir).store(PC_UUID,
                new LibraryCache.Entry(library.toString(), 1_000L, Collections.emptyList()));
    }

    private void storeClassicAppList(int appId, String title, String uuid, String artVersion) throws Exception {
        try (OutputStream out = CacheHelper.openCacheFileForOutput(application.getCacheDir(), "applist", PC_UUID)) {
            String xml = "<root status_code=\"200\"><App><AppTitle>" + title + "</AppTitle><ID>" + appId
                    + "</ID><UUID>" + uuid + "</UUID><ArtVersion>" + artVersion
                    + "</ArtVersion><IsHdrSupported>1</IsHdrSupported></App></root>";
            out.write(xml.getBytes(StandardCharsets.UTF_8));
        }
    }

    /** Runs ShortcutTrampoline with {@code shortcutIntent} and returns the Game start Intent it launches. */
    private Intent launchThroughTrampoline(Intent shortcutIntent, ComputerDetails computer) throws Exception {
        ActivityController<TestTrampoline> controller =
                Robolectric.buildActivity(TestTrampoline.class, shortcutIntent).create().start().resume();
        TestTrampoline activity = controller.get();
        ComputerManagerService service = new ComputerManagerService();
        ComputerManagerService.ComputerManagerBinder binder = service.new ComputerManagerBinder() {
            @Override public boolean waitForReady() { return true; }
            @Override public ComputerDetails getComputer(String uuid) { return computer; }
            @Override public void invalidateStateForComputer(String uuid) { }
            @Override public void startPolling(ComputerManagerListener listener) {
                listener.notifyComputerUpdated(computer);
            }
            @Override public void stopPolling() { }
            @Override public String getUniqueId() { return "0123456789abcdef"; }
        };
        assertNotNull("trampoline should bind ComputerManagerService", activity.connection);
        activity.connection.onServiceConnected(null, binder);
        Thread waiter = ReflectionHelpers.getField(activity, "serviceWaitThread");
        waiter.join(2000);
        assertFalse(waiter.isAlive());
        ShadowLooper.idleMainLooper();
        assertNotNull("trampoline should start Game", activity.started);
        assertEquals(1, activity.started.length);
        Intent gameIntent = activity.started[0];
        controller.pause().stop().destroy();
        return gameIntent;
    }

    /** Simulates GameConnection.onCreate + connectionStarted reporting the launched game to ShortcutHelper. */
    private void reportGameConnected(Activity activity, Intent gameIntent) {
        ComputerDetails computer = new ComputerDetails();
        computer.name = gameIntent.getStringExtra(Game.EXTRA_PC_NAME);
        computer.uuid = gameIntent.getStringExtra(Game.EXTRA_PC_UUID);

        String appName = gameIntent.getStringExtra(Game.EXTRA_APP_NAME);
        int appId = gameIntent.getIntExtra(Game.EXTRA_APP_ID, 0);
        boolean hdr = gameIntent.getBooleanExtra(Game.EXTRA_APP_HDR, false);
        NvApp rebuiltApp = new NvApp(appName, appId, hdr);
        rebuiltApp.setAppUuid(gameIntent.getStringExtra(Game.EXTRA_APP_UUID));
        rebuiltApp.setMonitorResume(gameIntent.getBooleanExtra(Game.EXTRA_MONITOR_RESUME, false));

        new ShortcutHelper(activity).reportGameLaunched(
                computer, rebuiltApp, gameIntent.getStringExtra(Game.EXTRA_APP_ART_KEY));
    }

    private ContentValues singleProgram() {
        assertEquals("expected exactly one channel card", 1, tvProvider.programs.size());
        return tvProvider.programs.values().iterator().next();
    }

    private long openPosterSize(String uriString) throws Exception {
        try (ParcelFileDescriptor pfd = posterProvider.openBoxArtFile(Uri.parse(uriString), "r")) {
            assertNotNull(pfd);
            return pfd.getStatSize();
        }
    }

    @Test
    public void subsequentLaunchFromTvChannelCardPreservesCoverArtAndMetadata() throws Exception {
        Activity hostActivity = Robolectric.buildActivity(Activity.class).setup().get();
        ComputerDetails computer = onlineComputer();

        // Console Home builds a Native API NvApp with empty appUuid and non-empty coverVersion.
        NvApp nativeListed = new NvApp(APP_NAME, APP_ID, true);
        nativeListed.setArtVersion(COVER_VERSION_V1);
        String coverKey = nativeListed.getAssetCacheKey();
        assertNotEquals(Integer.toString(APP_ID), coverKey);
        File coverFile = writeRealCover(coverKey, 60, 90);

        // First launch from Nightjar Console Home -> Game -> reportGameLaunched creates the TV card.
        ComputerManagerService service = new ComputerManagerService();
        ComputerManagerService.ComputerManagerBinder binder = service.new ComputerManagerBinder() {
            @Override public String getUniqueId() { return "0123456789abcdef"; }
        };
        Intent firstGameIntent = ServerHelper.createStartIntent(
                hostActivity, nativeListed, computer, binder, LaunchSource.CONSOLE_PLAY);
        reportGameConnected(hostActivity, firstGameIntent);

        assertEquals(1, tvProvider.insertCount);
        ContentValues afterFirst = singleProgram();
        String expectedPosterUri = PosterContentProvider.createBoxArtUri(PC_UUID, coverKey).toString();
        assertEquals(expectedPosterUri, afterFirst.getAsString(TvContract.PreviewPrograms.COLUMN_POSTER_ART_URI));
        assertEquals(APP_NAME, afterFirst.getAsString(TvContract.PreviewPrograms.COLUMN_TITLE));
        assertEquals(String.valueOf(APP_ID), afterFirst.getAsString(TvContract.PreviewPrograms.COLUMN_INTERNAL_PROVIDER_ID));
        assertEquals(coverFile.length(), openPosterSize(expectedPosterUri));

        // Round-trip the TV card's stored INTENT_URI exactly as Android TV home screen does.
        String storedIntentUri = afterFirst.getAsString(TvContract.PreviewPrograms.COLUMN_INTENT_URI);
        assertNotNull(storedIntentUri);
        Intent cardLaunchIntent = Intent.parseUri(storedIntentUri, Intent.URI_INTENT_SCHEME);
        assertEquals(coverKey, cardLaunchIntent.getStringExtra(Game.EXTRA_APP_ART_KEY));

        // Subsequent launch through the TV channel card -> ShortcutTrampoline -> Game -> reportGameLaunched.
        Intent secondGameIntent = launchThroughTrampoline(cardLaunchIntent, computer);
        assertEquals("ConnectingScreen and reportGameLaunched must receive the versioned cover key",
                coverKey, secondGameIntent.getStringExtra(Game.EXTRA_APP_ART_KEY));
        reportGameConnected(hostActivity, secondGameIntent);

        assertEquals("existing channel card should be updated in place", 1, tvProvider.insertCount);
        assertEquals(1, tvProvider.updateCount);
        ContentValues afterSecond = singleProgram();
        assertEquals("cover art URI must not turn into an unversioned title-only poster",
                expectedPosterUri, afterSecond.getAsString(TvContract.PreviewPrograms.COLUMN_POSTER_ART_URI));
        assertEquals(APP_NAME, afterSecond.getAsString(TvContract.PreviewPrograms.COLUMN_TITLE));
        assertEquals(String.valueOf(APP_ID), afterSecond.getAsString(TvContract.PreviewPrograms.COLUMN_INTERNAL_PROVIDER_ID));
        assertEquals(coverFile.length(), openPosterSize(afterSecond.getAsString(TvContract.PreviewPrograms.COLUMN_POSTER_ART_URI)));
        assertFalse("no unversioned title poster should be created",
                PosterContentProvider.titlePosterFile(application, PC_UUID, Integer.toString(APP_ID)).exists());

        Intent updatedCardIntent = Intent.parseUri(
                afterSecond.getAsString(TvContract.PreviewPrograms.COLUMN_INTENT_URI), Intent.URI_INTENT_SCHEME);
        assertEquals(coverKey, updatedCardIntent.getStringExtra(Game.EXTRA_APP_ART_KEY));
    }

    @Test
    public void legacyPreFixChannelCardWithoutArtKeyRecoversCoverFromSavedLibraryAndHealsIntent() throws Exception {
        Activity hostActivity = Robolectric.buildActivity(Activity.class).setup().get();
        ComputerDetails computer = onlineComputer();

        NvApp nativeListed = new NvApp(APP_NAME, APP_ID, false);
        nativeListed.setArtVersion(COVER_VERSION_V1);
        String coverKey = nativeListed.getAssetCacheKey();
        File coverFile = writeRealCover(coverKey, 60, 90);
        storeNativeLibrary(APP_ID, APP_NAME, "/covers/hollow.png", COVER_VERSION_V1);

        // Pre-populate a channel card as 6.0.0–6.3.1 left it after a broken relaunch:
        // title-only unversioned poster URI and an intent with no EXTRA_APP_ART_KEY.
        new TvChannelHelper(hostActivity).createTvChannel(computer);
        long channelId = tvProvider.channels.keySet().iterator().next();
        Intent legacyIntent = new Intent(hostActivity, ShortcutTrampoline.class)
                .putExtra(AppView.NAME_EXTRA, PC_NAME)
                .putExtra(AppView.UUID_EXTRA, PC_UUID)
                .putExtra(Game.EXTRA_APP_NAME, APP_NAME)
                .putExtra(Game.EXTRA_APP_ID, String.valueOf(APP_ID))
                .putExtra(Game.EXTRA_APP_HDR, false)
                .putExtra(Game.EXTRA_APP_UUID, "")
                .setAction(Intent.ACTION_DEFAULT);
        ContentValues brokenCard = new ContentValues();
        brokenCard.put(TvContract.PreviewPrograms.COLUMN_CHANNEL_ID, channelId);
        brokenCard.put(TvContract.PreviewPrograms.COLUMN_INTERNAL_PROVIDER_ID, String.valueOf(APP_ID));
        brokenCard.put(TvContract.PreviewPrograms.COLUMN_TITLE, APP_NAME);
        brokenCard.put(TvContract.PreviewPrograms.COLUMN_POSTER_ART_URI,
                PosterContentProvider.createBoxArtUri(PC_UUID, String.valueOf(APP_ID)).toString());
        brokenCard.put(TvContract.PreviewPrograms.COLUMN_INTENT_URI, legacyIntent.toUri(Intent.URI_INTENT_SCHEME));
        tvProvider.insert(TvContract.PreviewPrograms.CONTENT_URI, brokenCard);

        // Relaunch via the legacy card: should recover coverKey from FileLibraryCache and heal the card.
        Intent parsedLegacy = Intent.parseUri(
                singleProgram().getAsString(TvContract.PreviewPrograms.COLUMN_INTENT_URI), Intent.URI_INTENT_SCHEME);
        assertNull("legacy card has no EXTRA_APP_ART_KEY", parsedLegacy.getStringExtra(Game.EXTRA_APP_ART_KEY));

        Intent gameIntent = launchThroughTrampoline(parsedLegacy, computer);
        assertEquals(coverKey, gameIntent.getStringExtra(Game.EXTRA_APP_ART_KEY));
        reportGameConnected(hostActivity, gameIntent);

        ContentValues healed = singleProgram();
        String expectedPosterUri = PosterContentProvider.createBoxArtUri(PC_UUID, coverKey).toString();
        assertEquals(expectedPosterUri, healed.getAsString(TvContract.PreviewPrograms.COLUMN_POSTER_ART_URI));
        assertEquals(coverFile.length(), openPosterSize(expectedPosterUri));
        Intent healedIntent = Intent.parseUri(
                healed.getAsString(TvContract.PreviewPrograms.COLUMN_INTENT_URI), Intent.URI_INTENT_SCHEME);
        assertEquals(coverKey, healedIntent.getStringExtra(Game.EXTRA_APP_ART_KEY));
    }

    @Test
    public void legacyCardHealsFromOnDiskCoverCacheEvenWithoutLibraryOrAppListFiles() throws Exception {
        Activity hostActivity = Robolectric.buildActivity(Activity.class).setup().get();
        ComputerDetails computer = onlineComputer();

        NvApp nativeListed = new NvApp(APP_NAME, APP_ID, false);
        nativeListed.setArtVersion(COVER_VERSION_V1);
        String coverKey = nativeListed.getAssetCacheKey();
        writeRealCover(coverKey, 60, 90);

        Intent legacyIntent = new Intent(hostActivity, ShortcutTrampoline.class)
                .putExtra(AppView.NAME_EXTRA, PC_NAME)
                .putExtra(AppView.UUID_EXTRA, PC_UUID)
                .putExtra(Game.EXTRA_APP_NAME, APP_NAME)
                .putExtra(Game.EXTRA_APP_ID, String.valueOf(APP_ID))
                .putExtra(Game.EXTRA_APP_UUID, "");

        Intent gameIntent = launchThroughTrampoline(legacyIntent, computer);
        assertEquals(coverKey, gameIntent.getStringExtra(Game.EXTRA_APP_ART_KEY));
    }

    @Test
    public void classicAppViewCardWithAppUuidPreservesCoverEvenWhenAppListCacheIsCleared() throws Exception {
        Activity hostActivity = Robolectric.buildActivity(Activity.class).setup().get();
        ComputerDetails computer = onlineComputer();

        NvApp classicListed = new NvApp(APP_NAME, APP_ID, true);
        classicListed.setAppUuid(APP_UUID);
        classicListed.setArtVersion(String.valueOf(APP_ID));
        String coverKey = classicListed.getAssetCacheKey();
        File coverFile = writeRealCover(coverKey, 60, 90);

        // First launch from AppView creates the card with appUuid and artKey.
        new TvChannelHelper(hostActivity).createTvChannel(computer);
        new TvChannelHelper(hostActivity).addGameToChannel(computer, classicListed, coverKey);

        // Even with no applist cache on disk, launching via the card preserves coverKey.
        String storedIntentUri = singleProgram().getAsString(TvContract.PreviewPrograms.COLUMN_INTENT_URI);
        Intent cardLaunchIntent = Intent.parseUri(storedIntentUri, Intent.URI_INTENT_SCHEME);
        Intent gameIntent = launchThroughTrampoline(cardLaunchIntent, computer);
        assertEquals(coverKey, gameIntent.getStringExtra(Game.EXTRA_APP_ART_KEY));

        // Simulate NvConnection resolving appId back from the host before connectionStarted.
        gameIntent.putExtra(Game.EXTRA_APP_ID, APP_ID);
        reportGameConnected(hostActivity, gameIntent);

        ContentValues updated = singleProgram();
        assertEquals(PosterContentProvider.createBoxArtUri(PC_UUID, coverKey).toString(),
                updated.getAsString(TvContract.PreviewPrograms.COLUMN_POSTER_ART_URI));
        assertEquals(coverFile.length(),
                openPosterSize(updated.getAsString(TvContract.PreviewPrograms.COLUMN_POSTER_ART_URI)));
    }

    @Test
    public void relaunchThroughExistingCardPicksUpUpdatedCoverFromSavedLibrary() throws Exception {
        Activity hostActivity = Robolectric.buildActivity(Activity.class).setup().get();
        ComputerDetails computer = onlineComputer();

        NvApp v1App = new NvApp(APP_NAME, APP_ID, false);
        v1App.setArtVersion(COVER_VERSION_V1);
        String v1Key = v1App.getAssetCacheKey();
        writeRealCover(v1Key, 60, 90);

        new TvChannelHelper(hostActivity).createTvChannel(computer);
        new TvChannelHelper(hostActivity).addGameToChannel(computer, v1App, v1Key);

        // Later, the host updates the cover and Console Home caches the new cover + library entry.
        NvApp v2App = new NvApp(APP_NAME, APP_ID, false);
        v2App.setArtVersion(COVER_VERSION_V2);
        String v2Key = v2App.getAssetCacheKey();
        assertNotEquals(v1Key, v2Key);
        File v2Cover = writeRealCover(v2Key, 80, 120);
        storeNativeLibrary(APP_ID, APP_NAME, "/covers/hollow-v2.png", COVER_VERSION_V2);

        // Launching the older card (whose intent still carries v1Key) upgrades to v2Key.
        Intent cardIntent = Intent.parseUri(
                singleProgram().getAsString(TvContract.PreviewPrograms.COLUMN_INTENT_URI), Intent.URI_INTENT_SCHEME);
        assertEquals(v1Key, cardIntent.getStringExtra(Game.EXTRA_APP_ART_KEY));

        Intent gameIntent = launchThroughTrampoline(cardIntent, computer);
        assertEquals(v2Key, gameIntent.getStringExtra(Game.EXTRA_APP_ART_KEY));
        reportGameConnected(hostActivity, gameIntent);

        ContentValues updated = singleProgram();
        assertEquals(PosterContentProvider.createBoxArtUri(PC_UUID, v2Key).toString(),
                updated.getAsString(TvContract.PreviewPrograms.COLUMN_POSTER_ART_URI));
        assertEquals(v2Cover.length(),
                openPosterSize(updated.getAsString(TvContract.PreviewPrograms.COLUMN_POSTER_ART_URI)));
    }

    @Test
    public void addGameToChannelNeverDowngradesAnExistingRealCoverToATitlePoster() throws Exception {
        Activity hostActivity = Robolectric.buildActivity(Activity.class).setup().get();
        ComputerDetails computer = onlineComputer();

        NvApp listed = new NvApp(APP_NAME, APP_ID, false);
        listed.setArtVersion(COVER_VERSION_V1);
        String coverKey = listed.getAssetCacheKey();
        writeRealCover(coverKey, 60, 90);

        TvChannelHelper helper = new TvChannelHelper(hostActivity);
        helper.createTvChannel(computer);
        helper.addGameToChannel(computer, listed, coverKey);

        // Direct update with an unversioned rebuilt NvApp and null launchArtKey still preserves the cover.
        NvApp unversioned = new NvApp(APP_NAME, APP_ID, false);
        helper.addGameToChannel(computer, unversioned, null);

        ContentValues card = singleProgram();
        assertEquals(PosterContentProvider.createBoxArtUri(PC_UUID, coverKey).toString(),
                card.getAsString(TvContract.PreviewPrograms.COLUMN_POSTER_ART_URI));
    }

    @Test
    public void gameWithoutCoverOrWithRemovedCoverStillUsesTitlePoster() throws Exception {
        Activity hostActivity = Robolectric.buildActivity(Activity.class).setup().get();
        ComputerDetails computer = onlineComputer();

        // Game previously had a cover on disk, then host removed it (coverPath == null in saved library).
        NvApp oldApp = new NvApp(APP_NAME, APP_ID, false);
        oldApp.setArtVersion(COVER_VERSION_V1);
        writeRealCover(oldApp.getAssetCacheKey(), 60, 90);
        storeNativeLibrary(APP_ID, APP_NAME, null, "");

        TvChannelHelper helper = new TvChannelHelper(hostActivity);
        helper.createTvChannel(computer);
        NvApp current = new NvApp(APP_NAME, APP_ID, false);
        helper.addGameToChannel(computer, current, current.getAssetCacheKey());

        String posterUri = singleProgram().getAsString(TvContract.PreviewPrograms.COLUMN_POSTER_ART_URI);
        assertEquals(PosterContentProvider.createBoxArtUri(PC_UUID, String.valueOf(APP_ID)).toString(), posterUri);
        File titlePoster = PosterContentProvider.titlePosterFile(application, PC_UUID, String.valueOf(APP_ID));
        assertTrue("title poster must be generated when host has no cover", titlePoster.isFile());
        assertEquals(titlePoster.length(), openPosterSize(posterUri));

        // Placeholder grey box.png cached under versioned key also stays on title poster.
        int placeholderAppId = 42;
        NvApp placeholderApp = new NvApp("Notepad", placeholderAppId, false);
        placeholderApp.setArtVersion("42");
        String placeholderKey = placeholderApp.getAssetCacheKey();
        writePlaceholderCover(placeholderKey);
        helper.addGameToChannel(computer, new NvApp("Notepad", placeholderAppId, false), placeholderKey);
        assertTrue(PosterContentProvider.titlePosterFile(application, PC_UUID, placeholderKey).isFile());
    }

    @Test
    public void pinLockForwardsValidArtKeyAndRejectsMalformedArtKey() {
        NvApp listed = new NvApp(APP_NAME, APP_ID, false);
        listed.setArtVersion(COVER_VERSION_V1);
        String validKey = listed.getAssetCacheKey();

        Intent validIncoming = new Intent()
                .putExtra(AppView.UUID_EXTRA, PC_UUID)
                .putExtra(AppView.NAME_EXTRA, PC_NAME)
                .putExtra(Game.EXTRA_APP_ID, String.valueOf(APP_ID))
                .putExtra(Game.EXTRA_APP_NAME, APP_NAME)
                .putExtra(Game.EXTRA_APP_ART_KEY, validKey);
        Intent validAgain = PinLockActivity.forTrampoline(application, validIncoming, PC_UUID, true)
                .getParcelableExtra(PinLockActivity.EXTRA_NEXT);
        assertEquals(validKey, validAgain.getStringExtra(Game.EXTRA_APP_ART_KEY));

        Intent maliciousIncoming = new Intent(validIncoming)
                .putExtra(Game.EXTRA_APP_ART_KEY, "../../etc/passwd");
        Intent sanitizedAgain = PinLockActivity.forTrampoline(application, maliciousIncoming, PC_UUID, true)
                .getParcelableExtra(PinLockActivity.EXTRA_NEXT);
        assertNull("malformed art keys must not cross the PIN screen",
                sanitizedAgain.getStringExtra(Game.EXTRA_APP_ART_KEY));
    }

    @Test
    public void diskAssetLoaderFindCachedCoverKeyMatchesExactAppIdAndSkipsPlaceholders() throws Exception {
        DiskAssetLoader loader = new DiskAssetLoader(application);

        // App 70's cover must never match lookup for app 7.
        NvApp app70 = new NvApp("Game 70", 70, false);
        app70.setArtVersion("v1");
        writeRealCover(app70.getAssetCacheKey(), 60, 90);
        assertNull(loader.findCachedCoverKey(PC_UUID, 7));

        // Placeholder for app 7 is skipped.
        NvApp app7Placeholder = new NvApp("Game 7", 7, false);
        app7Placeholder.setArtVersion("placeholder");
        writePlaceholderCover(app7Placeholder.getAssetCacheKey());
        assertNull(loader.findCachedCoverKey(PC_UUID, 7));

        // Real cover for app 7 is found, and newer cover wins over older cover.
        NvApp app7Old = new NvApp("Game 7", 7, false);
        app7Old.setArtVersion("old");
        File oldFile = writeRealCover(app7Old.getAssetCacheKey(), 60, 90);
        assertTrue(oldFile.setLastModified(1_000_000L));

        NvApp app7New = new NvApp("Game 7", 7, false);
        app7New.setArtVersion("new");
        File newFile = writeRealCover(app7New.getAssetCacheKey(), 60, 90);
        assertTrue(newFile.setLastModified(2_000_000L));

        assertEquals(app7New.getAssetCacheKey(), loader.findCachedCoverKey(PC_UUID, 7));

        // Classic applist fallback also works when both native and classic entries exist.
        storeClassicAppList(7, "Game 7", APP_UUID, "7");
        NvApp classicApp7 = new NvApp("Game 7", 7, false);
        classicApp7.setAppUuid(APP_UUID);
        classicApp7.setArtVersion("7");
        File classicFile = writeRealCover(classicApp7.getAssetCacheKey(), 60, 90);
        assertTrue(classicFile.setLastModified(3_000_000L));
        assertEquals(classicApp7.getAssetCacheKey(),
                TvChannelHelper.resolveShortcutArtKey(application, PC_UUID, new NvApp("Game 7", 7, false), 7, null));
    }
}
