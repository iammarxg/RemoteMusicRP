<!-- SPDX-License-Identifier: AGPL-3.0-only -->
<script>
  import { onMount } from 'svelte';
  import { slide } from 'svelte/transition';
  import { api } from '../lib/api.js';
  import { t, tl } from '../lib/i18n.js';
  import { toast } from '../lib/stores.svelte.js';
  import DiscordSdkPanel from '../components/DiscordSdkPanel.svelte';

  let { onChange } = $props();
  let c = $state(null);
  let error = $state('');
  let editing = $state(null); // 'lastfm' | 'discord'
  let busy = $state(false);
  let lfUser = $state('');
  let lfKey = $state('');
  let clientId = $state('');
  let sdkMode = $derived(c?.discord.mode === 'social_sdk');

  async function load() {
    try {
      c = await api.get('/api/connections');
    } catch (e) {
      error = e.message;
    }
  }
  onMount(load);

  async function act(fn, done) {
    busy = true;
    error = '';
    try {
      await fn();
      editing = null;
      toast(done);
      await load();
      await onChange?.();
    } catch (e) {
      error = e.message;
    } finally {
      busy = false;
    }
  }

  const saveLastfm = (e) => {
    e.preventDefault();
    act(() => api.post('/api/connections/lastfm', { username: lfUser.trim(), api_key: lfKey.trim() }), t('common.saved'));
  };
  const saveDiscord = (e) => {
    e.preventDefault();
    act(() => api.post('/api/connections/discord', { client_id: clientId.trim() }), t('common.saved'));
  };
  const disconnect = (which) => {
    if (!confirm(t('connections.disconnect_confirm'))) return;
    act(() => api.del(`/api/connections/${which}`), t('connections.disconnected'));
  };
</script>

<div class="stack page">
  <h1>{t('connections.title')}</h1>
  {#if error}<p class="error-text" role="alert">{error}</p>{/if}

  {#if !c}
    <div class="skeleton sk"></div>
  {:else}
    <section class="panel stack">
      <div class="spread">
        <div class="row">
          <span class="dot" class:ok={c.discord.connected} class:warn={c.discord.client_id && !c.discord.connected}></span>
          <h2>{t('connections.discord')}</h2>
        </div>
        <div class="row">
          <button onclick={() => (editing = editing === 'discord' ? null : 'discord')}>
            {c.discord.client_id ? t('connections.change') : t('connections.connect')}
          </button>
          {#if c.discord.client_id}
            <button class="danger" disabled={busy} onclick={() => disconnect('discord')}>{t('common.disconnect')}</button>
          {/if}
        </div>
      </div>
      <p class="muted">{t(sdkMode ? 'connections.discord_mode_sdk' : 'connections.discord_mode')}</p>
      {#if c.discord.client_id && !sdkMode}
        <p class="small mono">{t('connections.discord_app', { id: c.discord.client_id })}</p>
        <p class="small">
          {#if c.discord.connected}
            {c.discord.user ? t('dashboard.discord_ok', { user: c.discord.user }) : t('dashboard.discord_ok_anon')}
          {:else}
            <span class="muted">{c.discord.error ?? t('dashboard.discord_off')}</span>
          {/if}
        </p>
      {/if}
      {#if editing === 'discord'}
        <form class="stack" onsubmit={saveDiscord} transition:slide={{ duration: 180 }}>
          <ol class="guide small">{#each tl(sdkMode ? 'wizard.discord_steps_sdk' : 'wizard.discord_steps') as l, i (i)}<li>{l}</li>{/each}</ol>
          <a href="https://discord.com/developers/applications" target="_blank" rel="noopener noreferrer">{t('wizard.discord_portal')}</a>
          <label class="field">
            {t('wizard.discord_id')}
            <input bind:value={clientId} inputmode="numeric" class="mono" required />
          </label>
          <div class="row">
            <button class="primary" disabled={busy || !/^\d{17,20}$/.test(clientId.trim())}>{t('common.save')}</button>
            <button type="button" class="ghost" onclick={() => (editing = null)}>{t('common.cancel')}</button>
          </div>
        </form>
      {/if}
      {#if sdkMode}
        {#if c.discord.client_id}<p class="small mono">{t('connections.discord_app', { id: c.discord.client_id })}</p>{/if}
        <DiscordSdkPanel hasAppId={!!c.discord.client_id} onChange={load} />
      {/if}
    </section>

    <section class="panel stack">
      <div class="spread">
        <div class="row">
          <span class="dot" class:ok={c.lastfm.connected}></span>
          <h2>{t('connections.lastfm')}</h2>
        </div>
        <div class="row">
          <button onclick={() => (editing = editing === 'lastfm' ? null : 'lastfm')}>
            {c.lastfm.connected ? t('connections.change') : t('connections.connect')}
          </button>
          {#if c.lastfm.connected}
            <button class="danger" disabled={busy} onclick={() => disconnect('lastfm')}>{t('common.disconnect')}</button>
          {/if}
        </div>
      </div>
      <p class="muted">
        {c.lastfm.connected ? t('connections.lastfm_as', { user: c.lastfm.username }) : t('common.not_connected')}
      </p>
      {#if editing === 'lastfm'}
        <form class="stack" onsubmit={saveLastfm} transition:slide={{ duration: 180 }}>
          <ol class="guide small">{#each tl('wizard.lastfm_steps') as l, i (i)}<li>{l}</li>{/each}</ol>
          <a href="https://www.last.fm/api/account/create" target="_blank" rel="noopener noreferrer">{t('wizard.lastfm_portal')}</a>
          <label class="field">{t('wizard.lastfm_user')}<input bind:value={lfUser} required /></label>
          <label class="field">
            {t('wizard.lastfm_key')}
            <input type="password" bind:value={lfKey} class="mono" autocomplete="off" required />
          </label>
          <div class="row">
            <button class="primary" disabled={busy || !lfUser || lfKey.trim().length !== 32}>
              {busy ? t('wizard.checking') : t('common.save')}
            </button>
            <button type="button" class="ghost" onclick={() => (editing = null)}>{t('common.cancel')}</button>
          </div>
        </form>
      {/if}
    </section>
  {/if}
</div>

<style>
  .page {
    gap: 1.5rem;
    max-width: 46rem;
  }
  .sk {
    block-size: 14rem;
  }
  .guide {
    margin: 0;
    padding-inline-start: 1.2rem;
    color: var(--muted);
    display: grid;
    gap: 0.25rem;
  }
</style>
