package com.limelight.binding.video;

import java.nio.ByteBuffer;
import java.util.ArrayList;
import java.util.Arrays;

import org.jcodec.codecs.h264.H264Utils;
import org.jcodec.codecs.h264.io.model.SeqParameterSet;
import org.jcodec.codecs.h264.io.model.VUIParameters;

import android.media.MediaCodec;
import android.os.Build;
import android.os.Process;
import android.os.SystemClock;

import com.limelight.LimeLog;
import com.limelight.nvstream.jni.MoonBridge;

/**
 * Handles MediaCodec input buffer acquisition, submit handoff queuing, CSD
 * batching, H.264 SPS patching, input spacing, and PyroWave submission
 * bookkeeping for {@link MediaCodecDecoderRenderer}.
 */
final class MediaCodecDecodeLoop {
    private final MediaCodecDecoderRenderer r;

    private final ArrayList<byte[]> vpsBuffers = new ArrayList<>();
    private final ArrayList<byte[]> spsBuffers = new ArrayList<>();
    private final ArrayList<byte[]> ppsBuffers = new ArrayList<>();
    private boolean submittedCsd;

    private int nextInputBufferIndex = -1;
    ByteBuffer nextInputBuffer;
    // When the decoder first had no input buffer for a frame since it last gave one, else -1.
    private volatile long noInputSinceMs = -1;
    private final SubmitHandoff.BytePool handoffPool = new SubmitHandoff.BytePool();
    private boolean badUnitLogged;

    boolean needsSpsBitstreamFixup, isExynos4;
    boolean fusedIdrFrame;
    boolean constrainedHighProfile;
    boolean needsBaselineSpsHack;
    private SeqParameterSet savedSps;

    boolean dolbyVisionPaddingLogged;
    boolean vividSeiDropLogged;
    final int[] vividSeiRange = new int[2];

    long lastTimestampUs;
    private long lastDecoderDrops;
    private long baseTimestampUs;
    long rawInputPtr;
    private int pyroWaveFailures;
    private boolean csdSubmittedForThisFrame;
    private int currentCodecFlags;

    MediaCodecDecodeLoop(MediaCodecDecoderRenderer renderer) {
        this.r = renderer;
    }

    /** A unit as the worker thread takes it: the engine reuses its buffers once the call returns. */
    static final class HandedUnit {
        final byte[] data;
        final int length, type, frameNumber, frameType;
        final char hostProcessingLatency;
        final long receiveTimeUs, enqueueTimeUs, receiveTimeNs, presentationTimeUs;

        HandedUnit(byte[] data, int length, int type, int frameNumber, int frameType, char hostProcessingLatency,
                   long receiveTimeUs, long enqueueTimeUs, long receiveTimeNs, long presentationTimeUs) {
            this.data = data;
            this.length = length;
            this.type = type;
            this.frameNumber = frameNumber;
            this.frameType = frameType;
            this.hostProcessingLatency = hostProcessingLatency;
            this.receiveTimeUs = receiveTimeUs;
            this.enqueueTimeUs = enqueueTimeUs;
            this.receiveTimeNs = receiveTimeNs;
            this.presentationTimeUs = presentationTimeUs;
        }
    }

    void clearInputBuffersOnRecovery() {
        nextInputBuffer = null;
        nextInputBufferIndex = -1;
        // The new codec's wait for a buffer counts from its own first miss.
        noInputSinceMs = -1;
    }

    void clearCsdOnConfigure() {
        submittedCsd = false;
        vpsBuffers.clear();
        spsBuffers.clear();
        ppsBuffers.clear();
    }

    boolean fetchNextInputBuffer() {
        return fetchNextInputBuffer(false);
    }

