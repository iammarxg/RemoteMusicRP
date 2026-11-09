<!-- SPDX-License-Identifier: AGPL-3.0-only -->
<script>
  import { slide } from 'svelte/transition';
  import { api } from '../lib/api.js';
  import { t } from '../lib/i18n.js';
  import { toast } from '../lib/stores.svelte.js';
  import Select from './Select.svelte';

  let { passwordSet = false, onImported = () => {} } = $props();

  // ---- export ----
  let kind = $state('full');
  let pass = $state('');
  let pass2 = $state('');
  let password = $state('');
  let exBusy = $state(false);
  let exError = $state('');

  function download(obj, name) {
    const blob = new Blob([JSON.stringify(obj, null, 2)], { type: 'application/json' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = name;
    document.body.append(a);
    a.click();
    a.remove();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
  }

  async function doExport(ev) {
    ev.preventDefault();
    exError = '';
    if (kind === 'full' && pass !== pass2) {
      exError = t('backup.mismatch');
      return;
    }
    exBusy = true;
    try {
      const file = await api.post('/api/backup/export', {
        kind,
        passphrase: kind === 'full' ? pass : undefined,
        password: kind === 'full' && passwordSet ? password : undefined,
      });
      const day = new Date().toISOString().slice(0, 10);
      download(file, `remotemusicrp-${kind === 'full' ? 'backup' : 'settings'}-${day}.json`);
      pass = pass2 = password = '';
      toast(t('backup.exported'));
    } catch (err) {
      exError = err.message;
    } finally {
      exBusy = false;
    }
  }

  // ---- import ----
  let file = $state(null);
  let fileName = $state('');
  let importPass = $state('');
  let imBusy = $state(false);
  let imError = $state('');

  async function pick(ev) {
    imError = '';
    file = null;
    const f = ev.currentTarget.files?.[0];
    if (!f) return;
    fileName = f.name;
    if (f.size > 300 * 1024) {
      imError = t('backup.too_big');
      return;
    }
    try {
      file = JSON.parse(await f.text());
    } catch {
      imError = t('backup.not_json');
    }
  }

  async function doImport(ev) {
    ev.preventDefault();
    imBusy = true;
    imError = '';
    try {
      const r = await api.post('/api/backup/import', {
        file,
        passphrase: file?.encrypted ? importPass : undefined,
      });
      toast(t('backup.imported', { n: r.secrets }));
      file = null;
      fileName = '';
      importPass = '';
      onImported();
    } catch (err) {
      imError = err.message;
    } finally {
      imBusy = false;
    }
  }
</script>

<section class="panel stack">
  <div>
    <h2>{t('backup.title')}</h2>
    <p class="muted small">{t('backup.intro')}</p>
  </div>

  <form class="stack" onsubmit={doExport}>
    <h3>{t('backup.export')}</h3>
    <div class="field">
      <span>{t('backup.kind')}</span>
      <Select
        bind:value={kind}
        label={t('backup.kind')}
        options={[
          { value: 'full', label: t('backup.kind_full') },
          { value: 'settings', label: t('backup.kind_settings') },
        ]}
      />
      <span class="hint">{t(kind === 'full' ? 'backup.kind_full_hint' : 'backup.kind_settings_hint')}</span>
    </div>
    {#if kind === 'full'}
      <div class="stack" transition:slide={{ duration: 160 }}>
        <div class="two">
          <label class="field">
            {t('backup.passphrase')}
            <input type="password" bind:value={pass} minlength="10" maxlength="1024" autocomplete="new-password" required />
          </label>
          <label class="field">
            {t('backup.passphrase_again')}
            <input type="password" bind:value={pass2} minlength="10" maxlength="1024" autocomplete="new-password" required />
          </label>
        </div>
        <p class="muted small">{t('backup.passphrase_hint')}</p>
        {#if passwordSet}
          <label class="field">
            {t('backup.password')}
            <span class="hint">{t('backup.password_hint')}</span>
            <input type="password" bind:value={password} autocomplete="current-password" required />
          </label>
        {/if}
      </div>
    {/if}
    {#if exError}<p class="error-text" role="alert">{exError}</p>{/if}
    <div><button class="primary" type="submit" disabled={exBusy}>{t('backup.download')}</button></div>
  </form>

  <hr />

  <form class="stack" onsubmit={doImport}>
    <h3>{t('backup.import')}</h3>
    <p class="muted small">{t('backup.import_hint')}</p>
    <label class="file">
      <input type="file" accept=".json,application/json" onchange={pick} />
      <span class="btn">{t('backup.choose')}</span>
      <span class="muted small">{fileName || t('backup.no_file')}</span>
    </label>
    {#if file?.encrypted}
      <label class="field" transition:slide={{ duration: 160 }}>
        {t('backup.passphrase')}
        <input type="password" bind:value={importPass} maxlength="1024" autocomplete="off" required />
      </label>
    {/if}
    {#if imError}<p class="error-text" role="alert">{imError}</p>{/if}
    <div><button type="submit" disabled={imBusy || !file}>{t('backup.restore')}</button></div>
  </form>
</section>

<style>
  h3 {
    font-size: var(--step-0);
    margin: 0;
  }
  hr {
    border: 0;
    border-block-start: 1px solid var(--line);
    margin: 0.25rem 0;
    inline-size: 100%;
  }
  .two {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 0.75rem;
  }
  div.field {
    display: grid;
    gap: 0.4rem;
    font-weight: 550;
    font-size: var(--step--1);
  }
  .hint {
    font-weight: 400;
    color: var(--muted);
  }
  .file {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    flex-wrap: wrap;
    cursor: pointer;
  }
  .file input {
    position: absolute;
    inline-size: 1px;
    block-size: 1px;
    opacity: 0;
  }
  .file:focus-within .btn {
    outline: 2px solid var(--focus);
    outline-offset: 2px;
  }
  @media (max-width: 40rem) {
    .two {
      grid-template-columns: 1fr;
    }
  }
</style>
