<script lang="ts">
  import Field from '../Field.svelte';
  import { COUCH_KEYBOARD_CHORDS, COUCH_OPEN_WITH_OPTIONS } from '../devices/device';
  import ChoiceField from './ChoiceField.svelte';
  import NumberField from './NumberField.svelte';
  import type { AppDraft } from './draft.svelte';

  let { draft }: { draft: AppDraft } = $props();

  const uid = $props.id();
</script>

<div class="form-grid">
  <Field
    label="Night mode"
    id="{uid}-night"
    hint="Evens out loud and quiet passages for this app, in place of Night mode in Settings."
  >
    <select
      id="{uid}-night"
      class="select"
      bind:value={() => draft.tristate('audio-night-mode'), (value) => draft.setTristate('audio-night-mode', value)}
    >
      <option value="">Use the host setting</option>
      <option value="on">On</option>
      <option value="off">Off</option>
    </select>
  </Field>
  <Field
    label="Headphone surround"
    id="{uid}-surround"
    hint="For a device that plays stereo, renders the PC's surround channels for headphones so sound keeps its direction, in place of Headphone surround in Settings."
  >
    <select
      id="{uid}-surround"
      class="select"
      bind:value={() => draft.tristate('audio-headphone-surround'), (value) => draft.setTristate('audio-headphone-surround', value)}
    >
      <option value="">Use the host setting</option>
      <option value="on">On</option>
      <option value="off">Off</option>
    </select>
  </Field>
  <Field
    label="Couch menu"
    id="{uid}-couch"
    hint="The menu the PC draws over this app's stream, in place of Couch menu in Settings. Choose Off for a game that uses the buttons that open it."
  >
    <select
      id="{uid}-couch"
      class="select"
      bind:value={() => draft.tristate('couch-menu'), (value) => draft.setTristate('couch-menu', value)}
    >
      <option value="">Use the host setting</option>
      <option value="on">On</option>
      <option value="off">Off</option>
    </select>
  </Field>
  <ChoiceField
    {draft}
    key="couch-menu-open-with"
    label="Open the couch menu with"
    hint="The ways to open it in this app, in place of the host's."
    options={COUCH_OPEN_WITH_OPTIONS}
  />
  <Field
    label="Controller chord"
    id="{uid}-chord"
    hint="The buttons held together on one controller, joined with +, such as l3+r3 or select. Leave it empty to use the host's."
  >
    <input
      id="{uid}-chord"
      class="input mono"
      type="text"
      placeholder="l3+r3"
      autocomplete="off"
      bind:value={() => draft.text('couch-menu-controller-chord'), (value) => draft.setText('couch-menu-controller-chord', value)}
    />
  </Field>
  <NumberField
    {draft}
    key="couch-menu-hold-ms"
    label="Controller chord hold"
    hint="How long the chord is held before the menu opens, 300 to 3000 ms. Leave it empty to use the host's."
    placeholder="1000"
    unit="ms"
  />
  <NumberField
    {draft}
    key="couch-menu-corner-hold-ms"
    label="Corner hold"
    hint="How long a touch, pen or mouse button holds the top-left corner before the menu opens, 500 to 3000 ms. Leave it empty to use the host's."
    placeholder="1500"
    unit="ms"
  />
  <ChoiceField
    {draft}
    key="couch-menu-keyboard-chord"
    label="Keyboard chord"
    hint="The keys that open the menu from a keyboard, in place of the host's."
    options={COUCH_KEYBOARD_CHORDS}
  />
</div>
