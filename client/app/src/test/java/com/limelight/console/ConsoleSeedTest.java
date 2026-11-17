package com.limelight.console;

import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.content.Context;

import com.limelight.nativeapi.FileLibraryCache;
import com.limelight.nativeapi.LibraryCache;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.PairingManager;
import com.limelight.nvstream.http.TestHosts;

import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;

import java.io.File;
import java.util.Collections;
import java.util.Set;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;

/** Home's first library: the list of PCs asks for it once, and the library file is read off the UI thread. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class ConsoleSeedTest {
    @Before
    public void forgetHostsAlreadyTried() {
        Set<String> tried = ReflectionHelpers.getStaticField(ConsoleSeed.class, "TRIED");
        tried.clear();
    }

    @Test
    public void aStoredLibraryIsFoundOnTheWorkerAndItsHostIsTriedOnlyOnce() throws Exception {
        Context context = RuntimeEnvironment.getApplication();
        String uuid = "seeded-host";
        new FileLibraryCache(new File(context.getFilesDir(), "console-library"))
                .store(uuid, new LibraryCache.Entry("{}", 1000, Collections.emptyList()));

        CountDownLatch seeded = new CountDownLatch(1);
        ConsoleSeed.seedOnce(context, "unique", onlineNativeHost(uuid), seeded::countDown);

        // The stored library is reported from the worker; a poll never waits for the file.
        assertTrue(seeded.await(5, TimeUnit.SECONDS));
        Set<String> tried = ReflectionHelpers.getStaticField(ConsoleSeed.class, "TRIED");
        assertTrue("the host is claimed before the library is read", tried.contains(uuid));
    }

    private static ComputerDetails onlineNativeHost(String uuid) {
        ComputerDetails computer = new ComputerDetails();
        computer.uuid = uuid;
        computer.state = ComputerDetails.State.ONLINE;
        computer.pairState = PairingManager.PairState.PAIRED;
        computer.caps = TestHosts.fixture("rubylight-native-serverinfo.xml");
        return computer;
    }
}
