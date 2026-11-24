<script lang="ts">
  import type { Setting } from '../../lib/settings-types';

  let {
    description,
    problem,
    handSet,
  }: {
    description: string;
    problem: string | null;
    handSet: Setting[];
  } = $props();
</script>

{#if problem}
  <p class="preset-note preset-error" role="alert" data-preset-error><strong>Not a preset.</strong> {problem}</p>
{/if}
<p class="preset-note" data-preset-note>
  {description}
  {#if handSet.length > 0}
    <strong>Custom:</strong> set by hand, so they win over the preset: {handSet.map((item) => item.label).join(', ')}.
  {/if}
</p>

<style>
  .preset-note {
    margin: 0;
    padding: 0 var(--space-5) var(--space-3);
    color: var(--muted);
    font-size: var(--text-sm);
  }
  .preset-note strong {
    color: var(--ink);
  }
  .preset-error,
  .preset-error strong {
    color: var(--danger);
  }
</style>
