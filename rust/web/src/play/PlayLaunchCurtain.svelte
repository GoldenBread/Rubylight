<script lang="ts">
  /**
   * The play page's launch curtain: shown while the host starts the game and negotiates the
   * WebRTC picture, and as a compact solid status pill while quitting. Solid media tokens only,
   * never `backdrop-filter`.
   */
  import BrandMark from '../components/BrandMark.svelte';

  let {
    title,
    host = '',
    picture = '',
    status = '',
    curtain = true,
  }: {
    title: string;
    host?: string;
    picture?: string;
    status?: string;
    /** Full launch curtain before the first frame arrives; compact status pill otherwise. */
    curtain?: boolean;
  } = $props();
</script>

{#if curtain}
  <div class="curtain">
    <div class="card">
      <div class="brand">
        <BrandMark size={48} />
        <span>{host || 'Rubylight'}</span>
      </div>
      <h2>{title || 'Starting game'}</h2>
      {#if picture}<p class="spec">{picture}</p>{/if}
      <p class="pill" role="status">
        <span class="pulse" aria-hidden="true"></span>
        <span>{status || 'Starting…'}</span>
      </p>
    </div>
  </div>
{:else if status}
  <p class="pill floating" role="status">
    <span class="pulse" aria-hidden="true"></span>
    <span>{status}</span>
  </p>
{/if}

<style>
  .curtain {
    position: absolute;
    inset: 0;
    z-index: var(--z-raised);
    display: grid;
    place-items: center;
    padding: 5vh 5vw;
    box-sizing: border-box;
    background: var(--media-glow) no-repeat, var(--media-bg);
    color: var(--media-ink);
  }
  .card {
    display: grid;
    justify-items: center;
    gap: 0.7em;
    max-width: min(32em, 92vw);
    padding: 1.8em 2.2em;
    border: 1px solid var(--media-line-strong);
    border-radius: var(--radius-xl);
    background: var(--media-panel);
    box-shadow: var(--media-edge), var(--media-shadow);
    text-align: center;
  }
  .brand {
    display: inline-flex;
    align-items: center;
    gap: 0.55em;
    color: var(--media-ink-2);
    font-size: 0.75em;
    font-weight: var(--weight-semibold);
    letter-spacing: var(--tracking-label);
    text-transform: uppercase;
  }
  h2 {
    margin: 0;
    font-family: var(--font-display);
    font-size: clamp(1.4em, 3vw, 2em);
    font-weight: var(--weight-bold);
    letter-spacing: var(--tracking-display);
    line-height: var(--leading-tight);
    overflow-wrap: anywhere;
  }
  .spec {
    margin: 0;
    color: var(--media-idle);
    font-size: 0.75em;
    font-variant-numeric: tabular-nums;
  }
  .pill {
    display: inline-flex;
    align-items: center;
    gap: 0.65em;
    margin: 0.2em 0 0;
    padding: 0.45em 1em;
    border: 1px solid var(--media-line-strong);
    border-radius: var(--radius-pill);
    background: var(--media-panel-raised);
    color: var(--media-ink);
    font-size: 0.8em;
    font-weight: var(--weight-medium);
  }
  .pill.floating {
    position: absolute;
    top: 45%;
    inset-inline-start: 50%;
    z-index: var(--z-raised);
    margin: 0;
    box-shadow: var(--media-shadow);
    transform: translateX(-50%);
  }
  .pulse {
    flex: none;
    width: 0.6em;
    height: 0.6em;
    border-radius: var(--radius-pill);
    background: var(--media-accent-text);
    animation: breathe var(--pulse-duration) var(--ease-standard) infinite alternate;
  }
  @keyframes breathe {
    to {
      opacity: 0.3;
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .pulse {
      animation: none;
    }
  }
</style>
