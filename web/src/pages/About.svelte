<!-- SPDX-License-Identifier: AGPL-3.0-only -->
<script>
  import { onMount } from 'svelte';
  import { api } from '../lib/api.js';
  import { t } from '../lib/i18n.js';
  import Logo from '../components/Logo.svelte';

  let a = $state(null);
  onMount(async () => {
    a = await api.get('/api/about').catch(() => null);
  });
</script>

<div class="stack page">
  <h1>{t('about.title')}</h1>
  <section class="panel stack">
    <Logo />
    <p>{t('app.tagline')}</p>
    {#if a}
      <p class="muted tabular">{t('about.version', { v: a.version })}</p>
      <p>{t('about.license')}</p>
      <p><a href={a.repository} target="_blank" rel="noopener noreferrer">{t('about.source')}</a></p>
    {/if}
  </section>
  <section class="panel stack">
    <h2>{t('about.privacy')}</h2>
    <p class="muted">{t('about.privacy_body')}</p>
  </section>
  {#if a}
    <section class="panel stack">
      <h2>{t('about.credits')}</h2>
      <ul class="credits">
        {#each a.attribution as c (c.name)}
          <li><a href={c.url} target="_blank" rel="noopener noreferrer">{c.name}</a> <span class="muted">{c.text}</span></li>
        {/each}
      </ul>
    </section>
  {/if}
</div>

<style>
  .page {
    gap: 1.5rem;
    max-width: 46rem;
  }
  .credits {
    margin: 0;
    padding-inline-start: 1.1rem;
    display: grid;
    gap: 0.4rem;
  }
</style>
