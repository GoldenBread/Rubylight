<script lang="ts">
  import type { ConfigValue } from '../../lib/api';
  import type { Option, Setting } from '../../lib/settings-types';
  import { session } from '../../lib/session.svelte';
  import { toText } from './values';

  let {
    setting,
    id,
    value,
    onchange,
    describedby,
    pnp,
  }: {
    setting: Setting;
    id: string;
    value: ConfigValue;
    onchange: (value: ConfigValue) => void;
    describedby?: string;
    pnp?: { value: string; onchange: (value: string | null) => void };
  } = $props();

  const control = $derived(setting.control);
  const current = $derived(toText(value));
  const describe = (...ids: (string | false | undefined)[]) => [...ids, describedby].filter(Boolean).join(' ') || undefined;

  const same = (a: string, b: string) => a.toLowerCase() === b.toLowerCase();

  /** The device ID choices of an adapter control are this prefix and the ID. */
  const PNP = 'pnp:';
  /** The card names more than one listed GPU has: those are told apart by device ID. */
  const sharedNames = $derived.by(() => {
    const seen = new Set<string>();
    const shared = new Set<string>();
    for (const gpu of session.metadata?.gpus ?? []) {
      const name = gpu.name.toLowerCase();
      if (!name) continue;
      if (seen.has(name)) shared.add(name);
      seen.add(name);
    }
    return shared;
  });
  /** What the control shows as chosen: the saved device ID when there is one, else the name. */
  const selected = $derived.by(() => {
    const id = control.kind === 'adapter' ? (pnp?.value.trim() ?? '') : '';
    if (!id) return current;
    return PNP + id;
  });

  const choices = $derived.by((): Option[] => {
    const meta = session.metadata;
    const options: Option[] = [];
    switch (control.kind) {
      case 'select':
        if (!control.options.some((option) => option.value === '') && (current === '' || toText(setting.default) === '')) {
          options.push({ value: '', label: 'Not set' });
        }
        options.push(...control.options);
        break;
      case 'display':
        options.push({ value: '', label: 'Automatic' });
        for (const display of meta?.capture_status?.displays ?? []) {
          const name = display.display_name;
          if (!name) continue;
          // The host also matches the device id; keep the stored spelling selected.
          const matches = current !== '' && [name, display.device_id].some((field) => field && same(field, current));
          const choice = matches ? current : name;
          if (options.some((option) => option.value === choice)) continue;
          const text = display.friendly_name || `Display ${options.length}`;
          options.push({ value: choice, label: display.primary ? `${text} · primary` : text });
        }
        break;
      case 'audio':
        options.push({ value: '', label: 'System default' });
        for (const sink of meta?.audio_sinks ?? []) {
          // The host matches the id, name, description or adapter name.
          const matches =
            current !== '' && [sink.id, sink.name, sink.description, sink.adapter].some((field) => field && same(field, current));
          const choice = matches ? current : sink.id;
          if (!choice || options.some((option) => option.value === choice)) continue;
          const marks: string[] = [];
          if (sink.default) marks.push('default');
          if (sink.virtual_sink && !sink.name.toLowerCase().includes('steam streaming speakers')) {
            marks.push('Steam Streaming Speakers');
          }
          options.push({ value: choice, label: [sink.name || sink.description || 'Unnamed audio device', ...marks].join(' · ') });
        }
        break;
      case 'adapter': {
        options.push({ value: '', label: 'Automatic' });
        const listed = new Set<string>();
        const counts = new Map<string, number>();
        for (const gpu of meta?.gpus ?? []) {
          const name = gpu.name;
          if (!name) continue;
          const shared = sharedNames.has(name.toLowerCase()) && !!gpu.pnp_id && !!pnp;
          if (shared) {
            // Two cards with one name: each is its own choice, kept by its device ID.
            const nth = (counts.get(name.toLowerCase()) ?? 0) + 1;
            counts.set(name.toLowerCase(), nth);
            const id = gpu.pnp_id as string;
            options.push({ value: selected !== '' && same(selected, PNP + id) ? selected : PNP + id, label: `${name} (${nth})` });
          } else if (!listed.has(name.toLowerCase())) {
            listed.add(name.toLowerCase());
            options.push({ value: selected !== '' && same(selected, name) ? selected : name, label: name });
          }
        }
        break;
      }
      default:
        return [];
    }
    if (!options.some((option) => option.value === selected)) {
      const missing = {
        value: selected,
        label: selected === '' ? 'Not set' : `Saved custom choice: ${selected.startsWith(PNP) ? selected.slice(PNP.length) : selected}`,
      };
      options.splice(options[0]?.value === '' ? 1 : 0, 0, missing);
    }
    return options;
  });

  const listsHost = $derived(control.kind === 'display' || control.kind === 'audio' || control.kind === 'adapter');

  const hostError = $derived.by(() => {
    const meta = session.metadata;
    if (!meta) return '';
    if (control.kind === 'display') {
      return meta.capture_status?.error ? `Could not list displays: ${meta.capture_status.error}` : '';
    }
    if (control.kind === 'adapter') return meta.gpu_error ? `Could not list graphics cards: ${meta.gpu_error}` : '';
    if (control.kind === 'audio') return meta.audio_error ? `Could not list audio devices: ${meta.audio_error}` : '';
    return '';
  });

  /** A choice in the menu. A card that shares its name with another is kept by device ID, the rest by name. */
  function choose(choice: string) {
    if (control.kind === 'adapter' && pnp) {
      if (choice.startsWith(PNP)) {
        const id = choice.slice(PNP.length);
        const gpu = (session.metadata?.gpus ?? []).find((gpu) => gpu.pnp_id && same(gpu.pnp_id, id));
        pnp.onchange(id);
        if (gpu) onchange(gpu.name);
        return;
      }
      if (pnp.value.trim()) pnp.onchange(null);
    }
    onchange(choice);
  }
</script>

<select
  {id}
  class="select"
  aria-describedby={describe(!!hostError && `${id}-host`)}
  onchange={(event) => choose(event.currentTarget.value)}
>
  {#each choices as option (option.value)}
    <option value={option.value} selected={option.value === selected}>{option.label}</option>
  {/each}
  {#if listsHost && session.metadata === null}
    <option disabled value={'\u0000loading'}>Loading…</option>
  {/if}
</select>
{#if hostError}<p class="note" id="{id}-host">{hostError}</p>{/if}

<style>
  .note {
    font-size: var(--text-xs);
    color: var(--muted);
    line-height: 1.45;
  }
</style>
