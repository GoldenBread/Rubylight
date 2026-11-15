package com.limelight.utils;

import android.app.Activity;
import android.content.Context;
import androidx.appcompat.app.AlertDialog;
import com.limelight.ui.NavigationDialogBuilder;
import com.google.android.material.progressindicator.LinearProgressIndicator;

import com.limelight.R;
import com.limelight.binding.PlatformBinding;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.NvHTTP;

import org.xmlpull.v1.XmlPullParserException;

import java.io.IOException;

public final class PyroWaveBandwidthTest {
    /** Whether each Sunshine-family PC (by uuid) has Foundation Sunshine's bandwidth test, asked once per run. */
    private static final java.util.concurrent.ConcurrentHashMap<String, Boolean> FOUNDATION_TEST =
            new java.util.concurrent.ConcurrentHashMap<>();

    private final Activity activity;
    private final ComputerDetails computer;
    private final String uniqueId;
    private final int bitrateKbps;
    private volatile boolean cancelled;
    private volatile NvHTTP http;
    private AlertDialog dialog;
    private Thread worker;

    /** Loss above this many percent shows as stutter. */
    static final double LOSS_STUTTERS = 1.0;
    /** Jitter above this many milliseconds shows as uneven frames. */
    static final double JITTER_STUTTERS = 10.0;
    /** The measured speed should carry the stream's bitrate this many times over. */
    static final double SPEED_HEADROOM = 1.5;

    /** {@code bitrateKbps} is the stream's bitrate, which the verdict weighs a measured speed by. */
    public PyroWaveBandwidthTest(Activity activity, ComputerDetails computer, String uniqueId, int bitrateKbps) {
        this.activity = activity;
        this.computer = new ComputerDetails(computer);
        this.uniqueId = uniqueId;
        this.bitrateKbps = bitrateKbps;
    }

    public void show() {
        dialog = new NavigationDialogBuilder(activity)
                .setTitle(R.string.connection_test_title)
                .setMessage(measuresSpeed() ? R.string.connection_test_intro : R.string.connection_test_intro_delay)
                .setPositiveButton(R.string.bandwidth_probe_start, (d, which) -> start())
                .setNegativeButton(android.R.string.cancel, (d, which) -> cancel())
                .setOnCancelListener(d -> cancel()).create();
        dialog.show();
    }

