<script lang="ts">
  import Field from '../Field.svelte';
  import { COUCH_KEYBOARD_CHORDS, COUCH_OPEN_WITH_OPTIONS, type Draft } from './device';

  let {
    uid,
    draft = $bindable(),
    openWithUnknown,
    chordUnknown,
    holdInvalid,
    cornerInvalid,
  }: {
    uid: string;
    draft: Draft;
    openWithUnknown: boolean;
    chordUnknown: boolean;
    holdInvalid: boolean;
    cornerInvalid: boolean;
  } = $props();
</script>

<div class="form-grid">
  <Field
    label="Couch menu"
    id="{uid}-couch"
    hint="The menu the PC draws over this device's stream, in place of Couch menu in Settings. Choose Off for a device whose games use the buttons that open it."
  >
    <select id="{uid}-couch" class="select" bind:value={draft.couch.couch_menu}>
      <option value="">Use the host setting</option>
      <option value="on">On</option>
      <option value="off">Off</option>
    </select>
  </Field>
  <Field
    label="Open the couch menu with"
    id="{uid}-couch-open"
    hint="The ways to open it on this device, in place of the host's. Leave it on Use the host setting unless this device needs its own."
    error={openWithUnknown ? `“${draft.couch.couch_menu_open_with}” is not a choice, so the host ignores it. Choose one.` : undefined}
  >
    <select id="{uid}-couch-open" class="select" bind:value={draft.couch.couch_menu_open_with}>
      <option value="">Use the host setting</option>
      {#if openWithUnknown}
        <option value={draft.couch.couch_menu_open_with}>{draft.couch.couch_menu_open_with} (not a choice)</option>
      {/if}
      {#each COUCH_OPEN_WITH_OPTIONS as option (option.value)}
        <option value={option.value}>{option.label}</option>
      {/each}
    </select>
  </Field>
  <Field
    label="Controller chord"
    id="{uid}-couch-chord"
    hint="The buttons held together on one controller, joined with +, such as l3+r3 or select. Leave it empty to use the host's."
  >
    <input
      id="{uid}-couch-chord"
      class="input mono"
      type="text"
      placeholder="l3+r3"
      autocomplete="off"
      bind:value={draft.couch.couch_menu_controller_chord}
    />
  </Field>
  <Field
    label="Controller chord hold"
    id="{uid}-couch-hold"
    hint="How long the chord is held before the menu opens, 300 to 3000 ms. Leave it empty to use the host setting."
    error={holdInvalid ? 'Enter a whole number from 300 to 3000, or leave it empty.' : undefined}
  >
    <input
      id="{uid}-couch-hold"
      class="input num"
      type="text"
      inputmode="numeric"
      placeholder="1000"
      aria-invalid={holdInvalid ? 'true' : undefined}
      bind:value={draft.couch.couch_menu_hold_ms}
    />
  </Field>
  <Field
    label="Corner hold"
    id="{uid}-couch-corner"
    hint="How long a touch, pen or mouse button holds the top-left corner before the menu opens, 500 to 3000 ms. Leave it empty to use the host setting."
    error={cornerInvalid ? 'Enter a whole number from 500 to 3000, or leave it empty.' : undefined}
  >
    <input
      id="{uid}-couch-corner"
      class="input num"
      type="text"
      inputmode="numeric"
      placeholder="1500"
      aria-invalid={cornerInvalid ? 'true' : undefined}
      bind:value={draft.couch.couch_menu_corner_hold_ms}
    />
  </Field>
  <Field
    label="Keyboard chord"
    id="{uid}-couch-keys"
    hint="The keys that open the menu from this device's keyboard, in place of the host's."
    error={chordUnknown ? `“${draft.couch.couch_menu_keyboard_chord}” is not a choice, so the host ignores it. Choose one.` : undefined}
  >
    <select id="{uid}-couch-keys" class="select" bind:value={draft.couch.couch_menu_keyboard_chord}>
      <option value="">Use the host setting</option>
      {#if chordUnknown}
        <option value={draft.couch.couch_menu_keyboard_chord}>{draft.couch.couch_menu_keyboard_chord} (not a choice)</option>
      {/if}
      {#each COUCH_KEYBOARD_CHORDS as option (option.value)}
        <option value={option.value}>{option.label}</option>
      {/each}
    </select>
  </Field>
</div>
