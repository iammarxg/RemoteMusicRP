<!-- SPDX-License-Identifier: AGPL-3.0-only -->
<!-- EXPERIMENTAL server mode: sign in with Discord through the Social SDK. -->
<script>
  import { onMount, onDestroy } from 'svelte';
  import { slide } from 'svelte/transition';
  import { api } from '../lib/api.js';
  import { t, tl } from '../lib/i18n.js';
  import { toast } from '../lib/stores.svelte.js';

  let { hasAppId = false, onChange = () => {} } = $props();

  let s = $state(null);
  let busy = $state(false);
  let error = $state('');
  let showPaste = $state(false);
  let pasted = $state('');
  let timer;

  async function load() {
    try {
      s = await api.get('/api/discord/sdk');
    } catch (e) {
      error = e.message;
    }
  }
  onMount(() => {
    load();
    timer = setInterval(load, 4000);
  });
  onDestroy(() => clearInterval(timer));

  async function signIn() {
    busy = true;
    error = '';
    try {
      const r = await api.post('/api/discord/sdk/begin');
      location.href = r.url; // Discord sends the browser back to /discord-callback
    } catch (e) {
      error = e.message;
      busy = false;
    }
  }

  async function finishPasted(e) {
    e.preventDefault();
    error = '';
    let code = '';
    let state = '';
    try {
      const u = new URL(pasted.trim());
      code = u.searchParams.get('code') ?? '';
      state = u.searchParams.get('state') ?? '';
    } catch {
      error = t('sdk.paste_invalid');
      return;
    }
    if (!code || !state) {
      error = t('sdk.paste_invalid');
      return;
    }
    busy = true;
    try {
      await api.post('/api/discord/sdk/finish', { code, state });
      toast(t('sdk.signed_in'));
      pasted = '';
      showPaste = false;
      await load();
      onChange();
    } catch (err) {
      error = err.message;
    } finally {
      busy = false;
    }
  }

  async function signOut() {
    if (!confirm(t('sdk.sign_out_confirm'))) return;
    busy = true;
    try {
      await api.post('/api/discord/sdk/sign-out');
      toast(t('sdk.signed_out'));
      await load();
      onChange();
    } catch (e) {
      error = e.message;
    } finally {
      busy = false;
    }
  }

  const ONLINE = ['online', 'idle', 'dnd', 'invisible'];

  async function setOnline(status) {
    if (s?.online_status === status) return;
    busy = true;
    error = '';
    try {
      await api.post('/api/discord/sdk/online-status', { status });
      s = { ...s, online_status: status };
      toast(t('sdk.online_changed'));
    } catch (e) {
      error = e.message;
    } finally {
      busy = false;
    }
  }

  async function copy(text) {
    try {
      await navigator.clipboard.writeText(text);
      toast(t('common.copied'));
    } catch {
      /* clipboard blocked: the text is selectable */
    }
  }
</script>

<div class="stack">
  <div class="warn small" role="note">
    <strong>{t('sdk.experimental')}</strong>
    <ul>{#each tl('sdk.risks') as r, i (i)}<li>{r}</li>{/each}</ul>
  </div>

  {#if s}
    <p class="small status">
      {#if s.signed_in && s.ready}
        <span class="dot ok"></span>{t('sdk.state_ready')}
      {:else if s.signed_in}
        <span class="dot warn"></span>{t('sdk.state_connecting', { status: s.status ?? '…' })}
      {:else}
        <span class="dot"></span>{t('sdk.state_signed_out')}
      {/if}
    </p>
    {#if s.error && !s.ready}<p class="muted small">{s.error}</p>{/if}

    {#if !s.signed_in}
      <ol class="guide small">
        {#each tl('sdk.steps') as l, i (i)}<li>{l}</li>{/each}
      </ol>
      <div class="redirect">
        <span class="small muted">{t('sdk.redirect')}</span>
        <code class="mono">{s.redirect_uri}</code>
        <button type="button" class="ghost" onclick={() => copy(s.redirect_uri)}>{t('common.copy')}</button>
      </div>
    {/if}

    <div class="row">
      {#if !s.signed_in}
        <button class="primary" disabled={busy || !hasAppId} onclick={signIn}>{t('sdk.sign_in')}</button>
        <button type="button" class="ghost" onclick={() => (showPaste = !showPaste)}>{t('sdk.paste_toggle')}</button>
      {:else}
        <button disabled={busy} onclick={signIn}>{t('sdk.sign_in_again')}</button>
        <button class="danger" disabled={busy} onclick={signOut}>{t('sdk.sign_out')}</button>
      {/if}
    </div>
    {#if !hasAppId}<p class="muted small">{t('sdk.need_app_id')}</p>{/if}

    {#if s.signed_in && s.ready}
      <div class="stack online" transition:slide={{ duration: 160 }}>
        <div>
          <strong class="small">{t('sdk.online_title')}</strong>
          <p class="muted small">{t('sdk.online_hint')}</p>
        </div>
        <div class="seg" role="group" aria-label={t('sdk.online_label')}>
          {#each ONLINE as o (o)}
            <button
              type="button"
              aria-pressed={s.online_status === o}
              class:on={s.online_status === o}
              disabled={busy}
              onclick={() => setOnline(o)}>{t(`preview.status_${o}`)}</button
            >
          {/each}
        </div>
        {#if !s.online_status}<p class="muted small">{t('sdk.online_unknown')}</p>{/if}
      </div>
    {/if}

    {#if showPaste}
      <form class="stack" onsubmit={finishPasted} transition:slide={{ duration: 160 }}>
        <p class="muted small">{t('sdk.paste_hint')}</p>
        <label class="field">
          {t('sdk.paste_label')}
          <input bind:value={pasted} spellcheck="false" autocomplete="off" placeholder="http://…/discord-callback?code=…&state=…" />
        </label>
        <div><button disabled={busy || !pasted.trim()}>{t('sdk.paste_submit')}</button></div>
      </form>
    {/if}
  {:else if !error}
    <div class="skeleton sk"></div>
  {/if}
  {#if error}<p class="error-text" role="alert">{error}</p>{/if}
</div>

<style>
  .warn {
    border: 1px solid var(--line-strong);
    border-radius: var(--radius-m);
    padding: 0.75rem 0.9rem;
  }
  .warn ul {
    margin: 0.4rem 0 0;
    padding-inline-start: 1.1rem;
    color: var(--muted);
    display: grid;
    gap: 0.2rem;
  }
  .status {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    font-weight: 600;
  }
  .guide {
    margin: 0;
    padding-inline-start: 1.2rem;
    color: var(--muted);
    display: grid;
    gap: 0.25rem;
  }
  .redirect {
    display: flex;
    align-items: center;
    gap: 0.6rem;
    flex-wrap: wrap;
  }
  .redirect code {
    background: var(--surface-2);
    border-radius: var(--radius-s);
    padding: 0.25rem 0.5rem;
    overflow-wrap: anywhere;
    font-size: var(--step--1);
  }
  .sk {
    block-size: 5rem;
  }
  .online {
    border-block-start: 1px solid var(--line);
    padding-block-start: 0.9rem;
  }
  .online p {
    margin: 0.2rem 0 0;
  }
  .seg {
    display: flex;
    flex-wrap: wrap;
    gap: 0.4rem;
  }
  .seg button.on {
    background: var(--text);
    color: var(--bg);
    border-color: var(--text);
  }
</style>
