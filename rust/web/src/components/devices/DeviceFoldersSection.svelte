<script lang="ts">
  import Field from '../Field.svelte';
  import { link } from '../../lib/router.svelte';
  import { grantRows, withGrant, type FolderAccess, type SharedFolder } from '../../lib/shared-folders';
  import type { Draft } from './device';

  let {
    uid,
    draft = $bindable(),
    sharedFolders,
    foldersError,
  }: {
    uid: string;
    draft: Draft;
    sharedFolders: SharedFolder[] | null;
    foldersError: string;
  } = $props();

  const folderRows = $derived(grantRows(sharedFolders ?? [], draft.shared_folders));

  function setFolderAccess(id: string, access: string) {
    draft.shared_folders = withGrant(draft.shared_folders, id, access === 'read' || access === 'write' ? (access as FolderAccess) : '');
  }
</script>

{#if sharedFolders === null}
  <p class="note">Loading folders…</p>
{:else}
  {#if foldersError}<p class="error">{foldersError}</p>{/if}
  {#if folderRows.length === 0}
    <p class="note">
      This PC shares no folders. Add them under Shared folders in <a href="/settings/network" use:link>Settings, Network</a>.
    </p>
  {:else}
    <div class="form-grid">
      {#each folderRows as row (row.id)}
        <Field
          label={row.gone ? `${row.id} (no longer shared)` : row.name}
          id="{uid}-folder-{row.id}"
          hint={row.gone ? 'The PC shares no folder with this id now. Choose No access to remove the grant.' : row.path}
        >
          <select
            id="{uid}-folder-{row.id}"
            class="select"
            value={row.access}
            onchange={(event) => setFolderAccess(row.id, event.currentTarget.value)}
          >
            <option value="">No access</option>
            <option value="read">Read: browse and download</option>
            <option value="write">Read and write: also upload</option>
          </select>
        </Field>
      {/each}
    </div>
  {/if}
  <p class="note">
    Folders are shared only here: Everything and the file transfer permissions don’t include them. Taking a folder
    away, or write access to it, disconnects the device.
  </p>
{/if}

<style>
  .note {
    font-size: var(--text-xs);
    color: var(--muted);
    line-height: 1.45;
  }
  .error {
    font-size: var(--text-xs);
    color: var(--danger);
    line-height: 1.45;
  }
</style>
