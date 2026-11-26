<script lang="ts">
  import Field from '../Field.svelte';
  import Toggle from '../Toggle.svelte';
  import NumberField from './NumberField.svelte';
  import ChoiceField, { type Choice } from './ChoiceField.svelte';
  import type { AppDraft } from './draft.svelte';

  let { draft }: { draft: AppDraft } = $props();

  const uid = $props.id();
  /** display_session.rs reads the legacy switch only while the app has no Frame generation value of its own. */
  const generates = $derived(draft.text('frame-generation-mode').trim() !== '');

  const GENERATION: Choice[] = [
    { value: 'game-provided', label: 'Provided by the game', hint: 'The game adds generated frames, e.g. FSR frame generation.' },
    {
      value: 'lossless-scaling',
      label: 'Lossless Scaling',
      hint: 'Lossless Scaling adds generated frames; the game is limited to half the target. See Lossless Scaling below.',
    },
  ];

  const GAMEPADS: Choice[] = [
    { value: 'auto', label: 'Automatic' },
    { value: 'vhf_xbox_one', label: 'Xbox One' },
    { value: 'vhf_xbox', label: 'Xbox Series' },
    { value: 'vhf_ds4', label: 'DualShock 4' },
    { value: 'vhf_ds5', label: 'DualSense', hint: 'Some games read the controller’s name and can treat this pad as an unknown controller, so the camera or cursor can drift with no input; for such a game choose DualSense with waveform haptics (USB/IP) or Xbox Series instead.' },
    { value: 'usbip_ds5', label: 'DualSense with waveform haptics (USB/IP)', hint: 'Requires usbip-win2 on the host. Start the game after connecting the controller.' },
    { value: 'usbip_switch2', label: 'Switch 2 Pro (USB/IP)', hint: 'Requires usbip-win2 on the host. Start the game after connecting the controller.' },
    { value: 'vhf_switch', label: 'Switch Pro' },
    { value: 'vhf_steam', label: 'Steam Controller', hint: 'Requires the current virtual gamepad driver; run Rubylight setup again if the stream says the driver lacks it.' },
    { value: 'vigem_x360', label: 'Xbox 360 (ViGEmBus)', hint: 'Requires ViGEmBus on the host, for tools such as reWASD that may not see the other types.' },
    { value: 'vigem_ds4', label: 'DualShock 4 (ViGEmBus)', hint: 'Requires ViGEmBus on the host, for tools such as reWASD that may not see the other types. Keeps the touchpad, motion and lightbar.' },
  ];
</script>

<div class="video">
  <div class="form-grid">
    <Field
      label="Prefer 10-bit SDR"
      id="{uid}-sdr"
      hint="When on, a device that asks for HDR gets 10-bit SDR instead and the display stays in SDR: less banding, but no HDR picture. Needs HEVC or AV1."
    >
      <select
        id="{uid}-sdr"
        class="select"
        bind:value={() => draft.tristate('prefer-10bit-sdr'), (value) => draft.setTristate('prefer-10bit-sdr', value)}
      >
        <option value="">Use the host setting</option>
        <option value="on">On</option>
        <option value="off">Off</option>
      </select>
    </Field>
    <ChoiceField
      {draft}
      key="frame-generation-mode"
      label="Frame generation"
      hint="Tells the host that this app’s frames are generated, so its frame limiter keeps the stream in pace. Leave it on None unless the game or Lossless Scaling generates frames."
      empty="None"
      options={GENERATION}
    />
    <ChoiceField
      {draft}
      key="gamepad"
      label="Controller type"
      hint="Automatic matches each device’s controller: DualSense for PlayStation or for motion and touchpad input, Switch Pro for Nintendo, Xbox Series otherwise. A choice here uses that controller for this app. If a game drifts the camera or cursor with no input, it may read the controller’s name: choose DualSense with waveform haptics (USB/IP) or Xbox Series for this app."
      options={GAMEPADS}
    />
    <NumberField
      {draft}
      key="absolute-left-release-delay"
      label="Touch and pen click delay"
      hint="How long a tap on a touch screen or a pen holds its left-button release back, in milliseconds: 0 to 1000. Raise it if a game misses taps or reads them as a drag; leave it empty to use the host setting."
      placeholder="10"
      unit="ms"
    />
    <Field
      label="Native pen and touch"
      id="{uid}-native-pen-touch"
      hint="Send pen and touch input from paired devices to Windows as pen and touch. Turn it off for games that handle them badly; devices then send touch as mouse input."
    >
      <select
        id="{uid}-native-pen-touch"
        class="select"
        bind:value={() => draft.tristate('native-pen-touch'), (value) => draft.setTristate('native-pen-touch', value)}
      >
        <option value="">Use the host setting</option>
        <option value="on">On</option>
        <option value="off">Off</option>
      </select>
    </Field>
  </div>

  <div class="toggles">
    <Toggle
      label="Legacy frame generation capture fix"
      hint={generates
        ? 'Not used: Frame generation above is set and takes its place.'
        : 'An older way to mark the app as generating frames, so the host’s frame limiter paces it on a streamed monitor. Choose Frame generation above for new apps.'}
      disabled={generates}
      bind:checked={() => draft.flag('gen1-framegen-fix'), (value) => draft.setFlag('gen1-framegen-fix', value)}
    />
  </div>
</div>

<style>
  .video {
    display: grid;
    gap: var(--space-5);
  }
  .toggles {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(min(100%, 280px), 1fr));
    gap: var(--space-4) var(--space-5);
  }
</style>
