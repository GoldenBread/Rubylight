package com.limelight.binding.input.driver;

import android.app.Application;
import android.hardware.usb.UsbDevice;
import android.hardware.usb.UsbDeviceConnection;
import android.hardware.usb.UsbInterface;

import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import org.robolectric.shadow.api.Shadow;
import org.robolectric.util.ReflectionHelpers;

import java.util.Arrays;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 33, application = Application.class, shadows = {UsbPads.Connection.class,
        com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class},
        instrumentedPackages = "com.limelight.nvstream.jni")
public class XboxOutputTest {
    private final UsbPads.Recorder listener = new UsbPads.Recorder();

    @Before
    public void resetPads() {
        UsbPads.Connection.reset();
    }

    private XboxOneController startedXboxOne() {
        UsbDevice device = UsbPads.xboxOne("/dev/bus/usb/001/002");
        XboxOneController pad = new XboxOneController(device, Shadow.newInstanceOf(UsbDeviceConnection.class), 11, listener);
        UsbInterface iface = device.getInterface(0);
        ReflectionHelpers.setField(pad, "inEndpt", iface.getEndpoint(0));
        ReflectionHelpers.setField(pad, "outEndpt", iface.getEndpoint(1));
        return pad;
    }

    private Xbox360Controller startedXbox360() {
        UsbDevice device = UsbPads.gameSir("/dev/bus/usb/001/003", 0x045e, false);
        Xbox360Controller pad = new Xbox360Controller(device, Shadow.newInstanceOf(UsbDeviceConnection.class), 12, listener);
        UsbInterface iface = device.getInterface(0);
        ReflectionHelpers.setField(pad, "inEndpt", iface.getEndpoint(0));
        ReflectionHelpers.setField(pad, "outEndpt", iface.getEndpoint(1));
        return pad;
    }

    @Test
    public void xboxOneRumbleAndTriggersNeverWaitForUsbAndCoalesce() throws Exception {
        XboxOneController pad = startedXboxOne();
        UsbPads.holdOutputs();
        pad.startOutput();
        pad.rumble((short) 1000, (short) 0);
        UsbPads.awaitOutputHeld();
        for (int i = 2; i <= 20; i++) {
            pad.rumble((short) (i * 1000), (short) (i * 500));
            pad.rumbleTriggers((short) (i * 1000), (short) (i * 250));
        }
        assertTrue("rumble() waited for the USB write", UsbPads.Connection.outputHeld.get());
        UsbPads.releaseOutputs();

        byte[] expected = XboxOneController.createRumblePacket((byte) 0,
                (short) 20000, (short) 10000, (short) 20000, (short) 5000);
        UsbPads.await(() -> UsbPads.Connection.outputs.stream().anyMatch(report ->
                report.length == expected.length &&
                        report[6] == expected[6] && report[7] == expected[7] &&
                        report[8] == expected[8] && report[9] == expected[9]));
        assertTrue("Wrote " + UsbPads.Connection.outputs.size() + " reports", UsbPads.Connection.outputs.size() <= 4);
        for (String thread : UsbPads.Connection.outputThreads) {
            assertEquals("Xbox One USB output", thread);
        }

        pad.stop();
        UsbPads.await(() -> UsbPads.Connection.closes.get() == 1);
        byte[] last = UsbPads.Connection.outputs.get(UsbPads.Connection.outputs.size() - 1);
        assertEquals(0, last[6]);
        assertEquals(0, last[7]);
        assertEquals(0, last[8]);
        assertEquals(0, last[9]);
    }

    @Test
    public void xboxOneRumbleDoesNotContendWithAckModeReportMonitorLock() throws Exception {
        XboxOneController pad = startedXboxOne();
        pad.startOutput();
        CountDownLatch lockHeld = new CountDownLatch(1);
        CountDownLatch releaseLock = new CountDownLatch(1);
        Thread holder = new Thread(() -> {
            synchronized (pad) {
                lockHeld.countDown();
                try {
                    releaseLock.await(5, TimeUnit.SECONDS);
                } catch (InterruptedException e) {
                    Thread.currentThread().interrupt();
                }
            }
        });
        holder.start();
        assertTrue(lockHeld.await(2, TimeUnit.SECONDS));
        try {
            // Even while ackModeReport holds `synchronized (pad)`, rumble() and rumbleTriggers() return immediately.
            pad.rumble((short) 0x8000, (short) 0x4000);
            pad.rumbleTriggers((short) 0x2000, (short) 0x1000);
            UsbPads.await(() -> !UsbPads.Connection.outputs.isEmpty());
        } finally {
            releaseLock.countDown();
            holder.join(2000);
            pad.stop();
            UsbPads.await(() -> UsbPads.Connection.closes.get() == 1);
        }
    }

    @Test
    public void xbox360RumbleNeverWaitsForUsbAndCoalesces() throws Exception {
        Xbox360Controller pad = startedXbox360();
        UsbPads.holdOutputs();
        pad.startOutput();
        pad.rumble((short) 1000, (short) 0);
        UsbPads.awaitOutputHeld();
        for (int i = 2; i <= 20; i++) {
            pad.rumble((short) (i * 1000), (short) (i * 500));
        }
        assertTrue("rumble() waited for the USB write", UsbPads.Connection.outputHeld.get());
        UsbPads.releaseOutputs();

        byte[] expected = Xbox360Controller.createRumblePacket((short) 20000, (short) 10000);
        UsbPads.await(() -> UsbPads.Connection.outputs.stream().anyMatch(report -> Arrays.equals(expected, report)));
        assertTrue("Wrote " + UsbPads.Connection.outputs.size() + " reports", UsbPads.Connection.outputs.size() <= 4);
        for (String thread : UsbPads.Connection.outputThreads) {
            assertEquals("Xbox 360 USB output", thread);
        }

        pad.stop();
        UsbPads.await(() -> UsbPads.Connection.closes.get() == 1);
        byte[] last = UsbPads.Connection.outputs.get(UsbPads.Connection.outputs.size() - 1);
        assertArrayEquals(Xbox360Controller.createRumblePacket((short) 0, (short) 0), last);
    }
}
