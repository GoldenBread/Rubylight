<script lang="ts">
  /**
   * The live command stage: what streams now, how it is doing and what to do about it, on one
   * cover-lit media plane. Home and Sessions show it whenever a device streams. It leads with the
   * first stream's health, traces and pipeline; every stream has its row below, with its own
   * Disconnect and its full figures.
   */
  import type { Snippet } from 'svelte';
  import Button from '../Button.svelte';
  import Icon from '../Icon.svelte';
  import AppCover from '../library/AppCover.svelte';
  import { coverPath, coverVersion, placeholder } from '../library/app';
  import PipelineTimeline from './PipelineTimeline.svelte';
  import StreamCard from './StreamCard.svelte';
  import SuspendButton from '../session/SuspendButton.svelte';
  import TelemetrySparklines from './TelemetrySparklines.svelte';
  import { diagnosticsText, leadStream, pipelinePhases, specPills, stageTitle, traces, worstHealth } from './studio-telemetry';
  import { api, type App, type QuickSuspendStatus, type SessionStatus, type StreamSession, type StutterStageId } from '../../lib/api';
  import { confirm, failed, notify } from '../../lib/feedback.svelte';
  import { duration, ms } from '../../lib/format';
  import { link } from '../../lib/router.svelte';

  interface Props {
    /** The streams to devices, the first one leading. Never empty. */
    streams: StreamSession[];
    /** The game that plays, when the host knows it. */
    game?: { name: string; app?: App | null } | null;
    /** The running app, for Quit game; without it the stage offers no quit. */
    status?: SessionStatus | null;
    /** Quick Suspend's state: Suspend game shows when the host can pause the game now. */
    quick?: QuickSuspendStatus | null;
    /** The last poll went unanswered: the figures stay, marked as old. */
    stale?: boolean;
    /** The region's name. */
    label?: string;
    /** uuid of the device being disconnected. */
    disconnecting?: string | null;
    ondisconnect: (stream: StreamSession) => void;
    /** After the game was quit. */
    onchange?: () => void;
    /** The stage's last row: what the page adds under the streams. */
    children?: Snippet;
  }
  let { streams, game = null, status = null, quick = null, stale = false, label = 'Live stream', disconnecting = null, ondisconnect, onchange, children }: Props = $props();

  const lead = $derived(leadStream(streams) as StreamSession);
  const health = $derived(worstHealth(streams));
  const measured = $derived(lead.performance.sample_frames > 0);
  const phases = $derived(pipelinePhases(lead.performance.stages, measured));
  const pills = $derived(specPills(lead));
  const title = $derived(stageTitle(streams, game?.name ?? null));
  const cover = $derived.by(() => {
    if (!game) return null;
    const path = game.app ? coverPath(game.app) : '';
    const uuid = game.app?.uuid;
    return { image: uuid && path ? api.apps.coverUrl(uuid, coverVersion(path)) : '', hue: placeholder(game.name).hue, path, uuid };
  });
  const appName = $derived(status?.app?.name || status?.appName || game?.name || 'the game');
  const single = $derived(streams.length === 1 ? lead : null);
  const presentToSent = $derived(measured && lead.performance.stages && lead.performance.stages.frames > 0 ? lead.performance.stages.present_to_sent : null);
  const stutter = $derived(lead.stutter_recent ?? lead.stutter ?? null);
  const lostStage = $derived(stutter?.stages.find((stage) => stage.id === stutter?.dominant) ?? null);
  const pipeline = $derived(lead.pipeline_rates ?? null);
  const timing = (value: number) => (measured ? ms(value) : '–');
  const hz = (mhz: number | null) => (mhz == null ? '–' : (mhz / 1000).toFixed(mhz < 100_000 ? 1 : 0));
  const stageWords: Record<StutterStageId, string> = {
    capture: 'capture',
    convert: 'conversion',
    encode: 'encoding',
    packetize: 'packetizing',
    send: 'sending',
  };

  let rows = $state<HTMLElement>();
  let detailsOpen = $state(false);
  let quitting = $state(false);

  /** Toggles the inline breakdown for a single stream, or opens the first device row's figures. */
  function showDetails() {
    if (single) {
      detailsOpen = !detailsOpen;
      return;
    }
    const details = rows?.querySelector('details');
    if (!details) return;
    details.open = true;
    details.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
    details.querySelector('summary')?.focus({ preventScroll: true });
  }

  async function copyDiagnostics() {
    const text = streams.map((stream) => diagnosticsText(stream, game?.name ?? null)).join('\n\n');
    try {
      await navigator.clipboard.writeText(text);
      notify('Copied the stream’s figures.', 'ok');
    } catch (failure) {
      failed('Copying the figures failed', failure);
    }
  }

  async function quit() {
    const app = appName;
    const ok = await confirm({
      title: `Quit ${app}?`,
      message: 'It closes on this PC and every stream ends. Unsaved progress may be lost.',
      confirm: 'Quit game',
      danger: true,
    });
    if (!ok) return;
    quitting = true;
    try {
      await api.apps.close();
      notify(`Quit ${app}.`, 'ok');
      onchange?.();
    } catch (failure) {
      failed('Quitting the game failed', failure);
    } finally {
      quitting = false;
    }
  }