    // With prefetch (right after a frame was queued) the decoder is only asked for a free buffer,
    // never waited for: see InputBufferWait. A prefetch that finds none succeeds, and the next
    // frame's own fetch waits, with the stall count and the hang detection.
    private boolean fetchNextInputBuffer(boolean prefetch) {
        long startTime;
        boolean codecRecovered;

        if (nextInputBuffer != null) {
            return true;
        }

        startTime = SystemClock.uptimeMillis();
        r.inputWaitSinceMs = startTime;
        long waitedMs = 0;
        // The hang limit runs from the first try that found no buffer, also one the receive thread
        // made before it handed the frame to the worker (see SubmitHandoff).
        long streakStart = !prefetch && noInputSinceMs >= 0 ? noInputSinceMs : startTime;

        try {
            int attempts = 0;
            while (nextInputBufferIndex < 0 && !r.stopping &&
                    r.codecRecoveryType.get() == MediaCodecDecoderRenderer.CR_RECOVERY_TYPE_NONE &&
                    InputBufferWait.askAgain(prefetch, attempts, SystemClock.uptimeMillis() - streakStart)) {
                // Under input pressure, recheck recovery and shutdown without a 10 ms wait.
                nextInputBufferIndex = r.videoDecoder.dequeueInputBuffer(InputBufferWait.timeoutUs(prefetch));
                attempts++;
            }

            if (nextInputBufferIndex >= 0) {
                nextInputBuffer = r.videoDecoder.getInputBuffer(nextInputBufferIndex);
                if (nextInputBuffer == null) {
                    nextInputBufferIndex = -1;
                }
            }
        } catch (IllegalStateException e) {
            r.recovery.handleDecoderException(e);
            return false;
        } finally {
            waitedMs = SystemClock.uptimeMillis() - startTime;
            if (nextInputBuffer == null && waitedMs >= InputBufferWait.HANG_MS) {
                // A wait as long as a hang is no decoder running well. Cleared while the wait still
                // shows: recordVendorKeyHang could take the moment before the hang check below.
                r.session.vendorKeyHangUnrecorded.set(false);
            }
            r.inputWaitSinceMs = -1;
            codecRecovered = r.recovery.doCodecRecoveryIfRequired(MediaCodecDecoderRenderer.CR_FLAG_INPUT_THREAD);
        }

        if (codecRecovered) {
            return false;
        }

        if (nextInputBuffer == null && !InputBufferWait.missingBufferFails(prefetch)) {
            return true;
        }

        int deltaMs = (int) (SystemClock.uptimeMillis() - streakStart);
        if (nextInputBuffer != null) {
            noInputSinceMs = -1;
        } else if (noInputSinceMs < 0) {
            noInputSinceMs = streakStart;
        }

        if (deltaMs >= 20) {
            String stallLine = r.telemetry.inputStalls.onStall(SystemClock.uptimeMillis(), deltaMs);
            if (stallLine != null) LimeLog.warning(stallLine);
        }

        if (nextInputBuffer == null) {
            if (deltaMs >= InputBufferWait.HANG_MS && (r.recovery.unresolvedInitialException() == null ||
                    SystemClock.uptimeMillis() - r.recovery.initialExceptionTimestamp >= MediaCodecRecovery.EXCEPTION_REPORT_DELAY_MS)) {
                if (!r.recovery.resetHungDecoder(deltaMs)) {
                    r.telemetry.failDecoder(r.telemetry.decoderName() + " took no input for " + deltaMs + " ms" +
                            (r.framesOutAtHangReset != r.numFramesOut ? "" : r.recovery.hangResetWithoutKeys ?
                                    ", also after its setup without vendor low-latency keys" : ", also after a reset"),
                            new MediaCodecDecoderRenderer.DecoderHungException(deltaMs));
                }
            }
            return false;
        }

        return true;
    }

    boolean inputReadyForReceiveThread(int frameType, int unitType) {
        if (frameType == MoonBridge.FRAME_TYPE_IDR) {
            if (unitType == MoonBridge.BUFFER_TYPE_SPS || unitType == MoonBridge.BUFFER_TYPE_VPS ||
                    unitType == MoonBridge.BUFFER_TYPE_PPS) {
                return true;
            }
            if ((r.videoFormat & (MoonBridge.VIDEO_FORMAT_MASK_H264 | MoonBridge.VIDEO_FORMAT_MASK_H265)) != 0 &&
                    (!submittedCsd || !fusedIdrFrame)) {
                return false;
            }
        }
        MediaCodec decoder = r.videoDecoder;
        if (nextInputBuffer != null || nextInputBufferIndex >= 0 || decoder == null || r.stopping ||
                r.codecRecoveryType.get() != MediaCodecDecoderRenderer.CR_RECOVERY_TYPE_NONE) {
            return true;
        }
        long start = SystemClock.uptimeMillis();
        try {
            int attempts = 0;
            while (nextInputBufferIndex < 0 && !r.stopping &&
                    r.codecRecoveryType.get() == MediaCodecDecoderRenderer.CR_RECOVERY_TYPE_NONE &&
                    InputBufferWait.askAgainOnReceive(attempts, SystemClock.uptimeMillis() - start)) {
                nextInputBufferIndex = decoder.dequeueInputBuffer(InputBufferWait.WAIT_STEP_US);
                attempts++;
            }
        } catch (IllegalStateException e) {
            return true;
        }
        if (nextInputBufferIndex >= 0) {
            noInputSinceMs = -1;
            return true;
        }
        if (noInputSinceMs < 0) noInputSinceMs = start;
        return r.stopping || r.codecRecoveryType.get() != MediaCodecDecoderRenderer.CR_RECOVERY_TYPE_NONE;
    }

