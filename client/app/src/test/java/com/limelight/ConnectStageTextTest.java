package com.limelight;

import android.app.Application;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

import static org.junit.Assert.assertEquals;

/** The connecting screen names each start stage in plain words, desktop's stage_text words. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class)
public class ConnectStageTextTest {
    @Test public void stagesAreNamedInPlainWords() {
        assertEquals(R.string.conn_stage_finding, GameConnection.stageText("name resolution"));
        assertEquals(R.string.conn_stage_setup, GameConnection.stageText("RTSP handshake"));
        assertEquals(R.string.conn_stage_stream, GameConnection.stageText("Control stream establishment"));
        assertEquals(R.string.conn_stage_video, GameConnection.stageText("video stream initialization"));
        assertEquals(R.string.conn_stage_audio, GameConnection.stageText("audio stream establishment"));
        assertEquals(R.string.conn_stage_input, GameConnection.stageText("input stream establishment"));
        // NvConnection reports the app's own launch under its name: no stage, the plain word.
        assertEquals(R.string.conn_starting, GameConnection.stageText("Celeste"));
        assertEquals(R.string.conn_starting, GameConnection.stageText(null));
    }
}
