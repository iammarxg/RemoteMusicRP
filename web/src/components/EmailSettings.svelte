<!-- SPDX-License-Identifier: AGPL-3.0-only -->
<script>
  import { onMount } from 'svelte';
  import { slide } from 'svelte/transition';
  import { api } from '../lib/api.js';
  import { t } from '../lib/i18n.js';
  import { toast } from '../lib/stores.svelte.js';
  import Select from './Select.svelte';
  import Toggle from './Toggle.svelte';

  let e = $state(null);
  let hasSecret = $state(false);
  let savedProvider = $state('none');
  let secret = $state('');
  let busy = $state(false);
  let error = $state('');

  function load(r) {
    e = r.settings;
    hasSecret = r.has_secret;
    savedProvider = r.settings.provider;
    secret = '';
  }
  onMount(async () => {
    try {
      load(await api.get('/api/email'));
    } catch (err) {
      error = err.message;
    }
  });

  // Typical ports when switching the security mode.
  let lastSec = null;
  $effect(() => {
    if (!e) return;
    if (lastSec && lastSec !== e.smtp_security) {
      if (e.smtp_security === 'tls' && e.smtp_port === 587) e.smtp_port = 465;
      if (e.smtp_security === 'starttls' && e.smtp_port === 465) e.smtp_port = 587;
    }
    lastSec = e.smtp_security;
  });

  let keepSecret = $derived(hasSecret && e?.provider === savedProvider);

  async function save(ev) {
    ev.preventDefault();
    busy = true;
    error = '';
    try {
      load(await api.put('/api/email', { settings: { ...e, smtp_port: Number(e.smtp_port) || 0 }, secret }));
      toast(t('common.saved'));
    } catch (err) {
      error = err.message;
    } finally {
      busy = false;
    }
  }
  async function test() {
    busy = true;
    error = '';
    try {
      await api.post('/api/email/test');
      toast(t('email.test_sent', { to: e.to }));
    } catch (err) {
      error = err.message;
    } finally {
      busy = false;
    }
  }
</script>

<section class="panel stack">
  <div>
    <h2>{t('email.title')}</h2>
    <p class="muted small">{t('email.intro')}</p>
  </div>
  {#if !e}
    {#if error}<p class="error-text" role="alert">{error}</p>{:else}<div class="skeleton sk"></div>{/if}
  {:else}
    <form class="stack" onsubmit={save}>
      <div class="field">
        <span>{t('email.provider')}</span>
        <Select
          bind:value={e.provider}
          label={t('email.provider')}
          options={[
            { value: 'none', label: t('email.provider_none') },
            { value: 'resend', label: 'Resend' },
            { value: 'smtp', label: t('email.provider_smtp') },
          ]}
        />
      </div>
      {#if e.provider !== 'none'}
        <div class="stack" transition:slide={{ duration: 160 }}>
          <div class="two">
            <label class="field">
              {t('email.from')}
              <span class="hint">{t(e.provider === 'resend' ? 'email.from_hint_resend' : 'email.from_hint')}</span>
              <input type="email" bind:value={e.from} maxlength="320" required placeholder="alerts@example.com" />
            </label>
            <label class="field">
              {t('email.to')}
              <span class="hint">{t('email.to_hint')}</span>
              <input type="email" bind:value={e.to} maxlength="320" required placeholder="you@example.com" />
            </label>
          </div>
          {#if e.provider === 'smtp'}
            <div class="two">
              <label class="field">
                {t('email.host')}
                <input bind:value={e.smtp_host} maxlength="253" required placeholder="smtp.example.com" spellcheck="false" />
              </label>
              <div class="two inner">
                <div class="field">
                  <span>{t('email.security')}</span>
                  <Select
                    bind:value={e.smtp_security}
                    label={t('email.security')}
                    options={[
                      { value: 'starttls', label: 'STARTTLS' },
                      { value: 'tls', label: 'TLS' },
                    ]}
                  />
                </div>
                <label class="field">
                  {t('email.port')}
                  <input type="number" bind:value={e.smtp_port} min="1" max="65535" required />
                </label>
              </div>
            </div>
            <p class="muted small">{t('email.tls_note')}</p>
            <label class="field">
              {t('email.username')}
              <input bind:value={e.smtp_username} maxlength="256" autocomplete="off" spellcheck="false" />
            </label>
          {/if}
          <label class="field">
            {t(e.provider === 'resend' ? 'email.api_key' : 'email.password')}
            {#if e.provider === 'resend'}
              <span class="hint">{t('email.api_key_hint')}
                <a href="https://resend.com/api-keys" target="_blank" rel="noopener noreferrer">resend.com/api-keys</a></span>
            {/if}
            <input
              type="password"
              bind:value={secret}
              maxlength="256"
              autocomplete="new-password"
              placeholder={keepSecret ? t('email.secret_saved') : ''}
            />
          </label>
          <Toggle bind:checked={e.alert_lastfm} label={t('email.alert_lastfm')} hint={t('email.alert_lastfm_hint')} />
        </div>
      {/if}
      {#if error}<p class="error-text" role="alert">{error}</p>{/if}
      <div class="row">
        <button class="primary" type="submit" disabled={busy}>{t('common.save')}</button>
        {#if savedProvider !== 'none'}
          <button type="button" disabled={busy} onclick={test}>{t('email.test')}</button>
        {/if}
      </div>
    </form>
  {/if}
</section>

<style>
  .sk {
    block-size: 6rem;
  }
  .two {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 0.75rem;
    align-items: end;
  }
  .two.inner {
    grid-template-columns: 1.3fr 1fr;
  }
  div.field {
    display: grid;
    gap: 0.4rem;
    font-weight: 550;
    font-size: var(--step--1);
  }
  @media (max-width: 40rem) {
    .two {
      grid-template-columns: 1fr;
    }
  }
</style>
