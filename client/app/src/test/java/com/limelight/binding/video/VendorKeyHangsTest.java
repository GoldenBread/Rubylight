package com.limelight.binding.video;

import android.app.Application;
import android.content.Context;
import android.content.SharedPreferences;
import android.os.Build;
import android.preference.PreferenceManager;

import com.limelight.BuildConfig;

import org.junit.After;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

import java.util.Arrays;
import java.util.List;

import static org.junit.Assert.*;

/** The record of decoders that took no input with their vendor keys: per decoder and format, per app and system build. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34, application = Application.class)
public class VendorKeyHangsTest {
    private static final List<String> KEYS = Arrays.asList("vendor.mtk.vdec.low-latency.mode=1", "operating-rate=32767");
    private final Context context = RuntimeEnvironment.getApplication();

    @After
    public void clear() {
        records().edit().clear().commit();
    }

    private SharedPreferences records() {
        return context.getSharedPreferences(VendorKeyHangs.PREFS, Context.MODE_PRIVATE);
    }

    @Test
    public void aRecordHoldsForItsDecoderAndFormatOnly() {
        VendorKeyHangs.record(context, VendorKeyHangs.key("c2.mtk.hevc.decoder", "video/hevc"), KEYS);
        assertEquals(KEYS, VendorKeyHangs.recorded(context, VendorKeyHangs.key("c2.mtk.hevc.decoder", "video/hevc")));
        assertNull(VendorKeyHangs.recorded(context, VendorKeyHangs.key("c2.mtk.hevc.decoder", "video/av01")));
        assertNull(VendorKeyHangs.recorded(context, VendorKeyHangs.key("c2.mtk.av1.decoder", "video/hevc")));
        // A file of its own: settings, presets and profiles never carry it.
        assertTrue(PreferenceManager.getDefaultSharedPreferences(context).getAll().isEmpty());
    }

    @Test
    public void anAppOrSystemUpdateDropsEveryRecord() {
        VendorKeyHangs.record(context, VendorKeyHangs.key("c2.mtk.hevc.decoder", "video/hevc"), KEYS);
        VendorKeyHangs.record(context, VendorKeyHangs.key("c2.mtk.av1.decoder", "video/av01"), KEYS);
        records().edit().putString(VendorKeyHangs.STAMP, "1|an older build").commit();

        assertNull(VendorKeyHangs.recorded(context, VendorKeyHangs.key("c2.mtk.hevc.decoder", "video/hevc")));
        assertNull(VendorKeyHangs.recorded(context, VendorKeyHangs.key("c2.mtk.av1.decoder", "video/av01")));
        assertEquals(1, records().getAll().size());
        assertEquals(VendorKeyHangs.stamp(), records().getString(VendorKeyHangs.STAMP, null));
    }

    @Test
    public void theRecordsHoldForThisAppVersionOnThisSystemBuild() {
        assertEquals(BuildConfig.VERSION_CODE + "|" + Build.FINGERPRINT, VendorKeyHangs.stamp());
    }
}
