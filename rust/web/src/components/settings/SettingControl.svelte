<script lang="ts">
  import { tick, untrack } from 'svelte';
  import PathPicker from '../PathPicker.svelte';
  import FoldersControl from './FoldersControl.svelte';
  import ListControl from './ListControl.svelte';
  import SelectControl from './SelectControl.svelte';
  import Switch from './Switch.svelte';
  import type { ConfigValue } from '../../lib/api';
  import type { Option, Setting } from '../../lib/settings-types';
  import {
    jsonText,
    numberError,
    parseJson,
    sameValue,
    textError,
    timeOfDay,
    toNumber,
    toText,
  } from './values';

  let {
    setting,
    id,
    value,
    onchange,
    invalidText,
    oninvalid,
    describedby,
    daily,
    pnp,
  }: {
    setting: Setting;
    /** On the control the row's label points at. */
    id: string;
    /** The effective value, normalised for this control. */
    value: ConfigValue;
    onchange: (value: ConfigValue) => void;
    /** An invalid JSON, number or text draft, kept by the page across category changes. */
    invalidText?: string;
    /** Called with an invalid draft, or null when the control is valid again. */
    oninvalid?: (text: string | null) => void;
    /** The id of the row's description. */
    describedby?: string;
    /**
     * The daily time a number control's `daily` choice keeps in another
     * setting: its value, and a setter (null clears it).
     */
    daily?: { value: string; onchange: (value: string | null) => void };
    /**
     * The graphics card's device ID setting, for an adapter control: its value,
     * and a setter (null clears it). Kept only for cards that share a name.
     */
    pnp?: { value: string; onchange: (value: string | null) => void };
  } = $props();

  const control = $derived(setting.control);
  const label = $derived(setting.label);
  const current = $derived(toText(value));
  /** The control's own messages first, then the row's description. */
  const describe = (...ids: (string | false | undefined)[]) => [...ids, describedby].filter(Boolean).join(' ') || undefined;

  const isSelect = $derived(
    control.kind === 'select' || control.kind === 'display' || control.kind === 'audio' || control.kind === 'adapter',
  );
  const isList = $derived(
    control.kind === 'list' || control.kind === 'commands' || control.kind === 'server-commands' || control.kind === 'flags',
  );

  // ---------- number ----------

  // What is typed, so a half-typed number is not replaced while editing.
  let numberText = $state(untrack(() => invalidText ?? (typeof value === 'number' ? String(value) : '')));

  $effect(() => {
    const next = control.kind === 'number' && typeof value === 'number' ? value : null;
    const pending = invalidText;
    untrack(() => {
      if (control.kind !== 'number') return;
      if (pending !== undefined) {
        numberText = pending;
        return;
      }
      const shown = numberText.trim() === '' ? null : toNumber(numberText);
      if (shown !== next) numberText = next === null ? '' : String(next);
    });
  });

  function editNumber(input: HTMLInputElement) {
    const text = input.value;
    numberText = text;
    // Browsers expose incomplete numbers (for example "1e") as an empty
    // value with badInput. Keep that distinct from intentionally clearing it.
    if (numberError(setting, text, input.validity.badInput)) {
      oninvalid?.(text);
    } else {
      oninvalid?.(null);
      onchange(toNumber(text) ?? '');
    }
  }

  const rangeError = $derived(numberError(setting, numberText, invalidText === ''));
  // An empty field uses the default, so the placeholder names it.
  const numberDefault = $derived(toNumber(setting.default));
  const numberPlaceholder = $derived(numberDefault === null ? 'Not set' : `Default: ${numberDefault}`);

  // ---------- presets: named values in a menu, Custom for any other ----------

  const CUSTOM = '\u0000custom';
  /** The number control's daily choice: a time of day in another setting replaces the number. */
  const DAILY = '\u0000daily';
  const dailyChoice = $derived(control.kind === 'number' && daily ? control.daily : undefined);
  const presets = $derived.by((): Option[] => {
    if (control.kind === 'number') {
      const list = (control.presets ?? []).map((preset) => ({ value: String(preset.value), label: preset.label }));
      if (!dailyChoice) return list;
      // After the preset the daily time replaces: once a day.
      const at = list.findIndex((preset) => preset.value === '86400');
      list.splice(at + 1, 0, { value: DAILY, label: dailyChoice.label });
      return list;
    }
    if (control.kind === 'text') return control.presets ?? [];
    return [];
  });
  /** Custom was chosen in the menu, so its field stays open whatever is typed. */
  let customChosen = $state(false);
  /** The daily choice was made in the menu, so its time field stays open while it is empty. */
  let dailyChosen = $state(false);
  /** Set while the time field itself empties the time, which keeps the daily choice. */
  let clearing = false;
  // A time emptied from outside the field (Undo, Reset) ends the choice.
  $effect(() => {
    const time = daily?.value.trim() ?? '';
    untrack(() => {
      if (time === '' && !clearing) dailyChosen = false;
      clearing = false;
    });
  });
  /** As the host reads it: a daily time applies while the number turns checks on. */
  const dailySet = $derived(
    !!dailyChoice && !!daily?.value.trim() && (toNumber(value) ?? toNumber(setting.default) ?? 0) > 0,
  );
  /** The preset the current value is, or '' when it is none of them. */
  const presetValue = $derived.by(() => {
    if (!presets.length || invalidText !== undefined) return '';
    if (control.kind === 'number') {
      // An empty time is daily only while the schedule is still once a day.
      if (dailySet || (dailyChosen && toNumber(value) === 86400)) return DAILY;
      const number = toNumber(value);
      return number === null ? '' : (presets.find((preset) => Number(preset.value) === number)?.value ?? '');
    }
    return presets.find((preset) => preset.value === current.trim())?.value ?? '';
  });
  const hasValue = $derived(invalidText !== undefined || (control.kind === 'number' ? toNumber(value) !== null : current.trim() !== ''));
  /** The field for a value of one's own shows. */
  const custom = $derived(presets.length > 0 && presetValue !== DAILY && (customChosen || (presetValue === '' && hasValue)));
  /** The id of the field that edits the value: the menu has the row's own id when there are presets. */
  const fieldId = $derived(presets.length ? `${id}-custom` : id);

  async function choosePreset(choice: string) {
    if (choice === DAILY && daily) {
      customChosen = false;
      dailyChosen = true;
      oninvalid?.(null);
      // Once a day, should the time be cleared later; checks stay on.
      numberText = '86400';
      onchange(86400);
      daily.onchange(daily.value.trim() || '03:00');
      await tick();
      document.getElementById(`${id}-daily`)?.focus();
      return;
    }
    // Any other choice is the schedule itself, so a daily time no longer overrides it.
    dailyChosen = false;
    if (daily?.value.trim()) daily.onchange(null);
    if (choice === CUSTOM) {
      customChosen = true;
      await tick();
      document.getElementById(fieldId)?.focus();
      return;
    }
    customChosen = false;
    oninvalid?.(null);
    if (control.kind === 'number') {
      numberText = choice;
      onchange(Number(choice));
    } else {
      onchange(choice);
    }
  }

  // ---------- text ----------

  const textShown = $derived(invalidText ?? current);
  const textProblem = $derived(control.kind === 'text' ? textError(setting, textShown) : '');
  const isTime = $derived(control.kind === 'text' && control.time === true);
  // A time field shows nothing for text it cannot read, such as 3:00, which
  // the host reads as 03:00.
  const fieldValue = $derived(isTime ? timeOfDay(textShown) : textShown);

  function editText(text: string) {
    if (textError(setting, text)) {
      oninvalid?.(text);
    } else {
      oninvalid?.(null);
      onchange(text);
    }
  }

  // ---------- json ----------

  let jsonDraft = $state(untrack(() => invalidText ?? jsonText(value)));
  const jsonError = $derived.by(() => {
    if (control.kind !== 'json') return '';
    const parsed = parseJson(jsonDraft);
    return parsed.ok ? '' : parsed.error;
  });
  const jsonRows = $derived(Math.min(16, Math.max(2, jsonDraft.split('\n').length + 1)));

  // Follow resets and discards; leave the text alone while it says the same thing.
  $effect(() => {
    const next = value;
    const pending = invalidText;
    untrack(() => {
      if (control.kind !== 'json') return;
      if (pending !== undefined) {
        if (pending !== jsonDraft) jsonDraft = pending;
        return;
      }
      const parsed = parseJson(jsonDraft);
      if (!parsed.ok || !sameValue(parsed.value, next ?? '')) jsonDraft = jsonText(next);
    });
  });

  function editJson(text: string) {
    jsonDraft = text;
    const parsed = parseJson(text);
    if (parsed.ok) {
      oninvalid?.(null);
      onchange(parsed.value);
    } else {
      oninvalid?.(text);
    }
  }