    int enqueueForHandoff(SubmitHandoff<HandedUnit> handoff, byte[] decodeUnitData, int decodeUnitLength,
                          int decodeUnitType, int frameNumber, int frameType, char frameHostProcessingLatency,
                          long receiveTimeUs, long enqueueTimeUs, long receiveTimeNs, long presentationTimeUs,
                          int frames, boolean keyUnit) {
        byte[] bytes = copyOfUnit(decodeUnitData, decodeUnitLength);
        HandedUnit unit = bytes == null ? null : new HandedUnit(bytes, decodeUnitLength, decodeUnitType,
                frameNumber, frameType, frameHostProcessingLatency, receiveTimeUs, enqueueTimeUs,
                receiveTimeNs, presentationTimeUs);
        return handoff.enqueue(unit, frames, keyUnit, frameNumber);
    }

    private byte[] copyOfUnit(byte[] array, int length) {
        if (array != null && array.length >= length) {
            byte[] copy = handoffPool.acquire(length);
            System.arraycopy(array, 0, copy, 0, length);
            return copy;
        }
        ByteBuffer direct = array == null ? r.directInput : null;
        if (direct != null && direct.limit() >= length) {
            byte[] copy = handoffPool.acquire(length);
            int origPos = direct.position();
            ((java.nio.Buffer) direct).position(0);
            try {
                direct.get(copy, 0, length);
            } finally {
                ((java.nio.Buffer) direct).position(origPos);
            }
            return copy;
        }
        if (!badUnitLogged) {
            badUnitLogged = true;
            LimeLog.severe("A frame of " + length + " bytes reached the input hand-off without its data (" +
                    (array != null ? "array of " + array.length : direct != null ? "short buffer" : "no buffer") +
                    "); dropping it and asking for a key frame");
        }
        return null;
    }

    void enableSubmitHandoff() {
        int bound = r.prefs.decodeQueueFrames <= 0 || r.prefs.decodeQueueFrames > 15 ? 15 : r.prefs.decodeQueueFrames;
        r.submitHandoff = new SubmitHandoff<>(bound, "InputSubmit",
                () -> Process.setThreadPriority(Process.THREAD_PRIORITY_URGENT_DISPLAY),
                r::submitHanded, u -> handoffPool.release(u.data));
    }

    boolean awaitSubmitWorker(SubmitHandoff<HandedUnit> handoff) {
        if (handoff.onWorkerThread()) {
            handoff.close(0);
            return true;
        }
        for (int waitedS = 1; !handoff.close(1000); waitedS++) {
            LimeLog.warning("Waiting for the input worker before releasing the decoder (" + waitedS + " s)");
            if (waitedS >= 30) {
                LimeLog.severe("The input worker did not end in 30 s; leaving its decoder unreleased");
                return false;
            }
        }
        return true;
    }

    private long decoderDropsTotal() {
        SubmitHandoff<HandedUnit> handoff = r.submitHandoff;
        return MoonBridge.getDecoderDrops() + (handoff == null ? 0 : handoff.drops());
    }

