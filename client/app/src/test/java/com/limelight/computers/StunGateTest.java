package com.limelight.computers;

import android.content.Context;
import android.net.ConnectivityManager;
import android.net.Network;
import android.net.NetworkCapabilities;
import android.net.NetworkInfo;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.Shadows;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowConnectivityManager;
import org.robolectric.shadows.ShadowNetwork;
import org.robolectric.shadows.ShadowNetworkCapabilities;
import org.robolectric.shadows.ShadowNetworkInfo;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNull;

/**
 * Verifies Android's VPN-bypass STUN network selector skips VPN and cellular transports while
 * selecting a non-VPN local network for {@link StunClient} socket binding.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34)
public class StunGateTest {
    private static NetworkCapabilities capsWithTransport(int transport) {
        NetworkCapabilities caps = ShadowNetworkCapabilities.newInstance();
        Shadows.shadowOf(caps).addTransportType(transport);
        return caps;
    }

    @Test
    public void stunNetworkSkipsVpnAndCellularAndPicksLanNetwork() {
        ConnectivityManager connMgr = (ConnectivityManager) RuntimeEnvironment.getApplication()
                .getSystemService(Context.CONNECTIVITY_SERVICE);
        ShadowConnectivityManager shadow = Shadows.shadowOf(connMgr);
        shadow.clearAllNetworks();

        NetworkInfo info = ShadowNetworkInfo.newInstance(
                NetworkInfo.DetailedState.CONNECTED, ConnectivityManager.TYPE_WIFI, 0, true, NetworkInfo.State.CONNECTED);
        Network vpn = ShadowNetwork.newInstance(10);
        Network cell = ShadowNetwork.newInstance(11);
        shadow.addNetwork(vpn, info);
        shadow.setNetworkCapabilities(vpn, capsWithTransport(NetworkCapabilities.TRANSPORT_VPN));
        shadow.addNetwork(cell, info);
        shadow.setNetworkCapabilities(cell, capsWithTransport(NetworkCapabilities.TRANSPORT_CELLULAR));

        assertNull(ComputerManagerService.stunNetwork(connMgr));

        Network wifi = ShadowNetwork.newInstance(12);
        shadow.addNetwork(wifi, info);
        shadow.setNetworkCapabilities(wifi, capsWithTransport(NetworkCapabilities.TRANSPORT_WIFI));

        assertEquals(wifi, ComputerManagerService.stunNetwork(connMgr));
    }
}
