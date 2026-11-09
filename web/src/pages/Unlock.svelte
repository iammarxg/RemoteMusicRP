<!-- SPDX-License-Identifier: AGPL-3.0-only -->
<script>
  import { api } from '../lib/api.js';
  import { t } from '../lib/i18n.js';
  import Logo from '../components/Logo.svelte';

  let { onDone } = $props();
  let passphrase = $state('');
  let busy = $state(false);
  let error = $state('');

  async function submit(e) {
    e.preventDefault();
    busy = true;
    error = '';
    try {
      await api.post('/api/unlock', { passphrase });
      passphrase = '';
      await onDone();
    } catch (err) {
      error = err.message;
    } finally {
      busy = false;
    }
  }
</script>

<main class="center">
  <form class="panel stack box" onsubmit={submit}>
    <Logo />
    <h1>{t('unlock.title')}</h1>
    <p class="muted">{t('unlock.body')}</p>
    <p class="muted small">{t('unlock.first')}</p>
    <label class="field">
      {t('unlock.label')}
      <!-- svelte-ignore a11y_autofocus -->
      <input type="password" bind:value={passphrase} autocomplete="current-password" required minlength="12" autofocus
        aria-invalid={!!error} />
    </label>
    {#if error}<p class="error-text" role="alert">{error}</p>{/if}
    <div><button class="primary" disabled={busy || passphrase.length < 12}>{t('unlock.submit')}</button></div>
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
    max-width: 28rem;
    width: 100%;
  }
</style>