    /**
     * Common per-unit accounting before MediaCodec or PyroWave submission.
     * Returns 0 to continue, 1 for DR_OK, or 2 for DR_NEED_IDR.
     */
    int beforeDecodeUnit(int decodeUnitLength, int frameNumber, int frameType,
                         char frameHostProcessingLatency, long receiveTimeUs,
                         long receiveTimeNs, long presentationTimeUs) {
        // A frame's first unit (an IDR's VPS or SPS, or its picture): the frame may be the first at
        // a size the UI asked for.
        boolean restartForSize = frameNumber != r.lastFrameNumber && r.reconfigure.applyStreamChanges();

        r.lastVideoFrameTimeMs = SystemClock.uptimeMillis();
        KeyframeSafetyNet safetyNet = r.keyframeSafetyNet;

        if (r.lastFrameNumber == 0) {
            r.activeWindowVideoStats.measurementStartTimestamp = SystemClock.uptimeMillis();
            lastDecoderDrops = decoderDropsTotal();
        } else if (frameNumber != r.lastFrameNumber && frameNumber != r.lastFrameNumber + 1) {
            long decoderDrops = decoderDropsTotal();
            SubmitHandoff<HandedUnit> handoff = r.submitHandoff;
            // The frames dropped while waiting for the keyframe the net asked for were no network loss.
            if (safetyNet != null && frameType == MoonBridge.FRAME_TYPE_IDR &&
                    safetyNet.takeAnsweredGap(frameNumber - r.lastFrameNumber - 1)) {
                lastDecoderDrops = decoderDrops;
                if (handoff != null) handoff.takeKeyGap();
            } else if (handoff != null && frameType == MoonBridge.FRAME_TYPE_IDR && handoff.takeKeyGap()) {
                int gap = frameNumber - r.lastFrameNumber - 1;
                MediaCodecTelemetry.recordFrameGap(r.activeWindowVideoStats, gap,
                        Math.max(gap, decoderDrops - lastDecoderDrops));
                lastDecoderDrops = decoderDrops;
            } else {
                lastDecoderDrops += MediaCodecTelemetry.recordFrameGap(r.activeWindowVideoStats,
                        frameNumber - r.lastFrameNumber - 1, decoderDrops - lastDecoderDrops);
            }
        }

        // Reset CSD data for each IDR frame
        if (r.lastFrameNumber != frameNumber && frameType == MoonBridge.FRAME_TYPE_IDR) {
            SubmitHandoff<HandedUnit> handedOff = r.submitHandoff;
            if (handedOff != null) handedOff.takeKeyGap();
            vpsBuffers.clear();
            spsBuffers.clear();
            ppsBuffers.clear();
            if (safetyNet != null) safetyNet.onKeyframe(r.lastVideoFrameTimeMs);
            r.telemetry.keyframesReceived++;
        }

        if (frameNumber != r.lastFrameNumber) {
            r.telemetry.queueDelay.onFrame(receiveTimeUs, presentationTimeUs, frameHostProcessingLatency * 100L);
            if (receiveTimeNs > 0) r.telemetry.lateFramesClockOffsetUs = receiveTimeUs - receiveTimeNs / 1000;
            r.lateFrames.push(frameNumber, presentationTimeUs, frameHostProcessingLatency * 100L, receiveTimeUs,
                    (r.streamFrameRateMilliHz + 500) / 1000);
        }
        r.lastFrameNumber = frameNumber;

        // Flip stats windows roughly every second
        if (SystemClock.uptimeMillis() >= r.activeWindowVideoStats.measurementStartTimestamp + 1000) {
            r.telemetry.flipStatsWindow();
        }

        r.telemetry.receivedVideoBytes.addAndGet(decodeUnitLength);

        if (restartForSize) {
            r.recovery.doCodecRecoveryIfRequired(MediaCodecDecoderRenderer.CR_FLAG_INPUT_THREAD);
            if (r.stopping) return 1;
            if (frameType != MoonBridge.FRAME_TYPE_IDR) return 2;
        }
        return 0;
    }

