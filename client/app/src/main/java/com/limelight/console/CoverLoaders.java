package com.limelight.console;

import android.content.Context;
import android.graphics.Bitmap;
import android.graphics.BitmapFactory;
import android.os.Handler;
import android.os.Looper;
import android.util.DisplayMetrics;
import android.widget.ImageView;
import android.widget.TextView;

import com.limelight.grid.assets.CachedAppAssetLoader;
import com.limelight.grid.assets.DiskAssetLoader;
import com.limelight.grid.assets.MemoryAssetLoader;
import com.limelight.grid.assets.NetworkAssetLoader;
import com.limelight.nativeapi.ConsoleSnapshot;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;

import java.util.HashMap;
import java.util.Map;
import java.util.function.Consumer;

/**
 * Covers for Home and the game detail: the library grid's own box-art loader (memory, disk, then the
 * host's /appasset), one per host. A cover is cached under the app's id and the cover version the host
 * reported, so a new cover version loads a new image.
 */
final class CoverLoaders {
    /** The art is fetched about this wide; the loader scales it down to the view. */
    private static final int ART_WIDTH_PX = 300;

    private final Context context;
    private final String uniqueId;
    private final int coverWidthDp;
    private final Map<String, CachedAppAssetLoader> loaders = new HashMap<>();
    private final Map<String, String> addresses = new HashMap<>();
    private Consumer<NvApp> artListener;

    CoverLoaders(Context context, String uniqueId, int coverWidthDp) {
        this.context = context;
        this.uniqueId = uniqueId;
        this.coverWidthDp = coverWidthDp;
    }

    /** The art of the game; {@code source} is null for a game the host's library does not list. */
    static NvApp appOf(String title, int appId, ConsoleSnapshot.Source source) {
        NvApp app = new NvApp(title, appId, source != null && source.hdrSupported);
        if (source != null) {
            app.setAppUuid(source.appUuid);
            app.setStableId(source.stableId());
            app.setGameId(source.gameId);
            app.setArtVersion(source.coverVersion == null ? "" : source.coverVersion);
        }
        return app;
    }

    /**
     * The host to load {@code source}'s cover from, or null when a native host says the game has no cover of its own:
     * {@code /appasset} would send the host's grey placeholder, so the title stands in, as with no host.
     */
    static ComputerDetails coverHost(ComputerDetails host, ConsoleSnapshot.Source source) {
        // A classic host's source has no install id and no cover path, and its covers load as before.
        return source != null && source.installId != null && source.coverPath == null ? null : host;
    }

    /** Shows the cover in {@code image}, or the title in {@code text} until there is one. */
    void bind(ComputerDetails host, NvApp app, ImageView image, TextView text) {
        if (host == null) {
            text.setText(app.getAppName());
            text.setVisibility(android.view.View.VISIBLE);
            image.setImageDrawable(null);
            bindInfoCover(app, image, text);
            return;
        }
        loader(host).populateImageView(app, image, text);
    }

    /**
     * A game the host says has no cover (or no host at all): with a SteamGridDB key set, its cover from the lookup
     * replaces the title once the file is there. Off the main thread; nothing shows when there is none.
     */
    private void bindInfoCover(NvApp app, ImageView image, TextView text) {
        if (!GameInfo.coverWanted(context)) return;
        String tag = "game-info:" + app.getAssetCacheKey();
        image.setTag(tag);
        GameInfo info = GameInfo.get(context);
        java.util.function.Consumer<java.io.File> show = file -> {
            if (file == null) return;
            DisplayMetrics metrics = context.getResources().getDisplayMetrics();
            int wanted = Math.max(1, Math.round(coverWidthDp * metrics.density));
            BitmapFactory.Options bounds = new BitmapFactory.Options();
            bounds.inJustDecodeBounds = true;
            BitmapFactory.decodeFile(file.getAbsolutePath(), bounds);
            BitmapFactory.Options options = new BitmapFactory.Options();
            options.inSampleSize = Math.max(1, bounds.outWidth / (wanted * 2));
            Bitmap bitmap = BitmapFactory.decodeFile(file.getAbsolutePath(), options);
            if (bitmap == null) return;
            new Handler(Looper.getMainLooper()).post(() -> {
                if (!tag.equals(image.getTag())) return;
                if (image instanceof com.limelight.ui.PosterImageView) ((com.limelight.ui.PosterImageView) image).setArt(bitmap);
                else image.setImageBitmap(bitmap);
                image.setVisibility(android.view.View.VISIBLE);
                text.setVisibility(android.view.View.GONE);
                if (artListener != null) artListener.accept(app);
            });
        };
        String name = app.getAppName();
        String gameId = app.getGameId();
        // Kept on disk already: no request. Otherwise the lookup runs on the shared worker thread.
        Thread thread = new Thread(() -> {
            java.io.File kept = info.cachedCover(name, gameId);
            if (kept != null) show.accept(kept);
            else info.cover(name, gameId, show);
        }, "game-info-cover");
        thread.setDaemon(true);
        thread.start();
    }

    /** The cover if it is in memory now (never starts a load), else null. */
    Bitmap peek(ComputerDetails host, NvApp app) {
        return host == null ? null : loader(host).peekCover(app);
    }

    /** True once the game is known to have no artwork, so its backdrop can stay the canvas instead of waiting for one. */
    boolean lacksArt(ComputerDetails host, NvApp app) {
        return host == null || loader(host).hasNoArt(app);
    }

    /** Called on the main thread when a cover that was still loading has arrived (or turned out to be missing). */
    void setArtListener(Consumer<NvApp> listener) {
        artListener = listener;
        for (CachedAppAssetLoader loader : loaders.values()) loader.setArtListener(listener);
    }

    private CachedAppAssetLoader loader(ComputerDetails host) {
        String address = host.activeAddress == null ? "" : host.activeAddress.address + ":" + host.activeAddress.port;
        CachedAppAssetLoader loader = loaders.get(host.uuid);
        if (loader == null || !address.equals(addresses.get(host.uuid))) {
            if (loader != null) {
                loader.cancelForegroundLoads();
                loader.cancelBackgroundLoads();
            }
            int dpi = context.getResources().getDisplayMetrics().densityDpi;
            double divisor = Math.max(1.0, ART_WIDTH_PX / (coverWidthDp * (dpi / 160.0)));
            loader = new CachedAppAssetLoader(host, divisor,
                    new NetworkAssetLoader(context, uniqueId), new MemoryAssetLoader(), new DiskAssetLoader(context),
                    // Without artwork the card shows the title on the cover's own surface.
                    Bitmap.createBitmap(1, 1, Bitmap.Config.ARGB_8888));
            loader.setArtListener(artListener);
            loaders.put(host.uuid, loader);
            addresses.put(host.uuid, address);
        }
        return loader;
    }

    /**
     * Stops loading; call when the screen goes away. The covers in memory stay: every screen shares that cache, and
     * the one this screen returns to would have to decode all of its own again.
     */
    void release() {
        for (CachedAppAssetLoader loader : loaders.values()) {
            loader.cancelForegroundLoads();
            loader.cancelBackgroundLoads();
        }
        loaders.clear();
        addresses.clear();
    }
}
