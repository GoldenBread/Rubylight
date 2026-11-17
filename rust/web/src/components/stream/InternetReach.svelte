<script lang="ts">
  // The invite dialog's "On the internet" (docs/browser-guests.md): what the router did with the
  // guest ports, the public address, and plainly what works instead when nothing reaches the PC.
  import Badge from '../Badge.svelte';
  import type { GuestReach } from '../../lib/api';
  import { portLines, reachAlternatives, reachVerdict } from '../../lib/guests';

  let { reach, ports }: { reach: GuestReach | null; ports: string } = $props();

  const verdict = $derived(reachVerdict(reach));
  const lines = $derived(reach ? portLines(reach.ports) : []);
  const blocked = $derived(!!reach && !reach.checking && !reach.address);
</script>

<div class="reach" aria-live="polite">
  <p class="verdict"><Badge tone={verdict.tone}>{verdict.tone === 'ok' ? 'Reachable' : verdict.tone === 'warn' ? 'Not reachable' : 'Checking'}</Badge> {verdict.text}</p>
  {#if reach && !reach.checking}
    <dl class="facts">
      {#if reach.method}
        <dt>Router</dt>
        <dd>Answered {reach.method}</dd>
      {/if}
      {#each lines as line (line.label)}
        <dt>{line.label}</dt>
        <dd><Badge tone={line.ok && line.detail === 'forwarded' ? 'ok' : 'warn'}>{line.ok ? 'Forwarded' : 'Not forwarded'}</Badge> <span class="muted">{line.detail === 'forwarded' ? '' : line.detail}</span></dd>
      {/each}
      {#if reach.error}
        <dt>Port forwarding</dt>
        <dd class="muted">{reach.error}</dd>
      {/if}
      <dt>Public address</dt>
      <dd>{reach.publicIpv4 ?? (reach.routerIpv4 ? `${reach.routerIpv4} (not public)` : 'Not known')}</dd>
      {#if reach.ipv6}
        <dt>IPv6</dt>
        <dd>{reach.ipv6}</dd>
      {/if}
    </dl>
    {#each reach.notes as note (note)}
      <p class="muted note">{note}</p>
    {/each}
    {#if blocked}
      <ul class="instead">
        {#each reachAlternatives(reach, ports) as line (line)}
          <li>{line}</li>
        {/each}
      </ul>
    {/if}
  {/if}
  <p class="muted note">
    Rubylight forwards only {ports} on your router, while an internet link works or a guest watches, and removes them
    after; also with automatic port forwarding off. Guests' browsers warn that the connection is not private, since the PC's
    certificate is its own: they continue past the warning to join.
  </p>
</div>

<style>
  .reach {
    display: grid;
    gap: var(--space-2);
  }
  .reach p {
    margin: 0;
  }
  .verdict {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-2);
    align-items: baseline;
  }
  .facts {
    display: grid;
    grid-template-columns: max-content 1fr;
    gap: var(--space-1) var(--space-3);
    margin: 0;
    font-size: var(--text-sm);
  }
  .facts dt {
    font-weight: var(--weight-semibold);
  }
  .facts dd {
    margin: 0;
    overflow-wrap: anywhere;
  }
  .note {
    font-size: var(--text-sm);
  }
  .instead {
    margin: 0;
    padding-left: var(--space-4);
    font-size: var(--text-sm);
  }
</style>