    /**
     * Handles IDR CSD batching and out-of-band CSD submission.
     * Returns 0 to continue submitting picture data, 1 for DR_OK, or 2 for DR_NEED_IDR.
     */
    int handleIdrCsd(byte[] decodeUnitData, int decodeUnitLength, int decodeUnitType, int frameType) {
        csdSubmittedForThisFrame = false;
        if (frameType != MoonBridge.FRAME_TYPE_IDR) return 0;

        if (decodeUnitType == MoonBridge.BUFFER_TYPE_SPS && r.videoFormat == MoonBridge.VIDEO_FORMAT_H264) {
            r.numSpsIn++;
            spsBuffers.add(patchH264Sps(decodeUnitData, decodeUnitLength));
            return 1;
        } else if (decodeUnitType == MoonBridge.BUFFER_TYPE_VPS) {
            r.numVpsIn++;
            vpsBuffers.add(Arrays.copyOf(decodeUnitData, decodeUnitLength));
            return 1;
        } else if (decodeUnitType == MoonBridge.BUFFER_TYPE_SPS) {
            // Preserve HEVC and H.264 4:4:4 SPS data without the 4:2:0 fixups above.
            r.numSpsIn++;
            spsBuffers.add(Arrays.copyOf(decodeUnitData, decodeUnitLength));
            return 1;
        } else if (decodeUnitType == MoonBridge.BUFFER_TYPE_PPS) {
            r.numPpsIn++;
            ppsBuffers.add(Arrays.copyOf(decodeUnitData, decodeUnitLength));
            return 1;
        } else if ((r.videoFormat & (MoonBridge.VIDEO_FORMAT_MASK_H264 | MoonBridge.VIDEO_FORMAT_MASK_H265)) != 0) {
            if (!submittedCsd || !fusedIdrFrame) {
                if (!fetchNextInputBuffer()) {
                    return 2;
                }
                putCsd();
                if (!queueNextInputBuffer(0, MediaCodec.BUFFER_FLAG_CODEC_CONFIG, 0, 0, 0, (char) 0, -1)) {
                    return 2;
                }
                csdSubmittedForThisFrame = true;
                submittedCsd = true;
                if (needsBaselineSpsHack) {
                    needsBaselineSpsHack = false;
                    if (!replaySps()) {
                        return 2;
                    }
                    LimeLog.info("SPS replay complete");
                }
            }
        }
        return 0;
    }

    /**
     * Prepares stats, fetches an input buffer, appends fused CSD if needed,
     * and computes the monotonic frame timestamp (or Long.MIN_VALUE on failure).
     */
    long prepareInputFrame(int decodeUnitLength, int frameType, long receiveTimeNs, long enqueueTimeUs) {
        r.activeWindowVideoStats.totalFramesReceived++;
        r.frameLatencyStats.onFrameReceived(receiveTimeNs);
        r.activeWindowVideoStats.totalFrames++;

        if (!fetchNextInputBuffer()) {
            return Long.MIN_VALUE;
        }

        int codecFlags = 0;
        if (frameType == MoonBridge.FRAME_TYPE_IDR) {
            codecFlags |= MediaCodec.BUFFER_FLAG_SYNC_FRAME;
            r.telemetry.largestKeyframeBytes = Math.max(r.telemetry.largestKeyframeBytes, decodeUnitLength);
            r.telemetry.inputBufferBytes = nextInputBuffer.limit();
            if (fusedIdrFrame && !csdSubmittedForThisFrame) {
                putCsd();
            }
        }
        currentCodecFlags = codecFlags;

        // The frame timestamps use an undefined epoch, so normalize them to uptime microseconds.
        if (baseTimestampUs == 0) {
            baseTimestampUs = (SystemClock.uptimeMillis() * 1000) - enqueueTimeUs;
        }
        long timestampUs = baseTimestampUs + enqueueTimeUs;
        if (timestampUs <= lastTimestampUs) {
            timestampUs = lastTimestampUs + 1;
        }
        lastTimestampUs = timestampUs;
        return timestampUs;
    }

    void logVividDropIfNeeded(String vividDecoder, int[] vividRange) {
        if (vividRange != null && !vividSeiDropLogged) {
            vividSeiDropLogged = true;
            LimeLog.warning("HDR Vivid: " + vividDecoder + " shows Vivid as SDR on displays without it;" +
                    " frames reach it without the host's Vivid SEI (" + (vividRange[1] - vividRange[0]) +
                    " bytes), with HDR10+ and HDR10");
        }
    }

    boolean ensureInputCapacity(int putLength) {
        if (putLength <= nextInputBuffer.limit() - nextInputBuffer.position()) {
            return true;
        }
        // The recovery drops this buffer and frame; the IDR asked for then fits.
        if (r.session.requestInputBufferGrowth((long) nextInputBuffer.position() + putLength, nextInputBuffer.limit())) {
            r.recovery.doCodecRecoveryIfRequired(MediaCodecDecoderRenderer.CR_FLAG_INPUT_THREAD);
            return false;
        }
        IllegalArgumentException exception = new IllegalArgumentException(
                "Decode unit length " + putLength + " too large for input buffer " + nextInputBuffer.limit() +
                " (requested " + r.requiredInputBufferSize + ")");
        r.telemetry.failDecoder(r.telemetry.decoderName() + " takes no frame this large: " + exception.getMessage(), exception);
        return false;
    }

