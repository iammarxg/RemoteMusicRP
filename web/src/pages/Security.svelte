<!-- SPDX-License-Identifier: AGPL-3.0-only -->
<script>
  import { onMount } from 'svelte';
  import { slide } from 'svelte/transition';
  import qrcode from 'qrcode-generator';
  import { api } from '../lib/api.js';
  import { t, ago } from '../lib/i18n.js';
  import { toast } from '../lib/stores.svelte.js';

  let { onChange } = $props();
  let sec = $state(null);
  let error = $state('');
  let busy = $state(false);

  let current = $state('');
  let next = $state('');

  let totpSetup = $state(null); // { secret, otpauth_uri, qr }
  let totpCode = $state('');
  let codes = $state(null); // recovery codes shown once
  let confirmPw = $state('');
  let showConfirm = $state(null); // 'disable' | 'recovery'

  let deleteText = $state('');
  let deletePw = $state('');

  async function load() {
    try {
      sec = await api.get('/api/security');
    } catch (e) {
      error = e.message;
    }
  }
  onMount(load);

  async function act(fn) {
    busy = true;
    error = '';
    try {
      return await fn();
    } catch (e) {
      error = e.message;
    } finally {
      busy = false;
    }
  }

  const changePassword = (e) => {
    e.preventDefault();
    act(async () => {
      await api.post('/api/security/password', { current: current || null, new: next });
      current = next = '';
      toast(t('security.changed'));
      await load();
      await onChange?.();
    });
  };

  const beginTotp = () =>
    act(async () => {
      const r = await api.post('/api/security/totp/begin');
      const qr = qrcode(0, 'M');
      qr.addData(r.otpauth_uri);
      qr.make();
      // SVG markup generated locally from our own data (no scripts).
      totpSetup = { ...r, qr: qr.createSvgTag({ cellSize: 5, margin: 2, scalable: true }) };
    });

  const confirmTotp = (e) => {
    e.preventDefault();
    act(async () => {
      const r = await api.post('/api/security/totp/confirm', { code: totpCode.trim() });
      codes = r.recovery_codes;
      totpSetup = null;
      totpCode = '';
      await load();
    });
  };

  const withPassword = (e) => {
    e.preventDefault();
    const which = showConfirm;
    act(async () => {
      if (which === 'disable') {
        await api.post('/api/security/totp/disable', { password: confirmPw });
        toast(t('security.totp_disabled'));
      } else {
        const r = await api.post('/api/security/recovery', { password: confirmPw });
        codes = r.recovery_codes;
      }
      confirmPw = '';
      showConfirm = null;
      await load();
    });
  };

  const logoutOthers = () =>
    act(async () => {
      const r = await api.del('/api/security/sessions');
      toast(t('security.logged_out', { n: r.logged_out }));
      await load();
    });

  const rotate = () =>
    act(async () => {
      await api.post('/api/security/rotate-keys');
      toast(t('security.rotated'));
    });

  const deleteAll = (e) => {
    e.preventDefault();
    act(async () => {
      await api.post('/api/danger/delete-everything', { confirm: deleteText, password: deletePw || null });
      toast(t('security.deleted'));
      location.hash = '#/dashboard';
      await onChange?.();
    });
  };

  async function copyCodes() {
    await navigator.clipboard?.writeText(codes.join('\n'));
    toast(t('common.copied'));
  }
</script>