    private void start() {
        LinearProgressIndicator progress = new LinearProgressIndicator(activity);
        progress.setMax(100);
        progress.setIndeterminate(true);
        int padding = Math.round(24 * activity.getResources().getDisplayMetrics().density);
        progress.setPadding(padding, 0, padding, 0);
        dialog = new NavigationDialogBuilder(activity)
                .setTitle(R.string.connection_test_title)
                .setMessage(R.string.connection_test_running)
                .setView(progress)
                .setNegativeButton(android.R.string.cancel, (d, which) -> cancel())
                .setOnCancelListener(d -> cancel()).create();
        dialog.show();
        com.limelight.LimeLog.info("Connection test started");
        worker = new Thread(() -> {
            ConnectionTestStats stats = new ConnectionTestStats();
            // The second line of the result: the speed, or why it was not measured. Hosts that can
            // never measure speed get no second line.
            String speed = null;
            double measuredMbps = Double.NaN;
            try {
                http = new NvHTTP(ServerHelper.getCurrentAddressFromComputer(computer), computer.httpsPort,
                        uniqueId, computer.serverCert, PlatformBinding.getCryptoProvider(activity.getApplicationContext()));
                if (cancelled) {
                    http.cancelPendingRequests();
                    return;
                }
                for (int sample = 0; sample < 10 && !cancelled; sample++) {
                    try {
                        stats.add(http.probeServerLatency());
                    } catch (IOException | XmlPullParserException e) {
                        stats.add(Double.NaN);
                    }
                    Thread.sleep(100);
                }
                if (cancelled) {
                    return;
                }
                if (!measuresSpeed()) {
                    speed = null;
                } else if (computer.serverCert == null) {
                    speed = activity.getString(R.string.connection_test_speed_needs_pairing);
                } else if (Double.isNaN(stats.latencyMs())) {
                    speed = activity.getString(R.string.connection_test_speed_no_answer);
                } else {
                    ComputerDetails current = http.getComputerDetails(true);
                    if (current.supportsPyroWaveBandwidthProbe()) {
                        double mbps = http.probePyroWaveBandwidth(current, percent -> activity.runOnUiThread(() -> {
                            if (!cancelled && !activity.isFinishing() && !activity.isDestroyed()) {
                                progress.setIndeterminate(false);
                                progress.setProgressCompat(percent, true);
                            }
                        }));
                        speed = speedText(activity, mbps, current.pyroWaveHostLinkMbps);
                        measuredMbps = mbps;
                    } else if (current.caps.family() == HostCapabilities.Family.SUNSHINE) {
                        // Foundation Sunshine's own test, as Moonlight V+ runs it. Other Sunshine PCs
                        // (stock, Apollo, Vibepollo) have none: they get the delay test alone.
                        if (hasFoundationTest(http, computer.uuid)) {
                            try {
                                double mbps = http.probeFoundationBandwidth(percent -> activity.runOnUiThread(() -> {
                                    if (!cancelled && !activity.isFinishing() && !activity.isDestroyed()) {
                                        progress.setIndeterminate(false);
                                        progress.setProgressCompat(percent, true);
                                    }
                                }));
                                speed = speedText(activity, mbps, 0);
                                measuredMbps = mbps;
                            } catch (java.io.FileNotFoundException e) {
                                speed = activity.getString(R.string.connection_test_speed_unsupported);
                            }
                        }
                    } else {
                        speed = activity.getString(R.string.connection_test_speed_unsupported);
                    }
                }
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
                return;
            } catch (IOException | XmlPullParserException e) {
                speed = measuresSpeed() ? activity.getString(R.string.connection_test_failed) : null;
            }
            com.limelight.LimeLog.info("Connection test finished");
            final String numbers = result(activity, stats, speed);
            final int[] verdict = verdict(stats, measuredMbps, bitrateKbps);
            final String message = verdict[1] == 0 ? activity.getString(verdict[0])
                    : activity.getString(verdict[0]) + "\n" + activity.getString(verdict[1]);
            activity.runOnUiThread(() -> {
                if (cancelled || activity.isFinishing() || activity.isDestroyed()) {
                    return;
                }
                dialog.dismiss();
                dialog = new NavigationDialogBuilder(activity)
                        .setTitle(R.string.connection_test_title)
                        .setMessage(message)
                        .setView(details(numbers))
                        .setPositiveButton(android.R.string.ok, null)
                        .setNeutralButton(R.string.pair_try_again, (d, which) -> start()).create();
                dialog.show();
            });
        }, "Connection test");
        worker.start();
    }

    /** Only hosts with the download test measure speed; the others are tested for delay alone. */
    private boolean measuresSpeed() {
        return computer.caps.mayHave(HostCapabilities.Feature.PYROWAVE_BANDWIDTH_PROBE) ||
                (computer.caps.family() == HostCapabilities.Family.SUNSHINE &&
                        !Boolean.FALSE.equals(computer.uuid == null ? null : FOUNDATION_TEST.get(computer.uuid)));
    }

    /** Asks the PC once whether it has Foundation Sunshine's test; a question that did not get through is asked again. */
    static boolean hasFoundationTest(NvHTTP http, String uuid) throws IOException {
        Boolean known = uuid == null ? null : FOUNDATION_TEST.get(uuid);
        if (known != null) return known;
        boolean found = http.supportsFoundationBandwidthTest();
        if (uuid != null) FOUNDATION_TEST.put(uuid, found);
        return found;
    }

