<!-- SPDX-License-Identifier: AGPL-3.0-only -->
<script>
  import { fly } from 'svelte/transition';
  import { api } from '../lib/api.js';
  import { t, tl } from '../lib/i18n.js';
  import { go } from '../lib/stores.svelte.js';
  import Logo from '../components/Logo.svelte';

  let { boot, onChange } = $props();

  const steps = ['password', 'discord', 'lastfm', 'done'];

  // Start at the first unfinished step.
  function firstStep(b) {
    if (!b.setup.password_set && !b.setup.password_skipped) return 0;
    if (!b.setup.discord_configured) return 1;
    if (!b.setup.lastfm_connected) return 2;
    return 3;
  }
  // svelte-ignore state_referenced_locally
  let step = $state(firstStep(boot));
  let busy = $state(false);
  let error = $state('');

  let password = $state('');
  let confirm = $state('');
  let setupCode = $state('');
  let clientId = $state('');
  let lfUser = $state('');
  let lfKey = $state('');

  async function run(fn) {
    busy = true;
    error = '';
    try {
      await fn();
    } catch (err) {
      error = err.message;
    } finally {
      busy = false;
    }
  }

  const setPassword = (e) => {
    e.preventDefault();
    if (password !== confirm) {
      error = t('wizard.password_mismatch');
      return;
    }
    run(async () => {
      await api.post('/api/setup/password', { password, setup_code: setupCode || null });
      password = confirm = '';
      await onChange();
      step = 1;
    });
  };
  const skipPassword = () =>
    run(async () => {
      await api.post('/api/setup/skip-password');
      await onChange();
      step = 1;
    });
  const saveDiscord = (e) => {
    e.preventDefault();
    run(async () => {
      await api.post('/api/connections/discord', { client_id: clientId.trim() });
      await onChange();
      step = 2;
    });
  };
  const saveLastfm = (e) => {
    e.preventDefault();
    run(async () => {
      await api.post('/api/connections/lastfm', { username: lfUser.trim(), api_key: lfKey.trim() });
      lfKey = '';
      await onChange();
      step = 3;
    });
  };
  const finish = () =>
    run(async () => {
      await api.post('/api/setup/complete');
      go('dashboard');
      await onChange();
    });
</script>

