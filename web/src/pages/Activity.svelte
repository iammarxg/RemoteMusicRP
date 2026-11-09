<!-- SPDX-License-Identifier: AGPL-3.0-only -->
<script>
  import { onMount } from 'svelte';
  import { api } from '../lib/api.js';
  import { t, dateTime } from '../lib/i18n.js';
  import Select from '../components/Select.svelte';

  let entries = $state(null);
  let stats = $state(null);
  let error = $state('');
  let filter = $state('all');
  const LEVELS = { all: ['info', 'warn', 'error'], warn: ['warn', 'error'], error: ['error'] };
  let shown = $derived(entries ? entries.filter((e) => LEVELS[filter].includes(e.level)) : null);

  function duration(s) {
    const d = Math.floor(s / 86400);
    const h = Math.floor((s % 86400) / 3600);
    const m = Math.floor((s % 3600) / 60);
    return d ? `${d} d ${h} h` : h ? `${h} h ${m} min` : `${m} min`;
  }

  async function load() {
    try {
      const r = await api.get('/api/activity?limit=300');
      entries = r.entries;
      stats = r.stats;
      error = '';
    } catch (e) {
      error = e.message;
    }
  }
  onMount(load);
</script>

<div class="stack page">
  <div class="spread">
    <h1>{t('activity.title')}</h1>
    <button onclick={load}>{t('activity.refresh')}</button>
  </div>
  {#if error}<p class="error-text" role="alert">{error}</p>{/if}
  {#if stats}
    <dl class="stats">
      <div class="panel"><dt>{t('activity.uptime')}</dt><dd class="tabular">{duration(stats.uptime_secs)}</dd></div>
      <div class="panel"><dt>{t('activity.tracks')}</dt><dd class="tabular">{stats.tracks_seen}</dd></div>
      <div class="panel"><dt>{t('activity.warnings')}</dt><dd class="tabular">{stats.last_day.warn}</dd></div>
      <div class="panel"><dt>{t('activity.errors')}</dt><dd class="tabular" class:bad={stats.last_day.error > 0}>{stats.last_day.error}</dd></div>
    </dl>
  {/if}
  {#if entries?.length}
    <div class="row filter">
      <span class="muted small">{t('activity.filter')}</span>
      <div class="sel">
        <Select
          bind:value={filter}
          label={t('activity.filter')}
          options={[
            { value: 'all', label: t('activity.filter_all') },
            { value: 'warn', label: t('activity.filter_warn') },
            { value: 'error', label: t('activity.filter_error') },
          ]}
        />
      </div>
    </div>
  {/if}
  {#if !entries}
    <div class="skeleton sk"></div>
  {:else if !entries.length}
    <p class="muted">{t('activity.empty')}</p>
  {:else if !shown.length}
    <p class="muted">{t('activity.none_match')}</p>
  {:else}
    <ol class="log">
      {#each shown as e, i (i)}
        <li class={e.level}>
          <time class="muted small tabular" datetime={new Date(e.at * 1000).toISOString()}>{dateTime(e.at)}</time>
          <span class="kind small">{e.kind}</span>
          <span class="msg">{e.message}</span>
        </li>
      {/each}
    </ol>
  {/if}
</div>

<style>
  .page {
    gap: 1.5rem;
  }
  .sk {
    block-size: 20rem;
  }
  .stats {
    margin: 0;
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(9.5rem, 1fr));
    gap: 0.75rem;
  }
  .stats .panel {
    padding: 0.9rem 1.1rem;
  }
  .stats dt {
    color: var(--muted);
    font-size: var(--step--1);
    font-weight: 600;
  }
  .stats dd {
    margin: 0.2rem 0 0;
    font-size: var(--step-1);
    font-weight: 650;
  }
  .stats dd.bad {
    color: var(--danger);
  }
  .filter .sel {
    min-inline-size: 13rem;
  }
  .log {
    list-style: none;
    margin: 0;
    padding: 0;
    background: var(--surface);
    border: 1px solid var(--line);
    border-radius: var(--radius-l);
    overflow: hidden;
  }
  .log li {
    display: grid;
    grid-template-columns: 11rem 6.5rem 1fr;
    gap: 1rem;
    padding: 0.7rem 1.1rem;
    border-inline-start: 3px solid transparent;
    align-items: baseline;
  }
  .log li + li {
    border-block-start: 1px solid var(--line);
  }
  .log li.warn {
    border-inline-start-color: var(--accent);
  }
  .log li.error {
    border-inline-start-color: var(--danger);
  }
  .kind {
    color: var(--muted);
    font-weight: 650;
  }
  .msg {
    overflow-wrap: anywhere;
  }
  @media (max-width: 40rem) {
    .log li {
      grid-template-columns: 1fr;
      gap: 0.1rem;
    }
  }
</style>
