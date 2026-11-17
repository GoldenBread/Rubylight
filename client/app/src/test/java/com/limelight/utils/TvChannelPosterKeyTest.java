package com.limelight.utils;

import com.limelight.nvstream.http.NvApp;

import org.junit.Test;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNotEquals;

/** nightjar#130: the TV channel's card names the cover the game list cached. */
public class TvChannelPosterKeyTest {
    @Test
    public void theCardUsesTheLaunchsCoverKeyNotTheStreamsRebuiltApp() {
        NvApp listed = new NvApp("Hollow Knight", 7, false);
        listed.setAppUuid("6f1c2a2e-4f4b-4f6e-9a65-0f3cbb0d4d11");
        // Rubylight's app list sends the app's ID as its art version.
        listed.setArtVersion("7");
        String launchKey = listed.getAssetCacheKey();

        // As GameConnection makes it from the launch's extras: no art version.
        NvApp streamed = new NvApp("Hollow Knight", 7, false);
        streamed.setAppUuid("6f1c2a2e-4f4b-4f6e-9a65-0f3cbb0d4d11");
        String appKey = streamed.getAssetCacheKey();
        assertNotEquals(launchKey, appKey);

        assertEquals(launchKey, TvChannelHelper.posterArtKey(launchKey, appKey, true, false));
        assertEquals("nothing cached yet: the list's key, which the cover will be cached under",
                launchKey, TvChannelHelper.posterArtKey(launchKey, appKey, false, false));
        assertEquals("only the app's own key is cached", appKey, TvChannelHelper.posterArtKey(launchKey, appKey, false, true));
        assertEquals(appKey, TvChannelHelper.posterArtKey(null, appKey, false, false));
        assertEquals(appKey, TvChannelHelper.posterArtKey("", appKey, false, false));
    }
}