<main class="wizard">
  <header class="stack">
    <Logo />
    <h1>{t('wizard.title')}</h1>
    <p class="muted">{t('wizard.intro')}</p>
  </header>

  <ol class="steps" aria-label={t('wizard.title')}>
    {#each steps as s, i (s)}
      <li class:done={i < step} class:current={i === step} aria-current={i === step ? 'step' : undefined}>
        <span class="num tabular">{i + 1}</span>
        <span class="label">{t(`wizard.step_${s}`)}</span>
      </li>
    {/each}
  </ol>

  {#key step}
    <section class="panel stack" in:fly={{ y: 10, duration: 200 }}>
      {#if step === 0}
        <h2>{t('wizard.step_password')}</h2>
        <p>{boot.exposed ? t('wizard.password_required') : t('wizard.password_body')}</p>
        <form class="stack" onsubmit={setPassword}>
          {#if boot.needs_setup_code}
            <label class="field">
              {t('wizard.setup_code')}
              <span class="hint">{t('wizard.setup_code_hint')}</span>
              <input bind:value={setupCode} class="mono" autocomplete="off" required />
            </label>
          {/if}
          <label class="field">
            {t('wizard.password')} <span class="hint">{t('wizard.password_hint')}</span>
            <input type="password" bind:value={password} autocomplete="new-password" minlength="12" required />
          </label>
          <label class="field">
            {t('wizard.password_confirm')}
            <input type="password" bind:value={confirm} autocomplete="new-password" minlength="12" required />
          </label>
          {#if error}<p class="error-text" role="alert">{error}</p>{/if}
          <div class="row">
            <button class="primary" disabled={busy || password.length < 12}>{t('wizard.set_password')}</button>
            {#if !boot.exposed}
              <button type="button" class="ghost" disabled={busy} onclick={skipPassword}>{t('wizard.skip')}</button>
            {/if}
          </div>
        </form>
      {:else if step === 1}
        <h2>{t('wizard.step_discord')}</h2>
        <p>{t('wizard.discord_body')}</p>
        <ol class="guide">
          {#each tl(boot.discord_mode === 'social_sdk' ? 'wizard.discord_steps_sdk' : 'wizard.discord_steps') as line, i (i)}<li>{line}</li>{/each}
        </ol>
        <p>
          <a href="https://discord.com/developers/applications" target="_blank" rel="noopener noreferrer"
            >{t('wizard.discord_portal')}</a
          >
        </p>
        <form class="stack" onsubmit={saveDiscord}>
          <label class="field">
            {t('wizard.discord_id')} <span class="hint">{t('wizard.discord_id_hint')}</span>
            <input bind:value={clientId} inputmode="numeric" autocomplete="off" class="mono" required
              pattern={'[0-9]{17,20}'} />
          </label>
          {#if error}<p class="error-text" role="alert">{error}</p>{/if}
          <div class="row">
            <button class="primary" disabled={busy || !/^\d{17,20}$/.test(clientId.trim())}>{t('common.continue')}</button>
          </div>
        </form>
      {:else if step === 2}
        <h2>{t('wizard.step_lastfm')}</h2>
        <p>{t('wizard.lastfm_body')}</p>
        <ol class="guide">
          {#each tl('wizard.lastfm_steps') as line, i (i)}<li>{line}</li>{/each}
        </ol>
        <p>
          <a href="https://www.last.fm/api/account/create" target="_blank" rel="noopener noreferrer"
            >{t('wizard.lastfm_portal')}</a
          >
        </p>
        <form class="stack" onsubmit={saveLastfm}>
          <label class="field">
            {t('wizard.lastfm_user')}
            <input bind:value={lfUser} autocomplete="username" required />
          </label>
          <label class="field">
            {t('wizard.lastfm_key')} <span class="hint">{t('wizard.lastfm_key_hint')}</span>
            <input type="password" bind:value={lfKey} autocomplete="off" class="mono" required />
          </label>
          {#if error}<p class="error-text" role="alert">{error}</p>{/if}
          <div class="row">
            <button type="button" class="ghost" onclick={() => (step = 1)}>{t('common.back')}</button>
            <button class="primary" disabled={busy || !lfUser || lfKey.trim().length !== 32}>
              {busy ? t('wizard.checking') : t('common.continue')}
            </button>
          </div>
        </form>
      {:else}
        <h2>{t('wizard.done_title')}</h2>
        <p>{t('wizard.done_body')}</p>
        {#if error}<p class="error-text" role="alert">{error}</p>{/if}
        <div><button class="primary" disabled={busy} onclick={finish}>{t('wizard.finish')}</button></div>
      {/if}
    </section>
  {/key}
</main>

<style>
  .wizard {
    max-width: 40rem;
    margin-inline: auto;
    padding: clamp(1.25rem, 5vw, 3.5rem) 1.25rem;
    display: grid;
    gap: 1.75rem;
  }
  .steps {
    list-style: none;
    padding: 0;
    margin: 0;
    display: grid;
    grid-template-columns: repeat(4, 1fr);
    gap: 0.5rem;
  }
  .steps li {
    display: grid;
    gap: 0.4rem;
    color: var(--muted);
    font-size: var(--step--1);
    font-weight: 600;
    border-block-start: 3px solid var(--line);
    padding-block-start: 0.6rem;
    transition: border-color 0.25s, color 0.25s;
  }
  .steps li.done {
    border-color: color-mix(in srgb, var(--accent) 55%, var(--line));
  }
  .steps li.current {
    border-color: var(--accent);
    color: var(--text);
  }
  .num {
    font-size: var(--step-1);
    font-weight: 750;
  }
  .guide {
    margin: 0;
    padding-inline-start: 1.3rem;
    display: grid;
    gap: 0.35rem;
    color: var(--muted);
  }
  .guide li::marker {
    color: var(--accent);
    font-weight: 700;
  }
  @media (max-width: 30rem) {
    .label {
      display: none;
    }
  }
</style>
