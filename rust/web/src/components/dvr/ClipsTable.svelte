<script lang="ts">
  /**
   * The saved clips, newest first: game, when, length, picture (codec, HDR), size and kind, with
   * Preview, Download and Delete on each row and Show more for the next page. On a phone each row
   * stacks into a card so its actions stay in view. The page owns the list and the actions; this
   * part lays them out.
   */
  import Badge from '../Badge.svelte';
  import Button from '../Button.svelte';
  import Table from '../Table.svelte';
  import { clipDownloadUrl, type ClipSummary } from '../../lib/api';
  import { clipLength, clipName, codecLabel, kindLabel, pictureText, savedAt, size } from './dvr-view';

  let {
    clips,
    deleting = null,
    more = false,
    loadingMore = false,
    onpreview,
    ondelete,
    onmore,
  }: {
    clips: ClipSummary[];
    /** The id of the clip being deleted. */
    deleting?: string | null;
    /** The host has older clips past this page. */
    more?: boolean;
    loadingMore?: boolean;
    onpreview: (clip: ClipSummary) => void;
    ondelete: (clip: ClipSummary) => void;
    onmore: () => void;
  } = $props();
</script>

<div class="clips-table">
<Table caption="Saved clips, newest first">
  <thead>
    <tr>
      <th>Clip</th>
      <th>Saved</th>
      <th class="numeric">Length</th>
      <th>Picture</th>
      <th class="numeric">Size</th>
      <th><span class="visually-hidden">Actions</span></th>
    </tr>
  </thead>
  <tbody>
    {#each clips as clip (clip.id)}
      <tr>
        <td class="clip">
          <span class="title">{clipName(clip)}</span>
          <span class="tags">
            <Badge tone={clip.kind === 'manual' ? 'accent' : 'neutral'}>{kindLabel(clip.kind)}</Badge>
            {#if clip.recovered}<Badge tone="warn">Recovered</Badge>{/if}
          </span>
        </td>
        <td class="when" data-label="Saved">{savedAt(clip.created_unix_ms)}</td>
        <td class="numeric num length" data-label="Length">{clipLength(clip.duration_ms)}</td>
        <td class="picture" data-label="Picture">
          <span class="tags">
            <Badge>{codecLabel(clip.codec)}</Badge>
            {#if clip.hdr}<Badge tone="accent">HDR</Badge>{/if}
          </span>
          <span class="sub num">{pictureText(clip)}</span>
        </td>
        <td class="numeric num size" data-label="Size">{size(clip.size_bytes)}</td>
        <td class="row-actions">
          <Button size="sm" variant="ghost" icon="play" onclick={() => onpreview(clip)}>
            Preview<span class="visually-hidden">{` ${clipName(clip)}`}</span>
          </Button>
          <Button size="sm" variant="ghost" icon="download" href={clipDownloadUrl(clip.id)}>
            Download<span class="visually-hidden">{` ${clipName(clip)}`}</span>
          </Button>
          <Button size="sm" variant="ghost" icon="trash" busy={deleting === clip.id} onclick={() => ondelete(clip)}>
            Delete<span class="visually-hidden">{` ${clipName(clip)}`}</span>
          </Button>
        </td>
      </tr>
    {/each}
  </tbody>
</Table>
</div>
{#if clips.some((clip) => clip.recovered)}
  <p class="footnote muted">Recovered clips were kept from a recording this PC could not finish, such as when it shut down mid-recording.</p>
{/if}
{#if more}
  <div class="more">
    <Button busy={loadingMore} onclick={onmore}>Show older clips</Button>
  </div>
{/if}

<style>
  th,
  td {
    white-space: nowrap;
  }
  .clip {
    min-width: 18ch;
    white-space: normal;
  }
  .title {
    display: block;
    font-weight: var(--weight-medium);
  }
  .tags {
    display: inline-flex;
    flex-wrap: wrap;
    gap: var(--space-1);
    margin-top: var(--space-1);
  }
  .picture .tags {
    margin-top: 0;
  }
  .sub {
    display: block;
    margin-top: var(--space-1);
    font-size: var(--text-xs);
    color: var(--muted);
  }
  .row-actions {
    text-align: end;
  }
  .row-actions :global(.button + .button) {
    margin-inline-start: var(--space-1);
  }
  .footnote {
    margin: 0;
    padding: var(--space-3) var(--space-5);
    border-top: 1px solid var(--line);
    font-size: var(--text-xs);
  }
  /* A phone stacks each row: the clip, then when and length, picture and size, then the actions. */
  @media (max-width: 520px) {
    .clips-table :global(table),
    tbody {
      display: block;
    }
    thead {
      position: absolute;
      width: 1px;
      height: 1px;
      overflow: hidden;
      clip-path: inset(50%);
    }
    .clips-table tr {
      display: grid;
      grid-template-columns: 1fr 1fr;
      gap: var(--space-3);
      padding: var(--space-3) var(--space-4);
      border-bottom: 1px solid var(--line);
    }
    .clips-table tr:last-child {
      border-bottom: 0;
    }
    .clips-table td {
      display: block;
      padding: 0;
      border: 0;
      text-align: start;
      white-space: normal;
    }
    .clips-table td[data-label]::before {
      content: attr(data-label);
      display: block;
      margin-bottom: var(--space-1);
      font-size: var(--text-xs);
      font-weight: var(--weight-semibold);
      letter-spacing: var(--tracking-label);
      text-transform: uppercase;
      color: var(--muted);
    }
    .clip,
    .row-actions {
      grid-column: 1 / -1;
      min-width: 0;
    }
    /* Three equal buttons on one line, so Delete never wraps away from the others. */
    .row-actions {
      display: grid;
      grid-template-columns: repeat(3, minmax(0, 1fr));
      gap: var(--space-1);
    }
    .row-actions :global(.button) {
      justify-content: center;
      padding-inline: var(--space-2);
    }
    .row-actions :global(.button + .button) {
      margin-inline-start: 0;
    }
    .footnote {
      padding-inline: var(--space-4);
    }
  }
  .more {
    display: flex;
    justify-content: center;
    padding: var(--space-4);
    border-top: 1px solid var(--line);
  }
</style>
