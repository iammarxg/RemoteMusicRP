<!-- SPDX-License-Identifier: AGPL-3.0-only -->
<script>
  import { onMount } from 'svelte';
  import { api } from '../lib/api.js';
  import { t } from '../lib/i18n.js';
  import ThemeSwitch from '../components/ThemeSwitch.svelte';
  import EmailSettings from '../components/EmailSettings.svelte';
  import Backup from '../components/Backup.svelte';

  let { boot, onChange = () => {} } = $props();
  import Toggle from '../components/Toggle.svelte';
  import { ago } from '../lib/i18n.js';
  import { toast } from '../lib/stores.svelte.js';

  let up = $state(null);
  let upBusy = $state(false);
  let auto = $state(false);

  async function loadUpdates() {
    try {
      up = await api.get('/api/updates');
      auto = up.auto_install;
    } catch {
      up = null;
    }
  }
  async function checkNow() {
    upBusy = true;
    try {
      up.info = await api.post('/api/updates/check');
    } catch (e) {
      toast(e.message, 'error');
    } finally {
      upBusy = false;
    }
  }
  async function installNow() {
    upBusy = true;
    try {
      const r = await api.post('/api/updates/install');
      toast(t('updates.restarting', { v: r.installed }));
      setTimeout(() => location.reload(), 8000);
    } catch (e) {
      toast(e.message, 'error');
      upBusy = false;
    }
  }
  async function setAuto(v) {
    try {
      await api.put('/api/updates/settings', { auto_install: v });
    } catch (e) {
      toast(e.message, 'error');
    }
  }
  $effect(() => {
    if (up && auto !== up.auto_install) {
      up.auto_install = auto;
      setAuto(auto);
    }
  });

  let s = $state(null);
  let error = $state('');
  onMount(async () => {
    try {
      s = await api.get('/api/settings');
      loadUpdates();
    } catch (e) {
      error = e.message;
    }
  });
</script>

<div class="stack page">
  <header class="stack head">
    <h1>{t('settings.title')}</h1>
    <p class="muted">{t('settings.intro')}</p>
  </header>
  <section class="panel spread">
    <div>
      <h2>{t('theme.appearance')}</h2>
      <p class="muted small">{t('theme.appearance_hint')}</p>
    </div>
    <ThemeSwitch />
  </section>
  {#if up}
    {@const i = up.info}
    <section class="panel stack">
      <div class="spread">
        <h2>{t('updates.title')}</h2>
        {#if up.check_enabled}
          <button disabled={upBusy} onclick={checkNow}>{t('updates.check')}</button>
        {/if}
      </div>
      <p>
        {#if i.available && i.security}<strong>{t('updates.security', { v: i.latest })}</strong>
        {:else if i.available}{t('updates.available', { v: i.latest })}
        {:else if i.error}{t('updates.check_failed')}
        {:else if i.checked_at}{t('updates.up_to_date')}
        {:else}{t('updates.current', { v: i.current })}{/if}
      </p>
      <p class="muted small">
        {t('updates.current', { v: i.current })}
        {i.checked_at ? t('updates.checked', { time: ago(i.checked_at) }) : t('updates.never_checked')}
        {#if i.error}<span> {i.error}</span>{/if}
      </p>
      {#if !up.check_enabled}<p class="muted small">{t('updates.disabled')}</p>{/if}
      {#if i.install_kind !== 'portable'}
        <p class="muted small">{t('updates.managed')}</p>
      {:else if !i.can_install}
        <p class="muted small">{t('updates.no_key')}</p>
      {/if}
      <div class="row">
        {#if i.available && i.can_install}
          <button class="primary" disabled={upBusy || i.installing} onclick={installNow}>
            {upBusy || i.installing ? t('updates.installing') : t('updates.install')}
          </button>
        {/if}
        {#if i.notes_url}<a href={i.notes_url} target="_blank" rel="noopener noreferrer">{t('updates.notes')}</a>{/if}
      </div>
      {#if i.can_install}
        <Toggle bind:checked={auto} label={t('updates.auto')} hint={t('updates.auto_hint')} />
      {/if}
    </section>
  {/if}
  <EmailSettings />
  <Backup passwordSet={boot?.setup?.password_set} onImported={onChange} />
  {#if error}<p class="error-text" role="alert">{error}</p>{/if}
  {#if !s}
    <div class="skeleton sk"></div>
  {:else}
    <section class="panel stack">
      <div>
        <h2>{t('settings.instance')}</h2>
        <p class="muted small">{t('settings.instance_hint')}</p>
      </div>
      <dl>
        <dt>{t('settings.bind')}</dt>
        <dd class="mono">{s.bind}</dd>
        {#if s.public_origin}
          <dt>{t('settings.public_origin')}</dt>
          <dd class="mono">{s.public_origin}</dd>
        {/if}
        {#if s.lan_access}
          <dt>{t('settings.lan_access')}</dt>
          <dd>
            {#if s.lan_url}<span class="mono">{s.lan_url}</span><br />{/if}
            <span class="muted small">{t('settings.lan_note')}</span>
          </dd>
        {/if}
        <dt>{t('settings.poll')}</dt>
        <dd>{t('settings.seconds', { n: s.poll_interval_secs })}</dd>
        <dt>{t('settings.retention')}</dt>
        <dd>{t('settings.days', { n: s.log_retention_days })}</dd>
        <dt>{t('settings.updates')}</dt>
        <dd>{s.update_check ? t('common.on') : t('common.off')}</dd>
        <dt>{t('settings.artwork')}</dt>
        <dd>
          {[s.artwork.apple && t('settings.art_apple'), s.artwork.cover_art_archive && t('settings.art_caa')]
            .filter(Boolean)
            .join(', ') || t('common.off')}
          <span class="muted">
            ({s.artwork.apple_first ? t('settings.art_order_apple') : t('settings.art_order_caa')})
          </span>
        </dd>
        <dt>{t('settings.country')}</dt>
        <dd class="mono">{s.artwork.country}</dd>
        <dt>{t('settings.data_dir')}</dt>
        <dd class="mono path">{s.data_dir}</dd>
      </dl>
    </section>
  {/if}
</div>

<style>
  .page {
    gap: 1.5rem;
    max-width: 46rem;
  }
  .head {
    gap: 0.4rem;
  }
  .sk {
    block-size: 18rem;
  }
  dl {
    margin: 0;
    display: grid;
    grid-template-columns: minmax(10rem, max-content) 1fr;
    gap: 0.9rem 1.5rem;
  }
  dt {
    color: var(--muted);
    font-weight: 600;
  }
  dd {
    margin: 0;
  }
  .path {
    overflow-wrap: anywhere;
  }
  @media (max-width: 34rem) {
    dl {
      grid-template-columns: 1fr;
      gap: 0.2rem;
    }
    dd {
      margin-block-end: 0.7rem;
    }
  }
</style>