</script>

<section class="stage" aria-label={label}>
  <div class="backdrop" class:tint={cover && !cover.image} style:--hue={cover?.hue ?? 350}
    style:background-image={cover?.image ? `url("${cover.image}")` : undefined} aria-hidden="true"></div>
  <div class="main" class:bare={!(game && cover)}>
    <div class="top">
      {#if health}
        <p class="health {health.level}" role="status">
          <span class="dot" aria-hidden="true"></span>
          <strong>{health.label}</strong>
          <span class="cause">{health.cause}</span>
          {#if health.level !== 'healthy'}<a class="troubleshoot" href="/troubleshoot" use:link>Troubleshoot</a>{/if}
        </p>
      {/if}
      {#if stale}<p class="old" role="status"><Icon name="alert" size={16} />Waiting for this PC: these figures are a few seconds old.</p>{/if}
    </div>
    <div class="what">
      <p class="eyebrow">Live now{#if single?.uptime_seconds}{' · '}<span class="num">{duration(single.uptime_seconds)}</span>{/if}</p>
      <h2>{title}</h2>
      <ul class="pills" aria-label="Stream format">
        {#each pills as pill (pill)}<li class="num">{pill}</li>{/each}
      </ul>
    </div>
    {#if game && cover}
      <a class="cover" href={cover.uuid ? `/library/${encodeURIComponent(cover.uuid)}` : '/library'} use:link aria-label="{game.name}: game page">
        <AppCover name={game.name} uuid={cover.uuid} path={cover.path} />
      </a>
    {/if}
  </div>

  <div class="figures">
    <TelemetrySparklines traces={traces(lead)} {stale} />
    <PipelineTimeline {phases} />
  </div>

  <div class="deck">
    <Button icon="info" onclick={showDetails}>Stream details</Button>
    <Button icon="copy" onclick={copyDiagnostics}>Copy diagnostics</Button>
    {#if single || status?.appRunning}
      <span class="end">
        {#if single}
          <Button busy={disconnecting === single.uuid} disabled={single.state === 'STOPPING'} onclick={() => ondisconnect(single)}>
            Disconnect<span class="visually-hidden"> {single.device_name}</span>
          </Button>
        {/if}
        {#if status?.appRunning}
          <SuspendButton {quick} {onchange} /><Button variant="danger" icon="power" busy={quitting} onclick={quit}>Quit game</Button>
        {/if}
      </span>
    {/if}
  </div>

  {#if single && detailsOpen}
    <div class="drawer" role="region" aria-label="Stream details">
      <dl class="metrics">
        <div>
          <dt>Frame rate</dt>
          <dd class="value num">{measured ? `${Math.round(single.performance.fps)} fps` : '–'}</dd>
          <dd class="sub">
            Target <span class="num">{single.fps}</span>
            {#if single.requested_fps && single.requested_fps !== single.fps}
              (asked for <span class="num">{single.requested_fps}</span>)
            {/if}
          </dd>
        </div>
        <div>
          <dt>Encoding (p95)</dt>
          <dd class="value num">{timing(single.performance.encode_p95_ms)}</dd>
          <dd class="sub">Average <span class="num">{timing(single.performance.encode_mean_ms)}</span></dd>
        </div>
        <div>
          <dt>Host processing</dt>
          <dd class="value num">{timing(single.performance.host_processing_mean_ms)}</dd>
          <dd class="sub">99% below <span class="num">{timing(single.performance.host_processing_p99_ms)}</span></dd>
        </div>
        <div>
          <dt>Present to sent</dt>
          <dd class="value num">{presentToSent ? ms(presentToSent.mean_ms) : timing(single.performance.frame_age_mean_ms)}</dd>
          <dd class="sub">
            {#if presentToSent}
              99% below <span class="num">{ms(presentToSent.p99_ms)}</span>
            {:else}
              Average frame age
            {/if}
          </dd>
        </div>
        {#if stutter && stutter.pictures > 0}
          <div class="whole">
            <dt>Where pictures lost time</dt>
            <dd class="value">{lostStage ? `${lostStage.percent}% in ${stageWords[lostStage.id]}` : 'No dominant stage'}</dd>
            <dd class="sub">
              {#if stutter.late_pictures > 0}
                <span class="num">{stutter.late_pictures}</span> of <span class="num">{stutter.pictures}</span> new pictures lost <span class="num">{ms(stutter.lost_ms)}</span>{#if lostStage}, {lostStage.percent}% in {stageWords[lostStage.id]}{/if}.
              {:else}
                No new picture took longer than this stream’s usual path on this PC.
              {/if}
            </dd>
          </div>
        {/if}
        {#if pipeline}
          <div class="whole">
            <dt>Pipeline frame rates</dt>
            <dd class="value num">{hz(pipeline.source_mhz)} → {hz(pipeline.capture_mhz)} → {hz(pipeline.submit_mhz)} → {hz(pipeline.output_mhz)} fps</dd>
            <dd class="sub">Display → capture → encode → send over <span class="num">{(pipeline.window_ms / 1000).toFixed(1)}</span> s (<span class="num">{hz(pipeline.repeat_mhz)}</span> fps repeats).</dd>
          </div>
        {/if}
      </dl>
    </div>
  {/if}

  {#if streams.length > 1}
    <div class="rows" bind:this={rows}>
      {#each streams as stream (stream.uuid)}
        <StreamCard {stream} busy={disconnecting === stream.uuid} {ondisconnect} />
      {/each}
    </div>
  {/if}
  {@render children?.()}
</section>

<style>
  /* A media plane: dark in both themes. The theme's own tokens are pointed at the media palette
     here, so buttons, badges and the stream rows inside draw for this plane without a variant. */
  .stage {
    --panel: var(--media-panel-raised);
    --sunken: var(--media-panel);
    --field: var(--media-panel);
    --ink: var(--media-ink);
    --ink-2: var(--media-ink-2);
    --muted: var(--media-idle);
    --line: var(--media-line);
    --line-strong: var(--media-line-strong);
    --focus: var(--media-focus);
    --edge: var(--media-edge);
    --ok: var(--media-ok);
    --ok-soft: color-mix(in srgb, var(--media-ok) 14%, var(--media-panel));
    --ok-line: color-mix(in srgb, var(--media-ok) 36%, var(--media-panel));
    --warn: var(--media-warn);
    --warn-soft: color-mix(in srgb, var(--media-warn) 14%, var(--media-panel));
    --warn-line: color-mix(in srgb, var(--media-warn) 36%, var(--media-panel));
    --danger: var(--media-danger);
    --danger-soft: color-mix(in srgb, var(--media-danger) 14%, var(--media-panel));
    --danger-line: color-mix(in srgb, var(--media-danger) 40%, var(--media-panel));
    --accent-text: var(--media-accent-text);
    position: relative;
    isolation: isolate;
    display: grid;
    overflow: hidden;
    border: 1px solid var(--media-line);
    border-radius: var(--radius-xl);
    background: var(--media-bg);
    box-shadow: var(--media-edge), var(--shadow);
    color: var(--media-ink);
  }
  /* The game's cover as light: blurred to its colours, held under the wash so text keeps its contrast. */
  .backdrop {
    position: absolute;
    inset: -48px;
    z-index: var(--z-below);
    background-color: var(--media-bg);
    background-image: var(--media-glow);
    background-size: cover;
    background-position: center;
    filter: blur(36px) saturate(1.25);
    opacity: 0.85;
  }
  .backdrop.tint {
    background-image:
      radial-gradient(130% 90% at 85% 10%, oklch(from var(--media-bg) calc(l + 0.26) 0.09 var(--hue) / 0.85), transparent 70%),
      linear-gradient(165deg, oklch(from var(--media-bg) calc(l + 0.14) 0.06 var(--hue)), oklch(from var(--media-bg) calc(l + 0.02) 0.03 var(--hue)));
  }
  .stage::before {
    content: '';
    position: absolute;
    inset: 0;
    z-index: var(--z-below);
    background: var(--media-wash);
  }
  .main,
  .figures,
  .deck {
    padding-inline: var(--space-6);
  }
  /* Health sits level with the cover's top, the title with its foot: no empty band between them. */
  .top {
    grid-area: top;
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    align-self: start;
    gap: var(--space-2) var(--space-4);
  }
  /* Stream health: a word in its colour and one sentence. */
  .health {
    display: inline-flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-1) var(--space-2);
    min-width: 0;
    padding: var(--space-2) var(--space-4) var(--space-2) var(--space-3);
    border: 1px solid var(--media-line);
    border-radius: var(--radius-pill);
    background: var(--media-scrim);
    font-size: var(--text-sm);
    line-height: var(--leading-snug);
  }
  .health .dot {
    width: 10px;
    height: 10px;
    flex: none;
    border-radius: 50%;
    background: var(--tone);
    box-shadow: var(--halo);
  }
  .health strong {
    color: var(--tone);
    font-weight: var(--weight-semibold);
  }
  .health .cause {
    color: var(--media-ink-2);
  }
  .health .troubleshoot {
    margin-inline-start: var(--space-1);
    color: var(--media-ink);
    font-weight: var(--weight-semibold);
    text-decoration: underline;
    text-underline-offset: 3px;
  }
  .health .troubleshoot:hover {
    color: var(--tone);
  }
  .healthy {
    --tone: var(--media-ok);
    --halo: var(--media-halo-ok);
  }
  .attention {
    --tone: var(--media-warn);
    --halo: var(--media-halo-warn);
  }
  .action {
    --tone: var(--media-danger);
    --halo: var(--media-halo-danger);
    border-color: var(--danger-line);
  }
  .healthy .dot {
    animation: beat var(--pulse-duration) ease-in-out infinite;
  }
  .old {
    display: inline-flex;
    align-items: center;
    gap: var(--space-2);
    color: var(--media-warn);
    font-size: var(--text-sm);
  }
  .main {
    display: grid;
    grid-template: 'top cover' auto 'what cover' 1fr / minmax(0, 1fr) auto;
    gap: var(--space-5) var(--space-6);
    padding-block: var(--space-5);
  }
  .main.bare {
    grid-template: 'top' auto 'what' auto / minmax(0, 1fr);
  }
  .what {
    grid-area: what;
    display: grid;
    align-self: end;
    gap: var(--space-2);
    min-width: 0;
  }
  .eyebrow {
    color: var(--media-ink-2);
  }
  h2 {
    max-width: 22ch;
    font-size: clamp(var(--text-xl), 3.2vw, var(--text-2xl));
    letter-spacing: var(--tracking-display);
    text-wrap: balance;
  }
  .pills {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-2);
    margin: var(--space-2) 0 0;
    padding: 0;
    list-style: none;
  }
  .pills li {
    padding: 3px var(--space-3);
    border: 1px solid var(--media-line-strong);
    border-radius: var(--radius-pill);
    background: var(--media-scrim);
    color: var(--media-ink);
    font-size: var(--text-xs);
    font-weight: var(--weight-medium);
    line-height: var(--leading);
    white-space: nowrap;
  }
  .cover {
    grid-area: cover;
    display: block;
    align-self: end;
    width: 132px;
    border-radius: var(--radius);
    box-shadow: var(--media-shadow);
    transition: transform var(--duration-fast) var(--ease-standard);
  }
  .cover:hover {
    transform: translateY(-2px);
  }
  .figures {
    display: grid;
    gap: var(--space-4);
    padding-block: 0 var(--space-5);
  }
  /* The action deck: calm, each button says what it does. Quit, the one that ends the game, stands apart. */
  .deck {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-2);
    padding-block: var(--space-3);
    border-top: 1px solid var(--media-line);
    background: var(--media-scrim);
  }
  .end {
    display: inline-flex;
    gap: var(--space-2);
    flex-wrap: wrap;
    margin-inline-start: auto;
  }
  .rows {
    display: grid;
    border-top: 1px solid var(--media-line);
    background: var(--media-panel);
  }
  .rows > :global(* + *) {
    border-top: 1px solid var(--media-line);
  }
  .drawer {
    padding: var(--space-4) var(--space-6);
    border-top: 1px solid var(--media-line);
    background: var(--media-panel);
    container-type: inline-size;
  }
  .metrics {
    display: grid;
    grid-template-columns: repeat(4, minmax(0, 1fr));
    margin: 0;
    border: 1px solid var(--media-line);
    border-radius: var(--radius);
    overflow: hidden;
    background: var(--media-panel-raised);
  }
  .metrics .whole {
    grid-column: 1 / -1;
  }
  .metrics div {
    padding: var(--space-2) var(--space-3);
    box-shadow: var(--rule-grid);
  }
  .metrics dt {
    font-size: var(--text-xs);
    color: var(--media-idle);
  }
  .metrics dd {
    margin: 0;
  }
  .metrics .value {
    font-size: var(--text-lg);
    line-height: var(--leading-snug);
  }
  .metrics .sub {
    font-size: var(--text-xs);
    color: var(--media-ink-2);
  }
  @container (max-width: 520px) {
    .metrics {
      grid-template-columns: repeat(2, minmax(0, 1fr));
    }
  }
  @keyframes beat {
    50% {
      opacity: 0.45;
    }
  }
  @media (max-width: 700px) {
    .main,
    .figures,
    .deck,
    .drawer {
      padding-inline: var(--space-4);
    }
    .health {
      border-radius: var(--radius);
    }
    .main {
      grid-template: 'top top' auto 'what cover' auto / minmax(0, 1fr) 72px;
      gap: var(--space-4);
      padding-block: var(--space-4);
    }
    .what {
      align-self: start;
    }
    .cover {
      align-self: start;
      width: 72px;
    }
    .deck :global(.button) {
      flex: 1 1 auto;
    }
    .end {
      flex: 1 1 100%;
      margin-inline-start: 0;
    }
    .end :global(.button) {
      width: 100%;
    }
  }
</style>
