package com.limelight.binding.video;

import android.app.Activity;
import android.app.Application;
import android.graphics.SurfaceTexture;
import android.media.MediaFormat;
import android.view.Surface;

import com.limelight.preferences.PreferenceConfiguration;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;

import static org.junit.Assert.*;

/**
 * The GLES upscaler is built for the stream a picture carries and the window surface it draws to.
 * Both can move under it: a host can answer a reconfigure with a size of its own, and a fold, a
 * turn or a split screen resizes the surface without telling the renderer.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class UpscalingRebuildTest {
    private static MediaCodecDecoderRenderer renderer(Activity activity) {
        ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.upscalingMode = UpscalingPolicy.Mode.SGSR1;
        prefs.videoFormat = PreferenceConfiguration.FormatOption.AUTO;
        return new MediaCodecDecoderRenderer(activity, prefs, null, 0, false, false, "", null);
    }

    @Test
    public void aPictureOfAnotherSizeMovesTheStreamSizeToWhatArrived() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            MediaCodecDecoderRenderer renderer = renderer(activity.get());
            SurfaceTexture texture = new SurfaceTexture(0);
            Surface surface = new Surface(texture);
            UpscalingPolicy policy = new UpscalingPolicy(60);
            GlesUpscaler upscaler = new GlesUpscaler(activity.get(), surface, UpscalingPolicy.Mode.SGSR1,
                    1280, 720, false, 0, policy);
            try {
                ReflectionHelpers.setField(renderer, "outputFormat",
                        MediaFormat.createVideoFormat("video/avc", 1920, 1080));
                ReflectionHelpers.setField(renderer, "initialWidth", 1280);
                ReflectionHelpers.setField(renderer, "initialHeight", 720);
                ReflectionHelpers.setField(renderer, "expectedWidth", 1280);
                ReflectionHelpers.setField(renderer, "expectedHeight", 720);
                ReflectionHelpers.setField(renderer, "upscaler", upscaler);
                ReflectionHelpers.setField(renderer, "upscalingPolicy", policy);

                ReflectionHelpers.callInstanceMethod(renderer, "checkUpscalingOutputFormat");

                // The host answered with 1080p, not the 720p that was asked for: the upscaler is
                // rebuilt for the picture that arrived, so initialWidth follows the picture.
                assertEquals(1920, (int) ReflectionHelpers.getField(renderer, "initialWidth"));
                assertEquals(1080, (int) ReflectionHelpers.getField(renderer, "initialHeight"));
            } finally {
                upscaler.close();
                surface.release();
                texture.release();
            }
        }
    }

    @Test
    public void aRebuildIsSizedByTheSurfaceTheUpscalerMeasured() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            MediaCodecDecoderRenderer renderer = renderer(activity.get());
            SurfaceTexture texture = new SurfaceTexture(0);
            Surface surface = new Surface(texture);
            try {
                // The stream still starts at 1280x720 on a 1280x720 surface, but the window has
                // since been resized to 2560x1440 (a split screen), which setRenderTarget never saw.
                ReflectionHelpers.setField(renderer, "renderTarget", surface);
                ReflectionHelpers.setField(renderer, "renderWidth", 1280);
                ReflectionHelpers.setField(renderer, "renderHeight", 720);
                ReflectionHelpers.setField(renderer, "initialWidth", 1280);
                ReflectionHelpers.setField(renderer, "initialHeight", 720);

                ReflectionHelpers.callInstanceMethod(renderer, "setupUpscaling",
                        ReflectionHelpers.ClassParameter.from(int[].class, new int[] {2560, 1440}));

                UpscalingPolicy policy = ReflectionHelpers.getField(renderer, "upscalingPolicy");
                // Up to 1280x720 inside 2560x1440 is no upscaling at all; the measured surface is
                // what decides, not the size recorded when the stream started.
                assertNotEquals(UpscalingPolicy.Reason.NO_UPSCALE, policy.getReason());
            } finally {
                GlesUpscaler built = ReflectionHelpers.getField(renderer, "upscaler");
                if (built != null) built.close();
                surface.release();
                texture.release();
            }
        }
    }
}
