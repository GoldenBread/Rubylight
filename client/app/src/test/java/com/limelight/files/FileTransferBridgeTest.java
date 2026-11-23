package com.limelight.files;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import android.app.Application;

import com.limelight.nvstream.jni.FileTransferBridge;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

import java.util.List;

/** Real-JNI lifecycle tests for {@link FileTransferBridge} queue/snapshot/cancel/destroy. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class FileTransferBridgeTest {
    @Test
    public void queueCancelAndDestroyRoundTripThroughRealRustJni() {
        assertTrue(FileTransferBridge.isAvailable());
        long handle = FileTransferBridge.create();
        assertTrue(handle > 0);
        try {
            long uploadId = FileTransferBridge.queueUpload(handle, "notes.txt", 128);
            long downloadId = FileTransferBridge.queueDownload(handle, "save.dat", "save.dat", 256);
            assertTrue(uploadId > 0);
            assertTrue(downloadId > 0);

            List<FileRow> rows = FileRow.parse(FileTransferBridge.snapshot(handle));
            assertEquals(2, rows.size());
            assertTrue(rows.get(0).upload);
            assertEquals("notes.txt", rows.get(0).name);
            assertEquals(FileRow.State.QUEUED, rows.get(0).state);
            assertFalse(rows.get(1).upload);
            assertEquals("save.dat", rows.get(1).name);

            assertTrue(FileTransferBridge.cancel(handle, uploadId));
            List<FileRow> afterCancel = FileRow.parse(FileTransferBridge.snapshot(handle));
            assertEquals(FileRow.State.CANCELLED, afterCancel.get(0).state);
            assertTrue(afterCancel.get(0).isFinished());

            FileTransferBridge.clearFinished(handle);
            List<FileRow> remaining = FileRow.parse(FileTransferBridge.snapshot(handle));
            assertEquals(1, remaining.size());
            assertEquals(downloadId, remaining.get(0).id);

            assertFalse(FileTransferBridge.cancel(handle, uploadId));
        } finally {
            FileTransferBridge.destroy(handle);
        }
    }
}