<div class="stack page">
  <h1>{t('security.title')}</h1>
  {#if error}<p class="error-text" role="alert">{error}</p>{/if}

  {#if codes}
    <section class="panel stack codes" transition:slide={{ duration: 180 }} aria-live="assertive">
      <h2>{t('security.recovery_title')}</h2>
      <p>{t('security.recovery_body')}</p>
      <ul class="mono">{#each codes as c (c)}<li>{c}</li>{/each}</ul>
      <div class="row">
        <button onclick={copyCodes}>{t('common.copy')}</button>
        <button class="primary" onclick={() => (codes = null)}>{t('security.recovery_done')}</button>
      </div>
    </section>
  {/if}

  {#if !sec}
    <div class="skeleton sk"></div>
  {:else}
    <section class="panel stack">
      <h2>{t('security.password')}</h2>
      <p class="muted">{sec.password_set ? t('security.password_set') : t('security.password_none')}</p>
      <form class="stack form" onsubmit={changePassword}>
        {#if sec.password_set}
          <label class="field">
            {t('security.current')}
            <input type="password" bind:value={current} autocomplete="current-password" required />
          </label>
        {/if}
        <label class="field">
          {t('security.new')} <span class="hint">{t('wizard.password_hint')}</span>
          <input type="password" bind:value={next} autocomplete="new-password" minlength="12" required />
        </label>
        <div>
          <button class="primary" disabled={busy || next.length < 12}>
            {sec.password_set ? t('security.change') : t('security.set')}
          </button>
        </div>
      </form>
    </section>

    <section class="panel stack">
      <h2>{t('security.totp')}</h2>
      {#if sec.totp_enabled}
        <p class="muted">{t('security.totp_on')}</p>
        <p class="small">{t('security.recovery_left', { n: sec.recovery_remaining })}</p>
        <div class="row">
          <button onclick={() => (showConfirm = 'recovery')}>{t('security.recovery_new')}</button>
          <button class="danger" onclick={() => (showConfirm = 'disable')}>{t('security.totp_disable')}</button>
        </div>
        {#if showConfirm}
          <form class="stack form" onsubmit={withPassword} transition:slide={{ duration: 160 }}>
            <label class="field">
              {t('security.confirm_password')}
              <input type="password" bind:value={confirmPw} autocomplete="current-password" required />
            </label>
            <div class="row">
              <button class={showConfirm === 'disable' ? 'danger' : 'primary'} disabled={busy || !confirmPw}>
                {showConfirm === 'disable' ? t('security.totp_disable') : t('security.recovery_new')}
              </button>
              <button type="button" class="ghost" onclick={() => (showConfirm = null)}>{t('common.cancel')}</button>
            </div>
          </form>
        {/if}
      {:else if !sec.password_set}
        <p class="muted">{t('security.totp_off')} {t('security.totp_needs_password')}</p>
      {:else if totpSetup}
        <p>{t('security.totp_scan')}</p>
        <div class="totp">
          <div class="qr">{@html totpSetup.qr}</div>
          <div class="stack">
            <p class="small muted">{t('security.totp_key')}</p>
            <p class="mono key">{totpSetup.secret}</p>
            <form class="stack" onsubmit={confirmTotp}>
              <label class="field">
                {t('security.totp_confirm')}
                <input bind:value={totpCode} inputmode="numeric" autocomplete="one-time-code" maxlength="6"
                  class="mono" required />
              </label>
              <div class="row">
                <button class="primary" disabled={busy || totpCode.trim().length !== 6}>{t('security.totp_verify')}</button>
                <button type="button" class="ghost" onclick={() => (totpSetup = null)}>{t('common.cancel')}</button>
              </div>
            </form>
          </div>
        </div>
      {:else}
        <p class="muted">{t('security.totp_off')}</p>
        <div><button class="primary" disabled={busy} onclick={beginTotp}>{t('security.totp_enable')}</button></div>
      {/if}
    </section>

    {#if sec.sessions.length}
      <section class="panel stack">
        <div class="spread">
          <h2>{t('security.sessions')}</h2>
          {#if sec.sessions.length > 1}
            <button disabled={busy} onclick={logoutOthers}>{t('security.logout_others')}</button>
          {/if}
        </div>
        <ul class="sessions">
          {#each sec.sessions as s, i (i)}
            <li>
              <span>{s.label}</span>
              <span class="muted small">
                {s.current ? t('security.this_device') : t('security.last_active', { time: ago(s.last_seen) })}
              </span>
            </li>
          {/each}
        </ul>
      </section>
    {/if}

    <section class="panel stack">
      <h2>{t('security.keys')}</h2>
      <p class="muted">{t('security.keys_body', { where: sec.key_store })}</p>
      <div><button disabled={busy} onclick={rotate}>{t('security.rotate')}</button></div>
    </section>

    <section class="panel stack danger-zone">
      <h2>{t('security.danger')}</h2>
      <p>{t('security.danger_body')}</p>
      <form class="stack form" onsubmit={deleteAll}>
        <label class="field">
          {t('security.danger_type')}
          <input bind:value={deleteText} autocomplete="off" class="mono" />
        </label>
        {#if sec.password_set}
          <label class="field">
            {t('security.confirm_password')}
            <input type="password" bind:value={deletePw} autocomplete="current-password" />
          </label>
        {/if}
        <div>
          <button class="danger" disabled={busy || deleteText !== 'DELETE' || (sec.password_set && !deletePw)}>
            {t('security.danger_button')}
          </button>
        </div>
      </form>
    </section>
  {/if}
</div>

<style>
  .page {
    gap: 1.5rem;
    max-width: 46rem;
  }
  .sk {
    block-size: 24rem;
  }
  .form {
    max-width: 24rem;
  }
  .totp {
    display: grid;
    grid-template-columns: 11rem 1fr;
    gap: 1.5rem;
    align-items: start;
  }
  .qr {
    background: #fff;
    border-radius: var(--radius-m);
    padding: 0.5rem;
    line-height: 0;
  }
  .qr :global(svg) {
    inline-size: 100%;
    block-size: auto;
  }
  .key {
    overflow-wrap: anywhere;
    font-size: var(--step-1);
  }
  .codes {
    border-color: var(--accent);
  }
  .codes ul {
    list-style: none;
    padding: 0;
    margin: 0;
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(12rem, 1fr));
    gap: 0.4rem 1rem;
    font-size: var(--step-1);
  }
  .sessions {
    list-style: none;
    padding: 0;
    margin: 0;
    display: grid;
  }
  .sessions li {
    display: flex;
    justify-content: space-between;
    gap: 1rem;
    padding-block: 0.6rem;
    border-block-start: 1px solid var(--line);
  }
  .danger-zone {
    border-color: color-mix(in srgb, var(--danger) 45%, var(--line));
  }
  @media (max-width: 34rem) {
    .totp {
      grid-template-columns: 1fr;
    }
    .qr {
      max-width: 12rem;
    }
  }
</style>
