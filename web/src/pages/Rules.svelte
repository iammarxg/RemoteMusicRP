<!-- SPDX-License-Identifier: AGPL-3.0-only -->
<script>
  import { onMount } from 'svelte';
  import { slide } from 'svelte/transition';
  import { api } from '../lib/api.js';
  import { t } from '../lib/i18n.js';
  import { toast } from '../lib/stores.svelte.js';
  import Toggle from '../components/Toggle.svelte';

  let data = $state(null);
  let artists = $state('');
  let tracks = $state('');
  let r = $state(null);
  let saved = $state('');
  let busy = $state(false);
  let error = $state('');

  const DAYS = ['mon', 'tue', 'wed', 'thu', 'fri', 'sat', 'sun'];
  let zones = [];
  try {
    zones = Intl.supportedValuesOf('timeZone');
  } catch {
    zones = [];
  }

  const lines = (s) =>
    s
      .split('\n')
      .map((x) => x.trim())
      .filter(Boolean);

  function load(d) {
    data = d;
    r = structuredClone(d.rules);
    artists = r.blocked_artists.join('\n');
    tracks = r.blocked_tracks.join('\n');
    saved = snapshot();
  }
  function snapshot() {
    return JSON.stringify({ ...r, blocked_artists: lines(artists), blocked_tracks: lines(tracks) });
  }
  let dirty = $derived(r ? snapshot() !== saved : false);

  onMount(async () => {
    try {
      load(await api.get('/api/rules'));
    } catch (e) {
      error = e.message;
    }
  });

  function addWindow() {
    r.quiet.push({ days: [true, true, true, true, true, true, true], start: '23:00', end: '07:00' });
    r.quiet_enabled = true;
  }

  async function save(e) {
    e.preventDefault();
    busy = true;
    error = '';
    try {
      load(await api.put('/api/rules', { ...r, blocked_artists: lines(artists), blocked_tracks: lines(tracks) }));
      toast(t('common.saved'));
    } catch (err) {
      error = err.message;
    } finally {
      busy = false;
    }
  }
</script>

<div class="stack page">
  <header class="stack head">
    <h1>{t('rules.title')}</h1>
    <p class="muted">{t('rules.intro')}</p>
  </header>

  {#if !r}
    {#if error}<p class="error-text" role="alert">{error}</p>{:else}<div class="skeleton sk"></div>{/if}
  {:else}
    <form class="stack" onsubmit={save}>
      <section class="panel stack">
        <div>
          <h2>{t('rules.blocklist')}</h2>
          <p class="muted small">{t('rules.blocklist_hint')}</p>
        </div>
        <div class="cols">
          <label class="field">
            {t('rules.artists')}
            <span class="hint">{t('rules.artists_hint')}</span>
            <textarea bind:value={artists} rows="6" spellcheck="false" placeholder="Artist name"></textarea>
          </label>
          <label class="field">
            {t('rules.tracks')}
            <span class="hint">{t('rules.tracks_hint')}</span>
            <textarea bind:value={tracks} rows="6" spellcheck="false" placeholder="Song title&#10;Artist - Song title"></textarea>
          </label>
        </div>
      </section>

      <section class="panel stack">
        <div class="spread">
          <div>
            <h2>{t('rules.quiet')}</h2>
            <p class="muted small">{t('rules.quiet_hint')}</p>
          </div>
          {#if data.quiet_now && r.quiet_enabled}<span class="badge">{t('rules.quiet_now')}</span>{/if}
        </div>
        <Toggle bind:checked={r.quiet_enabled} label={t('rules.quiet_enable')} />
        {#each r.quiet as w, i (i)}
          <div class="window" transition:slide={{ duration: 160 }}>
            <div class="days" role="group" aria-label={t('rules.days')}>
              {#each DAYS as d, j (d)}
                <button
                  type="button"
                  class="day"
                  aria-pressed={w.days[j]}
                  onclick={() => (w.days[j] = !w.days[j])}>{t(`rules.day_${d}`)}</button
                >
              {/each}
            </div>
            <div class="times">
              <label class="field">
                {t('rules.from')}
                <input type="time" bind:value={w.start} required />
              </label>
              <label class="field">
                {t('rules.to')}
                <input type="time" bind:value={w.end} required />
              </label>
              <button type="button" class="ghost" onclick={() => r.quiet.splice(i, 1)}>{t('rules.remove')}</button>
            </div>
            {#if w.start === w.end}<p class="muted small">{t('rules.all_day')}</p>
            {:else if w.start > w.end}<p class="muted small">{t('rules.overnight')}</p>{/if}
          </div>
        {/each}
        <div>
          <button type="button" onclick={addWindow} disabled={r.quiet.length >= 14}>{t('rules.add')}</button>
        </div>
        <label class="field">
          {t('rules.zone')}
          <span class="hint">{t('rules.zone_hint', { zone: data.system_zone })}</span>
          <input bind:value={r.time_zone} list="tz-list" maxlength="64" placeholder={data.system_zone} spellcheck="false" />
          <datalist id="tz-list">
            {#each zones as z (z)}<option value={z}></option>{/each}
          </datalist>
        </label>
      </section>

      {#if error}<p class="error-text" role="alert">{error}</p>{/if}
      <div class="row">
        <button class="primary" type="submit" disabled={busy || !dirty}>{t('common.save')}</button>
        {#if dirty}<span class="muted small">{t('rules.unsaved')}</span>{/if}
      </div>
    </form>
  {/if}
</div>

<style>
  .page {
    gap: 1.5rem;
    max-width: 52rem;
  }
  .head {
    gap: 0.4rem;
  }
  .sk {
    block-size: 20rem;
  }
  .cols {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 1rem;
  }
  .window {
    display: grid;
    gap: 0.75rem;
    padding: 0.9rem;
    border: 1px solid var(--line);
    border-radius: var(--radius-m);
  }
  .days {
    display: flex;
    flex-wrap: wrap;
    gap: 0.35rem;
  }
  .day {
    min-inline-size: 3rem;
    padding-inline: 0.6rem;
    color: var(--muted);
  }
  .day[aria-pressed='true'] {
    background: var(--accent);
    border-color: var(--accent);
    color: var(--accent-ink);
  }
  .times {
    display: flex;
    gap: 0.75rem;
    align-items: end;
    flex-wrap: wrap;
  }
  .times .field {
    inline-size: 9rem;
  }
  .badge {
    font-size: var(--step--1);
    font-weight: 600;
    border: 1px solid var(--line-strong);
    border-radius: 999px;
    padding: 0.2rem 0.7rem;
  }
  @media (max-width: 40rem) {
    .cols {
      grid-template-columns: 1fr;
    }
  }
</style>
