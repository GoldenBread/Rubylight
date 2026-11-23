package com.limelight;

import android.content.ContentProvider;
import android.content.ContentResolver;
import android.content.ContentValues;
import android.content.Context;
import android.content.UriMatcher;
import android.database.Cursor;
import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.graphics.Paint;
import android.graphics.Typeface;
import android.net.Uri;
import android.os.ParcelFileDescriptor;
import android.text.Layout;
import android.text.StaticLayout;
import android.text.TextPaint;
import android.text.TextUtils;


import com.limelight.grid.assets.DiskAssetLoader;
import com.limelight.grid.assets.PlaceholderArt;

import java.io.File;
import java.io.FileNotFoundException;
import java.io.FileOutputStream;
import java.io.IOException;
import java.util.List;

public class PosterContentProvider extends ContentProvider {


    public static final String AUTHORITY = "poster." + BuildConfig.APPLICATION_ID;
    public static final String PNG_MIME_TYPE = "image/png";
    public static final int APP_ID_PATH_INDEX = 2;
    public static final int COMPUTER_UUID_PATH_INDEX = 1;
    private DiskAssetLoader mDiskAssetLoader;

    private static final UriMatcher sUriMatcher;
    private static final String BOXART_PATH = "boxart";
    private static final int BOXART_URI_ID = 1;
    private static final String TITLE_POSTER_DIR = "tv-posters";
    private static final int POSTER_WIDTH = 300;
    private static final int POSTER_HEIGHT = 450;

    static {
        sUriMatcher = new UriMatcher(UriMatcher.NO_MATCH);
        sUriMatcher.addURI(AUTHORITY, BOXART_PATH, BOXART_URI_ID);
    }

    @Override
    public ParcelFileDescriptor openFile(Uri uri, String mode) throws FileNotFoundException {
        int match = sUriMatcher.match(uri);
        if (match == BOXART_URI_ID) {
            return openBoxArtFile(uri, mode);
        }
        return openBoxArtFile(uri, mode);

    }

    public ParcelFileDescriptor openBoxArtFile(Uri uri, String mode) throws FileNotFoundException {
        if (!"r".equals(mode)) {
            throw new UnsupportedOperationException("This provider is only for read mode");
        }

        List<String> segments = uri.getPathSegments();
        if (segments.size() != 3) {
            throw new FileNotFoundException();
        }
        String appId = segments.get(APP_ID_PATH_INDEX);
        String uuid = segments.get(COMPUTER_UUID_PATH_INDEX);
        if (!DiskAssetLoader.isValidCacheKey(appId) || !uuid.matches("[0-9a-fA-F-]+")) {
            throw new FileNotFoundException();
        }
        File file = mDiskAssetLoader.getFile(uuid, appId);
        if (PlaceholderArt.isCover(file)) {
            return ParcelFileDescriptor.open(file, ParcelFileDescriptor.MODE_READ_ONLY);
        }
        // No cover of the game's own (none cached, or the host's grey stand-in): the poster with the
        // game's title the channel wrote, since the TV home screen draws a failed poster as an
        // empty grey card. Only the app writes posters; this only reads them.
        File poster = titlePosterFile(getContext(), uuid, appId);
        if (poster.isFile()) {
            return ParcelFileDescriptor.open(poster, ParcelFileDescriptor.MODE_READ_ONLY);
        }
        throw new FileNotFoundException();
    }

    /** Where the title poster of the game with cover key {@code appId} on PC {@code uuid} is kept. */
    public static File titlePosterFile(Context context, String uuid, String appId) {
        return new File(new File(context.getCacheDir(), TITLE_POSTER_DIR), uuid + "-" + appId + ".png");
    }

    /**
     * Writes the poster with the game's title the provider serves while the game has no cover of its
     * own (TvChannelHelper, for a channel's card).
     */
    public static void writeTitlePoster(Context context, String uuid, String appId, String title) throws IOException {
        writeTitlePoster(titlePosterFile(context, uuid, appId), title.trim(),
                context.getColor(R.color.night_primary_container), context.getColor(R.color.night_on_primary_container));
    }

    /** The title on the app's colours at the size the cached covers have (2:3, as the channel's cards). */
    static void writeTitlePoster(File target, String title, int background, int text) throws IOException {
        File dir = target.getParentFile();
        if (dir != null && !dir.isDirectory() && !dir.mkdirs()) {
            throw new IOException("cannot create " + dir);
        }
        Bitmap bitmap = Bitmap.createBitmap(POSTER_WIDTH, POSTER_HEIGHT, Bitmap.Config.ARGB_8888);
        Canvas canvas = new Canvas(bitmap);
        canvas.drawColor(background);
        TextPaint paint = new TextPaint(Paint.ANTI_ALIAS_FLAG);
        paint.setColor(text);
        paint.setTextSize(POSTER_WIDTH / 9f);
        paint.setTypeface(Typeface.DEFAULT_BOLD);
        int margin = POSTER_WIDTH / 10;
        StaticLayout layout = StaticLayout.Builder.obtain(title, 0, title.length(), paint, POSTER_WIDTH - 2 * margin)
                .setAlignment(Layout.Alignment.ALIGN_CENTER)
                .setMaxLines(6)
                .setEllipsize(TextUtils.TruncateAt.END)
                .build();
        canvas.save();
        canvas.translate(margin, Math.max(margin, (POSTER_HEIGHT - layout.getHeight()) / 2f));
        layout.draw(canvas);
        canvas.restore();
        File temporary = File.createTempFile("poster-", ".tmp", dir);
        try {
            try (FileOutputStream out = new FileOutputStream(temporary)) {
                if (!bitmap.compress(Bitmap.CompressFormat.PNG, 100, out)) {
                    throw new IOException("PNG encoding failed");
                }
            }
            if (!temporary.renameTo(target)) {
                throw new IOException("cannot publish " + target);
            }
        } finally {
            bitmap.recycle();
            //noinspection ResultOfMethodCallIgnored
            temporary.delete();
        }
    }

    @Override
    public int delete(Uri uri, String selection, String[] selectionArgs) {
        throw new UnsupportedOperationException("This provider is only for read mode");
    }

    @Override
    public String getType(Uri uri) {
        return PNG_MIME_TYPE;
    }

    @Override
    public Uri insert(Uri uri, ContentValues values) {
        throw new UnsupportedOperationException("This provider is only for read mode");
    }

    @Override
    public boolean onCreate() {
        mDiskAssetLoader = new DiskAssetLoader(getContext());
        return true;
    }

    @Override
    public Cursor query(Uri uri, String[] projection, String selection,
                        String[] selectionArgs, String sortOrder) {
        throw new UnsupportedOperationException("This provider doesn't support query");
    }

    @Override
    public int update(Uri uri, ContentValues values, String selection,
                      String[] selectionArgs) {
        throw new UnsupportedOperationException("This provider is support read only");
    }


    public static Uri createBoxArtUri(String uuid, String appId) {
        return new Uri.Builder()
                .scheme(ContentResolver.SCHEME_CONTENT)
                .authority(AUTHORITY)
                .appendPath(BOXART_PATH)
                .appendPath(uuid)
                .appendPath(appId)
                .build();
    }

}