    int finishQueueDecodeUnit(long timestampUs, int decodeUnitLength, int frameNumber, int frameType,
                              char frameHostProcessingLatency, long receiveTimeNs, long enqueueTimeNs,
                              long presentationTimeUs) {
        // Before queuing: the decoder may hand the frame back before queueInputBuffer returns.
        if (r.hostFrameTimes || r.session.trackContentRate) r.hostFrameClock.onQueued(timestampUs, presentationTimeUs);
        if (!queueNextInputBuffer(timestampUs, currentCodecFlags, frameNumber, receiveTimeNs, enqueueTimeNs,
                frameHostProcessingLatency, presentationTimeUs)) {
            return MoonBridge.DR_NEED_IDR;
        }
        KeyframeSafetyNet safetyNet = r.keyframeSafetyNet;
        if (safetyNet != null && frameType != MoonBridge.FRAME_TYPE_IDR) {
            safetyNet.onFrame(decodeUnitLength);
            if (safetyNet.due(r.lastVideoFrameTimeMs, r.streamBitrateKbps > 0 ? r.streamBitrateKbps : r.prefs.bitrate,
                    r.streamFrameRateMilliHz)) {
                return MoonBridge.DR_NEED_IDR;
            }
        }
        return MoonBridge.DR_OK;
    }

    boolean queueNextInputBuffer(long timestampUs, int codecFlags, int frameNumber, long receiveTimeNs,
                                 long enqueueTimeNs, char hostProcessingLatency, long hostRtpUs) {
        boolean codecRecovered;

        // Counted before queueing, as the decoder may hand the frame back before queueInputBuffer
        // returns and the renderer must never count an output before its input; taken back when
        // the queue fails, so a refused frame does not leave the depth waiting for its output.
        boolean counted = false;
        boolean queued = false;
        try {
            if ((codecFlags & MediaCodec.BUFFER_FLAG_CODEC_CONFIG) == 0) {
                r.frameLatencyStats.onDecoderInput(frameNumber, timestampUs, receiveTimeNs, enqueueTimeNs,
                        System.nanoTime(), hostProcessingLatency, hostRtpUs);
                r.numFramesIn++;
                counted = true;
            }
            r.videoDecoder.queueInputBuffer(nextInputBufferIndex,
                    0, nextInputBuffer.position(),
                    timestampUs, codecFlags);
            queued = true;
            if ((codecFlags & MediaCodec.BUFFER_FLAG_CODEC_CONFIG) == 0) {
                r.frameLatencyStats.onDecoderQueued(timestampUs, System.nanoTime());
            }

            nextInputBufferIndex = -1;
            nextInputBuffer = null;
        } catch (IllegalStateException e) {
            r.frameLatencyStats.discard(timestampUs, "input_failed");
            if (r.recovery.handleDecoderException(e)) {
                nextInputBuffer.clear();
            } else {
                nextInputBufferIndex = -1;
                nextInputBuffer = null;
            }
            return false;
        } finally {
            // Taken back before a recovery sets the depth base from the counts.
            if (counted && !queued) r.numFramesIn--;
            codecRecovered = r.recovery.doCodecRecoveryIfRequired(MediaCodecDecoderRenderer.CR_FLAG_INPUT_THREAD);
        }

        if (codecRecovered) {
            return false;
        }

        return fetchNextInputBuffer(true);
    }

    void holdForInputSpacing() {
        InputSpacer spacer = r.inputSpacer;
        KeyframeSafetyNet.Counts counts = r.repairCounts;
        if (spacer == null || counts == null) return;
        long nowNs = System.nanoTime();
        long holdNs = r.stopping || r.codecRecoveryType.get() != MediaCodecDecoderRenderer.CR_RECOVERY_TYPE_NONE ? 0 :
                spacer.holdNs(nowNs, r.streamFrameRateMilliHz);
        if (holdNs > 0) {
            try {
                Thread.sleep(holdNs / 1_000_000, (int) (holdNs % 1_000_000));
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
            }
            long heldNs = System.nanoTime() - nowNs;
            nowNs += heldNs;
            counts.heldFrames++;
            counts.heldNs += heldNs;
        }
        spacer.onInput(nowNs);
    }

