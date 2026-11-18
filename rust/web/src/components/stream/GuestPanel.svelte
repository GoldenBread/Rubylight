<script lang="ts">
  // The Stream page's guests (docs/browser-guests.md): invite links, who watches, kick and
  // revoke. A link's token is shown once, right after it is made: the host keeps only its hash.
  import { onMount } from 'svelte';
  import Badge from '../Badge.svelte';
  import Button from '../Button.svelte';
  import CopyButton from '../CopyButton.svelte';
  import Dialog from '../Dialog.svelte';
  import Field from '../Field.svelte';
  import Input from '../Input.svelte';
  import InternetReach from './InternetReach.svelte';
  import Panel from '../Panel.svelte';
  import Select from '../Select.svelte';
  import { api, type CreatedInvite, type GuestEntry, type GuestInvite, type GuestPermission, type GuestReach, type GuestsStatus } from '../../lib/api';
  import { failed, notify } from '../../lib/feedback.svelte';
  import { poll } from '../../lib/format';
  import {
    AUDIENCES,
    EXPIRIES,
    PERMISSIONS,
    UNITS,
    expiresIn,
    guestLink,
    guestPorts,
    suggestedAddress,
    lifetime,
    linkAddress,
    permissionLabel,
    addressProblem,
    withPort,
    type Audience,
    type Expiry,
  } from '../../lib/guests';

  let status = $state.raw<GuestsStatus | null>(null);
  let loadError = $state('');
  let now = $state(Date.now() / 1000);

  let open = $state(false);
  // Strings, as the selects bind them.
  let permission = $state<string>('controller');
  let expiry = $state<string>('24h');
  let amount = $state<string | number | null>(2);
  let unit = $state('3600');
  let creating = $state(false);
  let created = $state.raw<CreatedInvite | null>(null);
  let address = $state('');
  /** The address the link starts with, so only one the owner typed is remembered. */
  let suggested = '';
  /** Where the guests are; "On the internet" maps the guest ports and finds the public address. */
  let audience = $state<string>('local');
  /** What the router did with the guest ports, for "On the internet". */
  let reach = $state.raw<GuestReach | null>(null);
  /** The owner typed over the address the host found: it stays. */
  let typed = $state(false);
  /** Counts audience changes, so an answer for an earlier choice is dropped. */
  let asked = 0;

  /** The address and port the link was made for. */
  let linked = $state('');

  const seconds = $derived(lifetime(expiry as Expiry, Number(amount), Number(unit)));
  const port = $derived(status?.port ?? null);
  // "On the internet" with no address: the router's answer is pending, or nothing reaches this PC
  // (InternetReach says so); Create stays off until there is one.
  const addressError = $derived(
    port === null
      ? 'The guest port is unknown'
      : audience === 'internet' && !address.trim()
        ? reach && !reach.checking
          ? 'Type a public address or DNS name the guest ports are forwarded to'
          : undefined
        : addressProblem(address),
  );
  const addressMissing = $derived(!address.trim());
  const link = $derived(created ? guestLink(linked, created.token) : '');
  const guests = $derived(status?.guests ?? []);
  const invites = $derived(status?.invites ?? []);
  const portNames = $derived(guestPorts(port, status?.media_ports ?? []));
  const internet = $derived(audience === 'internet');

  const ADDRESS_KEY = 'butterpollo.guest-address';
  /** An address typed for guests on the internet (such as a DNS name), kept apart from the local one. */
  const INTERNET_ADDRESS_KEY = 'butterpollo.guest-address.internet';
  const addressKey = (value: string) => (value === 'internet' ? INTERNET_ADDRESS_KEY : ADDRESS_KEY);
  // A browser that blocks site data throws on localStorage: the address then lasts for this visit.
  function savedAddress(value = 'local'): string | null {
    try {
      return localStorage.getItem(addressKey(value));
    } catch {
      return null;
    }
  }
  /** Remembers an address the owner typed over the one the dialog found, and forgets it when they
   * take the found one again. */
  function rememberAddress() {
    const found = audience === 'internet' ? (reach?.address ?? '') : suggested;
    try {
      if (address.trim() && address.trim() !== found) localStorage.setItem(addressKey(audience), address.trim());
      else localStorage.removeItem(addressKey(audience));
    } catch {
      // Not remembered; see savedAddress().
    }
  }

  async function load() {
    try {
      status = await api.guests.status();
      now = Date.now() / 1000;
      loadError = '';
    } catch (cause) {
      loadError = cause instanceof Error ? cause.message : String(cause);
    }
  }

  async function invite() {
    created = null;
    open = true;
    if (port === null) return;
    let lan: string[] = [];
    try {
      lan = (await api.guests.addresses()).addresses;
    } catch (cause) {
      failed('Reading this PC\'s addresses failed', cause);
    }
    suggested = linkAddress(location, lan, port);
    address = savedAddress() ?? suggested;
    if (audience === 'internet') void chooseAudience('internet');
  }

  /** "On the internet" asks the router for the guest ports and waits for its answer while the
   * dialog stays open, then puts the public address in the link, unless the owner typed one
   * there before (remembered) or since. */
  async function chooseAudience(value: string) {
    audience = value;
    const ask = ++asked;
    typed = false;
    if (value !== 'internet') {
      reach = null;
      address = savedAddress() ?? suggested;
      return;
    }
    reach = null;
    const saved = savedAddress('internet');
    address = saved ?? '';
    typed = saved !== null;
    try {
      let answer = await api.guests.prepareInternet();
      // A round can take a while (no UPnP router, several IPv6 addresses): keep asking every
      // second, then every three.
      for (let tries = 0; answer.checking && ask === asked && open; tries++) {
        await new Promise((resolve) => setTimeout(resolve, tries < 20 ? 1000 : 3000));
        if (ask !== asked || !open) return;
        answer = await api.guests.internet();
      }
      if (ask !== asked) return;
      reach = answer;
      if (!typed) address = suggestedAddress(value as Audience, suggested, answer);
    } catch (cause) {
      if (ask === asked) failed('Asking the router failed', cause);
    }
  }

  async function create() {
    if (seconds === null || creating || addressError || port === null) return;
    creating = true;
    try {
      const target = withPort(address, port);
      const made = await api.guests.invite(permission as GuestPermission, seconds, target, internet);
      linked = target;
      rememberAddress();
      created = made;
      void load();
    } catch (cause) {
      failed('Creating the link failed', cause);
    } finally {
      creating = false;
    }
  }

  function done() {
    open = false;
  }

  async function kick(guest: GuestEntry) {
    try {
      await api.guests.kick(guest.id);
      notify(`${guest.name} left the stream.`, 'ok');
    } catch (cause) {
      failed(`Removing ${guest.name} failed`, cause);
    }
    void load();
  }

  async function revoke(invite: GuestInvite) {
    try {
      await api.guests.revoke(invite.id);
      notify('Link revoked.', 'ok');
    } catch (cause) {
      failed('Revoking the link failed', cause);
    }
    void load();
  }

  const joinedWith = (invite: GuestInvite) => guests.filter((g) => g.invite === invite.id).length;

  onMount(() => poll(load, 3000));
