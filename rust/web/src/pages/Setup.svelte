<script lang="ts">
  import BrandMark from '../components/BrandMark.svelte';
  import Button from '../components/Button.svelte';
  import Field from '../components/Field.svelte';
  import { api } from '../lib/api';
  import { navigate } from '../lib/router.svelte';
  import { checkSession } from '../lib/session.svelte';

  let username = $state('');
  let password = $state('');
  let confirmation = $state('');
  let busy = $state(false);
  let error = $state('');
  /** Create account was pressed: missing or short values show their message from then on. */
  let tried = $state(false);

  // The button stays a ready primary button; what is missing shows on the field when it is pressed.
  const problems = $derived({
    username: username.trim() ? '' : 'Enter a username.',
    password: password.length >= 8 ? '' : 'Use at least 8 characters.',
    confirmation: confirmation === password ? '' : 'The passwords do not match.',
  });
  const usernameError = $derived(tried ? problems.username : '');
  const passwordError = $derived(tried ? problems.password : '');
  // A different confirmation shows as soon as one is typed.
  const confirmationError = $derived(tried || confirmation.length > 0 ? problems.confirmation : '');

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    tried = true;
    const first = problems.username ? 'new-username' : problems.password ? 'new-password' : problems.confirmation ? 'confirm-password' : '';
    if (first) {
      document.getElementById(first)?.focus();
      return;
    }
    busy = true;
    error = '';
    const trimmed = username.trim();
    try {
      await api.auth.setPassword({ newUsername: trimmed, newPassword: password, confirmNewPassword: confirmation });
      await api.auth.login(trimmed, password, true);
      navigate('/', { replace: true });
      await checkSession();
    } catch (failure) {
      error = failure instanceof Error ? failure.message : String(failure);
    } finally {
      busy = false;
    }
  }
</script>

