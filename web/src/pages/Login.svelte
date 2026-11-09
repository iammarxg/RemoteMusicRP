<!-- SPDX-License-Identifier: AGPL-3.0-only -->
<script>
  import { slide } from 'svelte/transition';
  import { api } from '../lib/api.js';
  import { t } from '../lib/i18n.js';
  import Logo from '../components/Logo.svelte';

  let { onDone } = $props();
  let password = $state('');
  let code = $state('');
  let remember = $state(false);
  let needCode = $state(false);
  let busy = $state(false);
  let error = $state('');

  async function submit(e) {
    e.preventDefault();
    busy = true;
    error = '';
    try {
      await api.post('/api/auth/login', { password, code: needCode ? code : null, remember });
      password = '';
      code = '';
      await onDone();
    } catch (err) {
      if (err.data?.need_code) {
        needCode = true;
      } else {
        error = err.message;
      }
    } finally {
      busy = false;
    }
  }
</script>

<main class="center">
  <form class="panel stack box" onsubmit={submit}>
    <Logo />
    <h1>{t('login.title')}</h1>
    <label class="field">
      {t('login.password')}
      <!-- svelte-ignore a11y_autofocus -->
      <input type="password" bind:value={password} autocomplete="current-password" required autofocus
        aria-invalid={!!error} />
    </label>
    {#if needCode}
      <label class="field" transition:slide={{ duration: 160 }}>
        {t('login.code')}
        <span class="hint">{t('login.code_hint')}</span>
        <!-- svelte-ignore a11y_autofocus -->
        <input bind:value={code} autocomplete="one-time-code" inputmode="text" required autofocus class="mono" />
      </label>
    {/if}
    <label class="check small">
      <input type="checkbox" bind:checked={remember} />
      {t('login.remember')}
    </label>
    {#if error}<p class="error-text" role="alert">{error}</p>{/if}
    <div><button class="primary" disabled={busy || !password}>{t('login.submit')}</button></div>
    <p class="muted small">{t('login.forgot')}</p>
  </form>
</main>

<style>
  .center {
    min-height: 100dvh;
    display: grid;
    place-items: center;
    padding: 1.25rem;
  }
  .box {
    max-width: 26rem;
    width: 100%;
  }
  .check {
    display: flex;
    gap: 0.5rem;
    align-items: center;
  }
</style>
