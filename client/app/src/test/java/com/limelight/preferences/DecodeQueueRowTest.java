package com.limelight.preferences;

import android.app.Application;

import com.limelight.R;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

import static org.junit.Assert.assertTrue;

/**
 * "Frames waiting for the decoder" says where it does nothing: a decoder that takes frames directly
 * (MediaCodecHelper's direct-submit list, every Codec2 decoder among them) has no queue to bound.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class)
public class DecodeQueueRowTest {
    @Test public void theRowSaysDecodersThatTakeFramesDirectlyHaveNoQueue() {
        String help = RuntimeEnvironment.getApplication().getString(R.string.summary_decode_queue_frames);
        assertTrue(help, help.endsWith("Most hardware decoders take each frame directly and have no queue, so the choice does nothing for them."));
    }
}