<main class="setup">
  <div class="column">
    <div class="brand">
      <BrandMark size={40} />
      <span>Rubylight</span>
    </div>

    <section class="card" aria-labelledby="setup-title">
      <header class="head">
        <p class="eyebrow">First-time setup</p>
        <h1 id="setup-title">Set up Rubylight</h1>
        <p class="lead">Create the console sign-in for your PC, then pair a phone, handheld, TV or laptop in seconds.</p>
      </header>

      <ol class="journey" aria-label="Setup steps">
        <li class="active" aria-current="step">
          <span class="step-num" aria-hidden="true">1</span>
          <span>Create credentials</span>
        </li>
        <li>
          <span class="step-num" aria-hidden="true">2</span>
          <span>Pair Nightjar or Moonlight</span>
        </li>
        <li>
          <span class="step-num" aria-hidden="true">3</span>
          <span>Play</span>
        </li>
      </ol>

      <form onsubmit={submit} novalidate>
        <Field label="Username" id="new-username" error={usernameError || undefined}>
          <input id="new-username" class="input" autocomplete="username" required aria-invalid={usernameError ? true : undefined} bind:value={username} />
        </Field>
        <Field label="Password" id="new-password" hint="At least 8 characters." error={passwordError || undefined}>
          <input id="new-password" class="input" type="password" autocomplete="new-password" minlength="8" required aria-invalid={passwordError ? true : undefined} bind:value={password} />
        </Field>
        <Field label="Confirm password" id="confirm-password" error={confirmationError || undefined}>
          <input id="confirm-password" class="input" type="password" autocomplete="new-password" required aria-invalid={confirmationError ? true : undefined} bind:value={confirmation} />
        </Field>
        <!-- Screen readers hear what is missing when the button is pressed; the fields show it too. -->
        <p class="visually-hidden" role="alert">{tried ? usernameError || passwordError || confirmationError : ''}</p>
        {#if error}<p class="notice danger" role="alert">{error}</p>{/if}
        <Button type="submit" variant="primary" {busy}>Create account</Button>
      </form>
    </section>
  </div>
</main>

<style>
  .setup {
    --panel: var(--media-panel);
    --panel-raised: var(--media-panel-raised);
    --sunken: var(--media-bg);
    --field: var(--media-bg);
    --ink: var(--media-ink);
    --ink-2: var(--media-ink-2);
    --muted: var(--media-idle);
    --line: var(--media-line);
    --line-strong: var(--media-line-strong);
    --focus: var(--media-focus);
    --edge: var(--media-edge);
    --accent-text: var(--media-accent-text);
    --accent-soft: color-mix(in srgb, var(--media-accent-text) 16%, var(--media-panel));
    --accent-line: color-mix(in srgb, var(--media-accent-text) 42%, var(--media-panel));
    --danger: var(--media-danger);
    --danger-soft: color-mix(in srgb, var(--media-danger) 14%, var(--media-panel));
    --danger-line: color-mix(in srgb, var(--media-danger) 40%, var(--media-panel));
    min-height: 100vh;
    min-height: 100dvh;
    display: grid;
    place-items: center;
    padding: var(--space-7) var(--space-4);
    background: var(--media-glow) no-repeat, var(--media-bg);
    color: var(--media-ink);
  }
  .column {
    width: min(520px, 100%);
    display: grid;
    gap: var(--space-5);
  }
  .brand {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: var(--space-3);
    font-family: var(--font-display);
    font-weight: var(--weight-bold);
    font-size: var(--text-lg);
    letter-spacing: var(--tracking-display);
    color: var(--media-ink);
  }
  .card {
    display: grid;
    gap: var(--space-5);
    padding: var(--space-6);
    background: var(--media-panel);
    border: 1px solid var(--media-line-strong);
    border-radius: var(--radius-xl);
    box-shadow: var(--media-edge), var(--media-shadow);
  }
  .head {
    display: grid;
    gap: var(--space-2);
  }
  .eyebrow {
    color: var(--media-accent-text);
  }
  h1 {
    font-size: var(--text-2xl);
    letter-spacing: var(--tracking-display);
  }
  .lead {
    color: var(--media-ink-2);
    font-size: var(--text-sm);
    line-height: var(--leading);
  }
  .journey {
    display: grid;
    grid-template-columns: repeat(3, minmax(0, 1fr));
    gap: var(--space-2);
    margin: 0;
    padding: var(--space-3);
    list-style: none;
    border: 1px solid var(--media-line);
    border-radius: var(--radius-lg);
    background: var(--media-bg);
  }
  .journey li {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    min-width: 0;
    padding: var(--space-2);
    border: 1px solid transparent;
    border-radius: var(--radius);
    color: var(--media-idle);
    font-size: var(--text-xs);
    font-weight: var(--weight-medium);
    line-height: var(--leading-snug);
  }
  .journey li.active {
    border-color: var(--accent-line);
    background: var(--accent-soft);
    color: var(--media-ink);
    font-weight: var(--weight-semibold);
  }
  .step-num {
    flex: none;
    display: grid;
    place-items: center;
    width: 22px;
    height: 22px;
    border: 1px solid var(--media-line-strong);
    border-radius: 50%;
    background: var(--media-panel);
    color: var(--media-ink-2);
    font-size: var(--text-xs);
    font-weight: var(--weight-bold);
    font-variant-numeric: tabular-nums;
  }
  .journey li.active .step-num {
    border-color: var(--media-accent);
    background: var(--media-accent);
    color: var(--media-accent-ink);
  }
  form {
    display: grid;
    gap: var(--space-4);
    padding-top: var(--space-4);
    border-top: 1px solid var(--media-line);
  }
  form :global(.button) {
    width: 100%;
    margin-top: var(--space-1);
  }
  /* A field with a message under it is marked in the same red. */
  .input[aria-invalid='true'] {
    border-color: var(--danger);
  }
  @media (max-width: 560px) {
    .setup {
      place-items: start center;
      padding: var(--space-6) var(--space-4);
    }
    .card {
      padding: var(--space-5);
    }
    .journey {
      grid-template-columns: minmax(0, 1fr);
    }
  }
</style>
