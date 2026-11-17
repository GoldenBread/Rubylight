// The Sessions page's wording and search, kept out of the component so the tests can
// read them: `Sessions.test.mjs`.
import type { SessionHistoryEntry, StreamSession } from './api';
import { duration } from './format';

/** How a session ended, in the console's words, and how loudly to say it. */
const REASONS: Record<string, { label: string; tone: 'neutral' | 'warn' | 'danger' }> = {
  ended: { label: 'Ended', tone: 'neutral' },
  failed: { label: 'Failed', tone: 'danger' },
  start_failed: { label: 'Could not start', tone: 'warn' },
  interrupted: { label: 'Host stopped', tone: 'warn' },
};
export function reasonLabel(reason: string): string {
  return REASONS[reason]?.label ?? 'Ended';
}
export function reasonTone(reason: string): 'neutral' | 'warn' | 'danger' {
  return REASONS[reason]?.tone ?? 'neutral';
}

/** What kind of failure the log analysis recognised, when it did. */
const CATEGORIES: Record<string, string> = {
  encoder: 'Encoder',
  capture: 'Capture',
  network: 'Network',
  driver: 'Driver',
};
export function categoryLabel(category: string | null | undefined): string {
  return category ? (CATEGORIES[category] ?? category) : '';
}

const CODECS: Record<string, string> = {
  h264: 'H.264',
  hevc: 'HEVC',
  av1: 'AV1',
  pyrowave: 'PyroWave',
};
export function codecName(codec: string): string {
  return CODECS[codec] ?? (codec ? codec.toUpperCase() : 'Unknown video format');
}

/** The search box: a device, a game, a codec or a reason, in any case. */
export function matches(entry: SessionHistoryEntry, query: string): boolean {
  const words = query.trim().toLowerCase();
  if (!words) return true;
  return [
    entry.device_name,
    entry.app,
    entry.codec,
    reasonLabel(entry.end_reason),
    categoryLabel(entry.end_category),
  ].some((value) => value.toLowerCase().includes(words));
}

/** What the stream ran: codec, picture size and rate, HDR. */
export function pictureText(entry: SessionHistoryEntry): string {
  return [
    codecName(entry.codec),
    entry.width && entry.height ? `${entry.width}×${entry.height}` : '',
    entry.hdr ? 'HDR' : '',
    entry.fps ? `${entry.fps} fps` : '',
  ]
    .filter(Boolean)
    .join(' · ');
}

export function fpsText(value: number): string {
  return value > 0 ? `${value.toFixed(1)} fps` : '–';
}

/** The p95 figure; a dash until the host has 20 sampled seconds to take 5% of. */
export function p95Text(value: number): string {
  return value > 0 ? value.toFixed(1) : '–';
}

/** What the stream sent on average, else the encoder's last rate. */
export function bitrateText(entry: SessionHistoryEntry): string {
  const kbps = entry.mean_bitrate_kbps > 0 ? entry.mean_bitrate_kbps : entry.bitrate_kbps;
  return kbps > 0 ? `${(kbps / 1000).toFixed(1)} Mbps` : '–';
}

/** The client's reported loss; a dash when it reported nothing, never 0%. */
export function lossText(entry: SessionHistoryEntry): string {
  const value = entry.loss_percent;
  if (value == null) return '–';
  return `${value.toFixed(value > 0 && value < 1 ? 2 : 1)}%`;
}

export function durationText(entry: SessionHistoryEntry): string {
  return entry.duration_ms > 0 ? duration(Math.round(entry.duration_ms / 1000)) : '–';
}

/**
 * Whether a stream the page listed is gone: its entry is in the history now.
 * The count also catches one of two streams sharing a device's uuid leaving.
 */
export function aStreamEnded(before: StreamSession[] | null, after: StreamSession[]): boolean {
  if (!before?.length) return false;
  if (after.length < before.length) return true;
  const running = new Set(after.map((stream) => stream.uuid));
  return before.some((stream) => !running.has(stream.uuid));
}
