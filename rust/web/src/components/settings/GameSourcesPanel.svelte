<script lang="ts">
  import Button from '../Button.svelte';
  import Panel from '../Panel.svelte';

  let { id, title }: { id: string; title: string } = $props();
</script>

<!-- The launchers' options: each has its switch and its Options link on the Games page. -->
<Panel {id} {title} level={3} flush>
  <div class="sources">
    <p class="muted">
      Steam, Epic Games, Battle.net, Microsoft Store and Xbox, EA app, Ubisoft Connect and GOG: what each adds, and its options, are on the
      Games page under Where your games come from.
    </p>
    <Button size="sm" href="/library" icon="library">Open Games</Button>
  </div>
</Panel>

<style>
  .sources {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-3);
    padding: var(--space-4) var(--space-5);
  }
  .sources p {
    margin: 0;
    max-width: 62ch;
    font-size: var(--text-sm);
  }
</style>
