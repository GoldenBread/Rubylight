<script lang="ts">
  import type { Snippet } from 'svelte';
  import Button from '../Button.svelte';
  import EmptyState from '../EmptyState.svelte';
  import Panel from '../Panel.svelte';
  import type { Setting } from '../../lib/settings-types';
  import type { TabSearchResult } from './settings-page';

  let {
    review,
    listingCustomized,
    pendingCount,
    invalidCount,
    resultCount,
    searching,
    query,
    emptyTitle,
    emptyMessage,
    results,
    isVisible,
    onclose,
    onopentab,
    row,
    otherKeys,
  }: {
    review: 'all' | 'changed' | 'invalid';
    listingCustomized: boolean;
    pendingCount: number;
    invalidCount: number;
    resultCount: number;
    searching: boolean;
    query: string;
    emptyTitle: string;
    emptyMessage: string;
    results: TabSearchResult[];
    isVisible: (setting: Setting) => boolean;
    onclose: () => void;
    onopentab: (id: string) => void;
    row: Snippet<[Setting, boolean]>;
    otherKeys: Snippet<[string[], boolean]>;
  } = $props();
</script>

<div class="results-heading">
  <div>
    <h2>{review === 'invalid' ? 'Review errors' : review === 'changed' ? 'Review changes' : listingCustomized ? 'Customized settings' : 'Search results'}</h2>
    <p class="muted" role="status">
      {#if review !== 'all' && pendingCount === 0}
        No unsaved changes.
      {:else if review === 'invalid' && invalidCount === 0}
        All errors corrected. Review your values, then save your changes.
      {:else}
        {resultCount} {resultCount === 1 ? 'setting' : 'settings'}{searching ? ` matching “${query.trim()}”` : ' across all tabs'}
      {/if}
    </p>
  </div>
  <Button size="sm" variant="ghost" onclick={onclose}>All settings</Button>
</div>
{#if results.length === 0}
  <Panel>
    <EmptyState icon="search" title={emptyTitle}>
      <p>{emptyMessage}</p>
    </EmptyState>
  </Panel>
{:else}
  {#each results as result (result.tab.id)}
    <Panel title={result.tab.label} flush>
      {#snippet actions()}
        <Button size="sm" variant="ghost" onclick={() => onopentab(result.tab.id)}>
          Open<span class="visually-hidden">{` ${result.tab.label}`}</span>
        </Button>
      {/snippet}
      <div class="rows">
        {#each result.settings as setting (setting)}
          {@render row(setting, !isVisible(setting))}
        {/each}
      </div>
      {#if result.unknown.length}
        {@render otherKeys(result.unknown, false)}
      {/if}
    </Panel>
  {/each}
{/if}

<style>
  .results-heading {
    display: flex;
    justify-content: space-between;
    align-items: center;
    flex-wrap: wrap;
    gap: var(--space-2);
  }
  .results-heading h2 {
    font-size: var(--text-lg);
  }
  .results-heading p {
    font-size: var(--text-sm);
  }
  .rows {
    display: grid;
  }
  .rows > :global(.setting + .setting) {
    border-top: 1px solid var(--line);
  }
</style>