</script>

<Panel title="Guests" flush>
  {#snippet actions()}
    <Button variant="primary" size="sm" icon="link" onclick={invite}>Invite</Button>
  {/snippet}
  {#if loadError && !status}
    <p class="notice danger row" role="alert">{loadError}</p>
  {:else if !guests.length && !invites.length}
    <p class="muted row">No guests</p>
  {:else}
    <ul class="list" aria-label="Guests and links">
      {#each guests as guest (guest.id)}
        <li>
          <span class="name">{guest.name}</span>
          <Badge tone="live">Watching</Badge>
          <Badge>{permissionLabel(guest.permission)}</Badge>
          <Button size="sm" variant="secondary" onclick={() => kick(guest)}>Kick<span class="visually-hidden"> {guest.name}</span></Button>
        </li>
      {/each}
      {#each invites as invite (invite.id)}
        {@const joined = joinedWith(invite)}
        <li>
          <span class="name">Link</span>
          {#if invite.internet}<Badge tone="accent">Internet</Badge>{/if}
          <Badge>{permissionLabel(invite.permission)}</Badge>
          <span class="muted when">{invite.expires <= now ? 'Expired' : `Expires ${expiresIn(invite.expires, now)}`}{joined ? ` · ${joined} watching` : ''}</span>
          <Button size="sm" variant="danger" onclick={() => revoke(invite)}>Revoke</Button>
        </li>
      {/each}
    </ul>
  {/if}
</Panel>

<Dialog bind:open title={created ? 'Invite link' : 'Invite a guest'} dismissible={!creating}>
  {#if created}
    <div class="form">
      <Field label="Link" id="guest-link">
        <div class="copy">
          <Input id="guest-link" value={link} mono readonly />
          <CopyButton value={link} label="Copy link" what="the invite link" showLabel />
        </div>
      </Field>
      <p class="muted when">{permissionLabel(created.invite.permission)} · expires {expiresIn(created.invite.expires)}</p>
    </div>
  {:else}
    <div class="form">
      <Field
        label="Guests can"
        id="guest-permission"
        hint={permission === 'controller-keyboard-mouse'
          ? 'Their keyboard and mouse control this PC, outside the game too. Anyone who gets the link, or reads it past a certificate warning, can use it.'
          : undefined}
      >
        <Select id="guest-permission" bind:value={permission} options={[...PERMISSIONS]} />
      </Field>
      <Field label="Link expires after" id="guest-expiry">
        <Select id="guest-expiry" bind:value={expiry} options={[...EXPIRIES]} />
      </Field>
      {#if expiry === 'custom'}
        <div class="custom">
          <Field label="Amount" id="guest-amount" error={seconds === null ? '5 minutes to 7 days' : undefined}>
            <Input id="guest-amount" type="number" min="1" step="1" bind:value={amount} invalid={seconds === null} />
          </Field>
          <Field label="Unit" id="guest-unit">
            <Select id="guest-unit" bind:value={unit} options={[...UNITS]} />
          </Field>
        </div>
      {/if}
      <Field label="Guests are" id="guest-audience">
        <Select id="guest-audience" value={audience} options={[...AUDIENCES]} onchange={chooseAudience} />
      </Field>
      {#if internet}
        <InternetReach {reach} ports={portNames} />
      {/if}
      <Field
        label="Address"
        id="guest-address"
        error={addressError}
        hint={internet && reach?.address
          ? typed && address.trim() !== reach.address
            ? `Your own address. Clear it and the link uses ${reach.address}, which Rubylight found.`
            : 'Found automatically. Type your own address or DNS name to use that instead.'
          : undefined}
      >
        <Input
          id="guest-address"
          bind:value={address}
          oninput={() => {
            typed = !!address.trim();
            if (!typed && internet && reach?.address) address = reach.address;
          }}
          mono
          invalid={!!addressError}
          spellcheck="false"
          autocomplete="off"
        />
      </Field>
    </div>
  {/if}
  {#snippet actions()}
    {#if created}
      <Button variant="primary" onclick={done}>Done</Button>
    {:else}
      <Button variant="secondary" disabled={creating} onclick={() => (open = false)}>Cancel</Button>
      <Button variant="primary" icon="link" busy={creating} disabled={seconds === null || !!addressError || addressMissing} onclick={create}>Create link</Button>
    {/if}
  {/snippet}
</Dialog>

<style>
  .row {
    margin: 0;
    padding: var(--space-3) var(--space-4);
  }
  .list {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .list li {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: var(--space-3);
    padding: var(--space-3) var(--space-4);
    border-top: 1px solid var(--line);
  }
  .list li:first-child {
    border-top: 0;
  }
  .name {
    flex: 1;
    min-width: 0;
    font-weight: var(--weight-semibold);
    overflow-wrap: anywhere;
  }
  .when {
    font-size: var(--text-sm);
  }
  .form {
    display: grid;
    gap: var(--space-4);
  }
  .form p {
    margin: 0;
  }
  .custom {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: var(--space-3);
  }
  .copy {
    display: flex;
    gap: var(--space-2);
    align-items: center;
  }
  .copy :global(input) {
    flex: 1;
    min-width: 0;
  }
</style>