    /** Details, which shows or hides the measured numbers under the verdict. */
    private android.view.View details(String numbers) {
        int padding = Math.round(24 * activity.getResources().getDisplayMetrics().density);
        android.widget.LinearLayout column = new android.widget.LinearLayout(activity);
        column.setOrientation(android.widget.LinearLayout.VERTICAL);
        column.setPadding(padding, 0, padding, 0);
        android.widget.Button toggle = new android.widget.Button(activity, null, android.R.attr.borderlessButtonStyle);
        toggle.setText(R.string.title_details);
        android.widget.TextView text = new android.widget.TextView(activity);
        androidx.core.widget.TextViewCompat.setTextAppearance(text, R.style.TextAppearance_Nightjar_Caption);
        text.setText(numbers);
        text.setVisibility(android.view.View.GONE);
        toggle.setOnClickListener(v -> text.setVisibility(text.getVisibility() == android.view.View.VISIBLE
                ? android.view.View.GONE : android.view.View.VISIBLE));
        column.addView(toggle, new android.widget.LinearLayout.LayoutParams(
                android.view.ViewGroup.LayoutParams.WRAP_CONTENT, android.view.ViewGroup.LayoutParams.WRAP_CONTENT));
        column.addView(text);
        return column;
    }

    /**
     * What the test means for streaming, as desktop's {@code Connection::verdict}: the verdict's string and its next
     * step (0 for none). Can't connect, then stutter from loss, jitter or a speed short of {@code bitrateKbps}
     * ({@code speedMbps} NaN when the speed was not measured), else good.
     */
    static int[] verdict(ConnectionTestStats stats, double speedMbps, int bitrateKbps) {
        double loss = stats.lossPercent();
        if (!Double.isFinite(stats.latencyMs()) || (Double.isFinite(loss) && loss >= 100.0)) {
            return new int[] {R.string.connection_test_cant_connect, R.string.connection_test_cant_connect_step};
        }
        if (Double.isFinite(loss) && loss > LOSS_STUTTERS) {
            return new int[] {R.string.connection_test_stutters, R.string.connection_test_step_loss};
        }
        double jitter = stats.jitterMs();
        if (Double.isFinite(jitter) && jitter > JITTER_STUTTERS) {
            return new int[] {R.string.connection_test_stutters, R.string.connection_test_step_jitter};
        }
        if (Double.isFinite(speedMbps) && speedMbps * 1000.0 < bitrateKbps * SPEED_HEADROOM) {
            return new int[] {R.string.connection_test_stutters, R.string.connection_test_step_speed};
        }
        return new int[] {R.string.connection_test_good, 0};
    }

    /** "Ping 63 ms · Jitter 41 ms · Loss 0.0%", then the speed line when there is one. */
    static String result(Context context, ConnectionTestStats stats, String speed) {
        String delay = context.getString(R.string.connection_test_stats, milliseconds(context, stats.latencyMs()),
                milliseconds(context, stats.jitterMs()), percent(context, stats.lossPercent()));
        return speed == null ? delay : delay + "\n" + speed;
    }

    static String speedText(Context context, double mbps, long linkMbps) {
        String speed = context.getString(R.string.connection_test_speed, mbps);
        return linkMbps > 0 ? speed + " · " + context.getString(R.string.bandwidth_probe_link, linkMbps) : speed;
    }

    /** Whole milliseconds, with one decimal below 10 ms where the difference still matters. */
    private static String milliseconds(Context context, double value) {
        if (Double.isNaN(value)) {
            return context.getString(R.string.connection_test_unavailable);
        }
        return context.getString(R.string.connection_test_ms,
                String.format(java.util.Locale.getDefault(), value < 10 ? "%.1f" : "%.0f", value));
    }

    private static String percent(Context context, double value) {
        if (Double.isNaN(value)) {
            return context.getString(R.string.connection_test_unavailable);
        }
        return context.getString(R.string.connection_test_percent, String.format(java.util.Locale.getDefault(), "%.1f", value));
    }

    public void cancel() {
        cancelled = true;
        if (worker != null) {
            worker.interrupt();
        }
        NvHTTP request = http;
        if (request != null) {
            request.cancelPendingRequests();
        }
        if (dialog != null) {
            dialog.dismiss();
        }
    }
}