    long preparePyroWaveFrame(int frameNumber, char frameHostProcessingLatency, long enqueueTimeNs,
                              long receiveTimeNs, long presentationTimeUs) {
        long inputNs = System.nanoTime();
        r.telemetry.pyroWaveQueueDelayTotal += Math.max(0, inputNs - enqueueTimeNs) / 1000000.0f;
        r.telemetry.pyroWaveQueueSamples++;
        long ptsUs = Math.max(inputNs / 1000, lastTimestampUs + 1);
        lastTimestampUs = ptsUs;
        r.frameLatencyStats.onPyroWaveInput(frameNumber, ptsUs, receiveTimeNs, enqueueTimeNs, inputNs,
                frameHostProcessingLatency, presentationTimeUs, inputNs);
        if (MoonBridge.getPendingVideoFrames() > 0) {
            r.activeWindowVideoStats.totalFrames++;
            r.activeWindowVideoStats.totalFramesReceived++;
            r.telemetry.pyroWaveSkippedFrames++;
            r.frameLatencyStats.discard(ptsUs, "replaced_before_decode");
            return -1;
        }
        return ptsUs;
    }

    void finishPyroWaveFrame(long ptsUs, long submissionResult) {
        r.activeWindowVideoStats.decoderTimeMs += r.telemetry.pyroWaveSubmissionTiming.addDurationNs(
                r.pyroWaveRenderer.getLastSubmissionDurationNs());
        float recordLoss = r.pyroWaveRenderer.getLastRecordLossPercent();
        if (recordLoss >= 0) {
            r.telemetry.pyroWaveLossTotal += recordLoss;
            r.telemetry.pyroWaveLossSamples++;
        }
        r.activeWindowVideoStats.totalFrames++;
        r.activeWindowVideoStats.totalFramesReceived++;
        if (submissionResult == PyroWaveDecoderRenderer.SUBMIT_ACCEPTED) {
            r.telemetry.pyroWaveDecodedFrames++;
            pyroWaveFailures = 0;
        } else if (submissionResult == PyroWaveDecoderRenderer.SUBMIT_BUSY) {
            // Native validated this frame before declining it for capacity.
            pyroWaveFailures = 0;
            r.telemetry.pyroWaveSkippedFrames++;
            r.frameLatencyStats.discard(ptsUs, "replaced_before_decode");
        } else {
            r.frameLatencyStats.discard(ptsUs, submissionResult < 0 ? "decode_failed" : "invalid_frame");
            if (submissionResult < 0 || ++pyroWaveFailures == 60) {
                int hdrFailure = (r.videoFormat & MoonBridge.VIDEO_FORMAT_MASK_10BIT) != 0 ?
                        PyroWaveDecoderRenderer.streamHdrFailure() : 0;
                LimeLog.severe("PyroWave decoder stopped producing frames" +
                        (hdrFailure != 0 ? ": " + r.context.getString(hdrFailure) : ""));
                r.codecSelector.pyroWaveHlgFramesStopped = true;
                // Game's end-of-stream text names HLG when the window took it (isPyroWaveHlgLost).
                r.codecSelector.isPyroWaveHlgLost();
                r.stopping = true;
                MoonBridge.bridgeClConnectionTerminated(MoonBridge.ML_ERROR_NO_VIDEO_FRAME);
            }
        }
    }