</script>

{#if control.kind === 'toggle'}
  <Switch {id} {describedby} checked={value === true} onchange={(checked) => onchange(checked)} />
{:else if isSelect}
  <SelectControl {setting} {id} {value} {onchange} {describedby} {pnp} />
{:else if control.kind === 'number' || control.kind === 'text'}
  {#if presets.length}
    <select
      {id}
      class="select"
      aria-describedby={describe()}
      onchange={(event) => void choosePreset(event.currentTarget.value)}
    >
      {#if !custom && presetValue === ''}<option value="" selected>Not set</option>{/if}
      {#each presets as option (option.value)}
        <option value={option.value} selected={!custom && option.value === presetValue}>{option.label}</option>
      {/each}
      <option value={CUSTOM} selected={custom}>Custom</option>
    </select>
  {/if}
  {#if presetValue === DAILY && daily}
    <input
      id="{id}-daily"
      class="input"
      type="time"
      aria-label={`${label}, time of day`}
      aria-describedby={describe()}
      value={timeOfDay(daily.value)}
      oninput={(event) => {
        clearing = !event.currentTarget.value;
        daily.onchange(event.currentTarget.value || null);
      }}
    />
  {/if}
  {#if !presets.length || custom}
    {@render field()}
  {/if}
{:else if isList}
  <ListControl {setting} {id} {value} {onchange} {describedby} />
{:else if control.kind === 'folders'}
  <FoldersControl {id} {value} {onchange} {oninvalid} />
{:else if control.kind === 'json'}
  <textarea
    {id}
    class="textarea"
    rows={jsonRows}
    spellcheck="false"
    autocomplete="off"
    aria-invalid={jsonError ? 'true' : undefined}
    aria-describedby={describe(!!jsonError && `${id}-json`)}
    value={jsonDraft}
    oninput={(event) => editJson(event.currentTarget.value)}
  ></textarea>
  {#if jsonError}
    <p class="error" id="{id}-json">
      Not valid JSON: {jsonError}. Use double quotes around text, commas between entries, and matching brackets.
    </p>
  {/if}
{/if}

{#snippet field()}
{#if control.kind === 'number'}
  <div class="number" style:--unit-width={control.unit ? `${control.unit.length + 1}ch` : undefined}>
    <input
      id={fieldId}
      class="input num"
      type="number"
      inputmode="decimal"
      min={control.min}
      max={control.max}
      step={control.step ?? 'any'}
      placeholder={numberPlaceholder}
      aria-label={presets.length ? `${label}, custom value` : undefined}
      aria-invalid={rangeError ? 'true' : undefined}
      aria-describedby={describe(!!control.unit && `${id}-unit`, !!rangeError && `${id}-range`)}
      value={numberText}
      oninput={(event) => editNumber(event.currentTarget)}
    />
    {#if control.unit}<span class="unit" id="{id}-unit">{control.unit}</span>{/if}
  </div>
  {#if rangeError}<p class="error" id="{id}-range">{rangeError}</p>{/if}
{:else if control.kind === 'text'}
  {#snippet text()}
    <input
      id={fieldId}
      class="input"
      class:mono={control.mono}
      type={isTime ? 'time' : 'text'}
      autocomplete="off"
      spellcheck={control.mono ? 'false' : undefined}
      placeholder={control.placeholder ?? (toText(setting.default) || undefined)}
      aria-label={presets.length ? `${label}, custom value` : undefined}
      aria-invalid={textProblem ? 'true' : undefined}
      aria-describedby={describe(!!textProblem && `${id}-text`)}
      value={fieldValue}
      oninput={(event) => editText(event.currentTarget.value)}
    />
  {/snippet}
  {#if control.browse}
    <PathPicker kind={control.browse} value={current} onpick={onchange} children={text} />
  {:else}
    {@render text()}
  {/if}
  {#if textProblem}<p class="error" id="{id}-text">{textProblem}</p>{/if}
{/if}
{/snippet}

<style>
  .error {
    font-size: var(--text-xs);
    color: var(--danger);
    line-height: 1.45;
  }
  [aria-invalid='true'] {
    border-color: var(--danger);
  }
  /* Sized by its rows: an empty list needs two lines, not the page default. */
  .textarea {
    min-height: 0;
  }
  /* The unit sits inside the field, so every number field has the same width. */
  .number {
    position: relative;
    display: grid;
  }
  .number .input {
    appearance: textfield;
    padding-right: calc(var(--unit-width, 0ch) + 20px);
  }
  .number .input::-webkit-inner-spin-button,
  .number .input::-webkit-outer-spin-button {
    appearance: none;
    margin: 0;
  }
  .unit {
    position: absolute;
    right: 10px;
    top: 50%;
    transform: translateY(-50%);
    font-size: var(--text-sm);
    color: var(--muted);
    pointer-events: none;
  }
</style>
