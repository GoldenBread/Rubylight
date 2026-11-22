package com.limelight.nvstream;

import org.junit.Test;

import static org.junit.Assert.assertEquals;

public class ClientHintBitrateControllerTest {
    @Test
    public void singleModerateLossSampleResetsStableStreak() {
        ClientHintBitrateController controller = new ClientHintBitrateController(20000, 0, 0);
        // 4 clean samples past START_DELAY_MS (3000 ms), one short of STABLE_SAMPLES (5).
        assertEquals(0, controller.sample(3000, true, 0f, 0f));
        assertEquals(0, controller.sample(4000, true, 0f, 0f));
        assertEquals(0, controller.sample(5000, true, 0f, 0f));
        assertEquals(0, controller.sample(6000, true, 0f, 0f));
        // A single 3% loss sample (2 < loss <= 5) does not cut yet (lossStreak == 1),
        // but must reset stableSamples so the next clean sample does not probe up.
        assertEquals(0, controller.sample(7000, true, 3.0f, 0f));
        assertEquals(0, controller.sample(8000, true, 0f, 0f));
        // 4 more clean samples (5 total since the loss sample) are needed to probe up.
        assertEquals(0, controller.sample(9000, true, 0f, 0f));
        assertEquals(0, controller.sample(10000, true, 0f, 0f));
        assertEquals(0, controller.sample(11000, true, 0f, 0f));
        assertEquals(21000, controller.sample(12000, true, 0f, 0f));
    }
}