    private void doProfileSpecificSpsPatching(SeqParameterSet sps) {
        if (sps.profileIdc == 100 && constrainedHighProfile) {
            LimeLog.info("Setting constraint set flags for constrained high profile");
            sps.constraintSet4Flag = true;
            sps.constraintSet5Flag = true;
        } else if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) {
            sps.constraintSet4Flag = false;
            sps.constraintSet5Flag = false;
        }
    }

    /** The stream's H.264 SPS NAL unit (with its start code), rewritten for this decoder's quirks. */
    private byte[] patchH264Sps(byte[] decodeUnitData, int decodeUnitLength) {
        ByteBuffer spsBuf = ByteBuffer.wrap(decodeUnitData);
        int startSeqLen = decodeUnitData[2] == 0x01 ? 3 : 4;
        spsBuf.position(startSeqLen + 1);
        SeqParameterSet sps = H264Utils.readSPS(spsBuf);

        if (!r.refFrameInvalidationActive && Build.VERSION.SDK_INT < Build.VERSION_CODES.O) {
            if (r.initialWidth <= 720 && r.initialHeight <= 480 && r.refreshRate <= 60) {
                LimeLog.info("Patching level_idc to 31");
                sps.levelIdc = 31;
            } else if (r.initialWidth <= 1280 && r.initialHeight <= 720 && r.refreshRate <= 60) {
                LimeLog.info("Patching level_idc to 32");
                sps.levelIdc = 32;
            } else if (r.initialWidth <= 1920 && r.initialHeight <= 1080 && r.refreshRate <= 60) {
                LimeLog.info("Patching level_idc to 42");
                sps.levelIdc = 42;
            }
        }

        if (!r.refFrameInvalidationActive) {
            LimeLog.info("Patching num_ref_frames in SPS");
            sps.numRefFrames = 1;
        }

        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O &&
                sps.vuiParams != null &&
                r.hevcDecoder == null &&
                r.av1Decoder == null) {
            sps.vuiParams.videoSignalTypePresentFlag = false;
            sps.vuiParams.colourDescriptionPresentFlag = false;
            sps.vuiParams.chromaLocInfoPresentFlag = false;
        }

        if (needsSpsBitstreamFixup || isExynos4 || Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            if (sps.vuiParams == null) {
                LimeLog.info("Adding VUI parameters");
                sps.vuiParams = new VUIParameters();
            }
            if (sps.vuiParams.bitstreamRestriction == null) {
                LimeLog.info("Adding bitstream restrictions");
                sps.vuiParams.bitstreamRestriction = new VUIParameters.BitstreamRestriction();
                sps.vuiParams.bitstreamRestriction.motionVectorsOverPicBoundariesFlag = true;
                sps.vuiParams.bitstreamRestriction.maxBytesPerPicDenom = 2;
                sps.vuiParams.bitstreamRestriction.maxBitsPerMbDenom = 1;
                sps.vuiParams.bitstreamRestriction.log2MaxMvLengthHorizontal = 16;
                sps.vuiParams.bitstreamRestriction.log2MaxMvLengthVertical = 16;
                sps.vuiParams.bitstreamRestriction.numReorderFrames = 0;
            } else {
                LimeLog.info("Patching bitstream restrictions");
            }
            sps.vuiParams.bitstreamRestriction.maxDecFrameBuffering = sps.numRefFrames;
            if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) {
                sps.vuiParams.bitstreamRestriction.maxBytesPerPicDenom = 2;
                sps.vuiParams.bitstreamRestriction.maxBitsPerMbDenom = 1;
            }
        } else if (sps.vuiParams != null) {
            sps.vuiParams.bitstreamRestriction = null;
        }

        if (needsBaselineSpsHack) {
            LimeLog.info("Hacking SPS to baseline");
            sps.profileIdc = 66;
            savedSps = sps;
        }

        doProfileSpecificSpsPatching(sps);

        ByteBuffer escapedNalu = H264Utils.writeSPS(sps, decodeUnitLength);
        byte[] naluBuffer = new byte[startSeqLen + 1 + escapedNalu.limit()];
        System.arraycopy(decodeUnitData, 0, naluBuffer, 0, startSeqLen + 1);
        escapedNalu.get(naluBuffer, startSeqLen + 1, escapedNalu.limit());
        return naluBuffer;
    }

    /** Puts the IDR frame's parameter sets (VPS, SPS, PPS) into the input buffer, in that order. */
    private void putCsd() {
        for (byte[] vpsBuffer : vpsBuffers) {
            nextInputBuffer.put(vpsBuffer);
        }
        for (byte[] spsBuffer : spsBuffers) {
            nextInputBuffer.put(spsBuffer);
        }
        for (byte[] ppsBuffer : ppsBuffers) {
            nextInputBuffer.put(ppsBuffer);
        }
    }

    private boolean replaySps() {
        if (!fetchNextInputBuffer()) {
            return false;
        }
        nextInputBuffer.put(new byte[]{0x00, 0x00, 0x00, 0x01, 0x67});
        savedSps.profileIdc = 100;
        doProfileSpecificSpsPatching(savedSps);
        ByteBuffer escapedNalu = H264Utils.writeSPS(savedSps, 128);
        nextInputBuffer.put(escapedNalu);
        savedSps = null;
        return queueNextInputBuffer(0, MediaCodec.BUFFER_FLAG_CODEC_CONFIG, 0, 0, 0, (char) 0, -1);
    }
}
