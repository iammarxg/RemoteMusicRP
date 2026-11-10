<!-- SPDX-License-Identifier: AGPL-3.0-only -->
<!-- Shown on another device while the instance has no password yet: the
     first password is set on the computer running RemoteMusicRP (no code
     needed there). Checks every few seconds and moves on to the login page
     once a password exists. Headless servers can use the setup code. -->
<script>
  import { onMount, onDestroy } from 'svelte';
  import { t } from '../lib/i18n.js';
  import { toast } from '../lib/stores.svelte.js';
  import Logo from '../components/Logo.svelte';

  let { boot, onRefresh, onUseCode } = $props();
  let timer;

  onMount(() => {
    timer = setInterval(onRefresh, 3000);
  });
  onDestroy(() => clearInterval(timer));

  async function copy(text) {
    try {
      await navigator.clipboard.writeText(text);
      toast(t('common.copied'));
    } catch {
      /* clipboard blocked: the text is selectable */
    }
  }
</script>

<main class="center">
  <section class="panel stack box">
    <Logo />
    <h1>{t('elsewhere.title')}</h1>
    <p>{t('elsewhere.body')}</p>
    <ol class="steps">
      <li>
        {t('elsewhere.step_open')}
        <span class="addr">
          <code class="mono">{boot.local_url}</code>
          <button type="button" class="ghost" onclick={() => copy(boot.local_url)}>{t('common.copy')}</button>
        </span>
        <span class="muted small">{t('elsewhere.step_open_hint')}</span>
      </li>
      <li>{t('elsewhere.step_password')}</li>
    </ol>
    <p class="muted small waiting" aria-live="polite">
      <span class="dot"></span>{t('elsewhere.waiting')}
    </p>
    <hr />
    <div class="stack tight">
      <p class="small"><strong>{t('elsewhere.headless_title')}</strong></p>
      <p class="muted small">{t('elsewhere.headless_body')}</p>
      <div><button type="button" onclick={onUseCode}>{t('elsewhere.use_code')}</button></div>
    </div>
  </section>
</main>

<style>
  .center {
    min-height: 100dvh;
    display: grid;
    place-items: center;
    padding: 1.25rem;
  }
  .box {
    max-width: 30rem;
    width: 100%;
  }
  .steps {
    margin: 0;
    padding-inline-start: 1.2rem;
    display: grid;
    gap: 0.6rem;
  }
  .steps li > span {
    display: block;
  }
  .addr {
    display: flex !important;
    align-items: center;
    gap: 0.5rem;
    flex-wrap: wrap;
    margin-block: 0.25rem;
  }
  .addr code {
    background: var(--surface-2);
    border-radius: var(--radius-s);
    padding: 0.25rem 0.5rem;
  }
  .waiting {
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }
  .dot {
    inline-size: 0.5rem;
    block-size: 0.5rem;
    border-radius: 50%;
    background: var(--muted);
    animation: pulse 1.6s ease-in-out infinite;
  }
  @keyframes pulse {
    50% {
      opacity: 0.25;
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .dot {
      animation: none;
    }
  }
  hr {
    border: none;
    border-block-start: 1px solid var(--line);
    margin: 0.25rem 0;
  }
  .tight {
    gap: 0.4rem;
  }
  .tight p {
    margin: 0;
  }
</style>
