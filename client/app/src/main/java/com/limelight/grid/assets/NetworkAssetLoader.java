package com.limelight.grid.assets;

import android.content.Context;

import com.limelight.LimeLog;
import com.limelight.binding.PlatformBinding;
import com.limelight.nvstream.http.NvHTTP;
import com.limelight.utils.ServerHelper;

import java.io.IOException;
import java.io.InputStream;

public class NetworkAssetLoader {
    private final Context context;
    private final String uniqueId;

    public NetworkAssetLoader(Context context, String uniqueId) {
        this.context = context;
        this.uniqueId = uniqueId;
    }

    /** True for the host's 404 on /appasset. */
    public static boolean isNotFound(IOException e) {
        // NvHTTP throws FileNotFoundException for a 404; the other codes arrive as HostHttpResponseException.
        return e instanceof java.io.FileNotFoundException
                || (e instanceof com.limelight.nvstream.http.HostHttpResponseException
                && ((com.limelight.nvstream.http.HostHttpResponseException) e).getErrorCode() == 404);
    }

    public InputStream getBitmapStream(CachedAppAssetLoader.LoaderTuple tuple) {
        InputStream in = null;
        tuple.hostHasNoCover = false;
        try {
            NvHTTP http = new NvHTTP(ServerHelper.getCurrentAddressFromComputer(tuple.computer),
                    tuple.computer.httpsPort, uniqueId, tuple.computer.serverCert,
                    PlatformBinding.getCryptoProvider(context));
            in = http.getBoxArt(tuple.app);
        } catch (IOException e) {
            // Only the host's own "no such cover" counts; a timeout or a closed connection says nothing about the game.
            tuple.hostHasNoCover = isNotFound(e);
        }

        if (in != null) {
            LimeLog.info("Network asset load complete: " + tuple);
        }
        else {
            LimeLog.info("Network asset load failed: " + tuple);
        }

        return in;
    }
}
