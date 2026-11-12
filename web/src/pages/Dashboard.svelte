<!-- SPDX-License-Identifier: AGPL-3.0-only -->
<script>
  import { onDestroy, onMount } from 'svelte';
  import { fade } from 'svelte/transition';
  import { api } from '../lib/api.js';
  import { t, clock } from '../lib/i18n.js';
  import { now, toast, go } from '../lib/stores.svelte.js';
  import PresencePreview from '../components/PresencePreview.svelte';
  import MemberPreview from '../components/MemberPreview.svelte';

  let { boot } = $props();
  let data = $state(null);
  let error = $state('');
  let conn = $state(null);
  let timer;

  async function poll() {
    if (document.hidden) return;
    try {
      data = await api.get('/api/status');
      error = '';
    } catch (e) {
      error = e.message;
    }
  }

  onMount(async () => {
    poll();
    conn = await api.get('/api/connections').catch(() => null);
    timer = setInterval(poll, 2000);
    document.addEventListener('visibilitychange', poll);
  });
  onDestroy(() => {
    clearInterval(timer);
    document.removeEventListener('visibilitychange', poll);
  });

  let svc = $derived(data?.service);
  // Discord is only contacted once the service is actually running.
  let live = $derived(svc && ['running', 'paused', 'error'].includes(svc.phase));
  let np = $derived(svc?.now_playing);
  let elapsed = $derived(np ? now.unix - np.started_unix : 0);
  let total = $derived(np?.duration_ms ? np.duration_ms / 1000 : null);
  let pct = $derived(total ? Math.min(100, (elapsed / total) * 100) : null);

  // A cover the browser couldn't load: show the blank cover with a note, and
  // tell the server once so it's in the Activity log.
  let brokenArt = $state(null);
  function artFailed(url) {
    if (brokenArt === url) return;
    brokenArt = url;
    api.post('/api/artwork/display-failed').catch(() => {});
  }
  let artOk = $derived(np?.art_url && brokenArt !== np.art_url);

  function uptime(s) {
    const h = Math.floor(s / 3600);
    const m = Math.floor((s % 3600) / 60);
    return h ? `${h} h ${m} min` : `${m} min`;
  }

  async function setPaused(paused) {
    try {
      await api.post('/api/presence/pause', { paused });
      await poll();
    } catch (e) {
      toast(t('common.error_generic', { message: e.message }), 'error');
    }
  }
</script>

<div class="stack page">
  <div class="spread">
    <h1>{t('dashboard.title')}</h1>
    {#if data}
      {#if data.paused}
        <button class="primary" onclick={() => setPaused(false)}>{t('dashboard.resume')}</button>
      {:else}
        <button onclick={() => setPaused(true)}>{t('dashboard.pause')}</button>
      {/if}
    {/if}
  </div>

  {#if error}<p class="error-text" role="alert">{error}</p>{/if}

  <section class="stage" aria-live="polite" aria-busy={!data}>
    {#if artOk && !data?.paused}
      {#key np.art_url}
        <img class="backdrop" src={np.art_url} alt="" aria-hidden="true" referrerpolicy="no-referrer"
          in:fade={{ duration: 700 }} />
      {/key}
    {/if}
    {#if !data}
      <div class="art skeleton"></div>
      <div class="meta stack">
        <div class="skeleton sk-title"></div>
        <div class="skeleton sk-line"></div>
      </div>
    {:else if svc?.phase === 'needs_setup'}
      <div class="meta stack">
        <h2>{t('dashboard.needs_setup')}</h2>
        <div><button class="primary" onclick={() => go('connections')}>{t('dashboard.finish_setup')}</button></div>
      </div>
    {:else if data.paused}
      <div class="meta stack">
        <h2>{t('dashboard.paused')}</h2>
        <p class="muted">{t('dashboard.paused_hint')}</p>
      </div>
    {:else if svc?.hidden === 'quiet'}
      <div class="meta stack">
        <h2>{t('dashboard.hidden_quiet')}</h2>
        <p class="muted">{t('dashboard.hidden_quiet_hint')}</p>
      </div>
    {:else if np}
      {#key np.artist + np.title}
        <div class="np" in:fade={{ duration: 400 }}>
          {#if artOk}
            <img
              class="art"
              src={np.art_url}
              alt={np.album ?? ''}
              referrerpolicy="no-referrer"
              onerror={() => artFailed(np.art_url)}
            />
          {:else}
            <div class="art blank" aria-hidden="true">
              <svg viewBox="0 0 64 64"><circle cx="32" cy="32" r="28" fill="none" stroke="currentColor" stroke-width="2" /><circle cx="32" cy="32" r="6" fill="currentColor" /></svg>
            </div>
          {/if}
          <div class="meta">
            <p class="muted small">{t('dashboard.listening')}</p>
            {#if svc.hidden}<p class="hidden-note small">{t(`dashboard.hidden_${svc.hidden}`)}</p>{/if}
            <h2 class="title">{np.title}</h2>
            <p class="artist">{np.artist}</p>
            {#if np.album}<p class="muted">{np.album}</p>{/if}
            {#if pct !== null}
              <div class="progress tabular small">
                <span>{clock(Math.min(elapsed, total))}</span>
                <span class="bar"><span style:inline-size={`${pct}%`}></span></span>
                <span>{clock(total)}</span>
              </div>
            {/if}
            <p class="muted small links">
              {#if np.art_url && !artOk}<span>{t('dashboard.art_not_loaded')} </span>{/if}
              {#if np.art_source}<span
                  >{t('dashboard.art_from', { source: t(`dashboard.source_${np.art_source}`) })}{#if np.art_release}
                    · {t(`dashboard.release_${np.art_release}`)}{/if}</span
                >{/if}
              {#if np.track_url}<a href={np.track_url} target="_blank" rel="noopener noreferrer">{t('dashboard.open_track')}</a>{/if}
            </p>
          </div>
        </div>
      {/key}
    {:else}
      <div class="meta stack">
        <h2>{t('dashboard.nothing')}</h2>
        <p class="muted">{t('dashboard.nothing_hint')}</p>
      </div>
    {/if}
  </section>

  <div class="grid">
    <section class="panel stack">
      <h3>{t('dashboard.on_discord')}</h3>
      {#if svc?.activity}
        <MemberPreview
          activity={svc.activity}
          user={svc.discord_user ? { name: svc.discord_display_name ?? svc.discord_user, avatar: svc.discord_avatar_url, status: svc.discord_status } : null}
        />
      {/if}
      <PresencePreview activity={svc?.activity ?? null} />
    </section>

    <section class="panel stack">
      <div class="status">
        <span class="dot" class:ok={svc?.discord_connected}
          class:bad={live && !svc.discord_connected}></span>
        <div>
          <p class="label">{t('dashboard.status_discord')}</p>
          <p class="muted small">
            {#if svc?.discord_connected}
              {svc.discord_user ? t('dashboard.discord_ok', { user: svc.discord_user }) : t('dashboard.discord_ok_anon')}
            {:else if live}
              {svc?.discord_error ?? t('dashboard.discord_off')}
            {:else}
              {t('dashboard.discord_after_setup')}
            {/if}
          </p>
        </div>
      </div>
      <div class="status">
        <span class="dot" class:ok={svc && !svc.lastfm_error && conn?.lastfm?.connected}
          class:bad={!!svc?.lastfm_error}></span>
        <div>
          <p class="label">{t('dashboard.status_lastfm')}</p>
          <p class="muted small">
            {#if svc?.lastfm_error}{svc.lastfm_error}
            {:else if conn?.lastfm?.connected}{t('dashboard.lastfm_ok', { user: conn.lastfm.username })}
            {:else}{t('common.not_connected')}{/if}
          </p>
        </div>
      </div>
      {#if data}<p class="muted small tabular">{t('dashboard.uptime', { time: uptime(data.uptime_secs) })}</p>{/if}
    </section>
  </div>
</div>

<style>
  .hidden-note {
    font-weight: 600;
    margin-block-end: 0.3rem;
  }
  .page {
    gap: 1.5rem;
  }
  .stage {
    position: relative;
    isolation: isolate;
    overflow: hidden;
    display: grid;
    grid-template-columns: auto 1fr;
    gap: clamp(1.25rem, 3vw, 2.5rem);
    align-items: center;
    min-block-size: 16rem;
    padding: clamp(1.25rem, 3.5vw, 2.25rem);
    border-radius: var(--radius-l);
    background: var(--surface);
    border: 1px solid var(--line);
  }
  /* The album art, blurred and darkened, is the only colour in the UI. */
  .backdrop {
    position: absolute;
    inset: -20%;
    inline-size: 140%;
    block-size: 140%;
    object-fit: cover;
    filter: blur(60px) saturate(1.4);
    opacity: 0.42;
    z-index: -1;
    pointer-events: none;
  }
  .stage::after {
    content: '';
    position: absolute;
    inset: 0;
    z-index: -1;
    background: linear-gradient(90deg, transparent 0%, color-mix(in srgb, var(--surface) 70%, transparent) 70%);
    pointer-events: none;
  }
  .np {
    display: contents;
  }
  .art {
    inline-size: clamp(8rem, 22vw, 13rem);
    aspect-ratio: 1;
    border-radius: var(--radius-m);
    object-fit: cover;
    box-shadow: var(--shadow);
  }
  .art.blank {
    display: grid;
    place-items: center;
    background: var(--surface-2);
    color: var(--line);
  }
  .art.blank svg {
    inline-size: 50%;
  }
  .meta {
    min-width: 0;
    display: grid;
    gap: 0.3rem;
    grid-column: span 1;
  }
  .stage > .meta:only-child {
    grid-column: 1 / -1;
  }
  .title {
    font-size: var(--step-4);
    font-weight: 700;
    letter-spacing: -0.045em;
    line-height: 1.02;
    overflow-wrap: anywhere;
  }
  .artist {
    font-size: var(--step-2);
    font-weight: 500;
    letter-spacing: -0.02em;
  }
  .progress {
    display: grid;
    grid-template-columns: auto 1fr auto;
    gap: 0.6rem;
    align-items: center;
    max-width: 30rem;
    margin-block-start: 0.6rem;
  }
  .bar {
    block-size: 5px;
    border-radius: 3px;
    background: var(--line);
    overflow: hidden;
  }
  .bar span {
    display: block;
    block-size: 100%;
    background: var(--text);
    transition: inline-size 1s linear;
  }
  .sk-title {
    block-size: 2.5rem;
    inline-size: 70%;
  }
  .sk-line {
    block-size: 1.2rem;
    inline-size: 40%;
  }
  .links {
    display: flex;
    gap: 1rem;
    flex-wrap: wrap;
    margin-block-start: 0.4rem;
  }
  .grid {
    display: grid;
    grid-template-columns: minmax(0, 1.2fr) minmax(0, 1fr);
    gap: 1.25rem;
    align-items: start;
  }
  .status {
    display: flex;
    gap: 0.8rem;
    align-items: baseline;
  }
  .label {
    font-weight: 650;
  }
  @media (max-width: 44rem) {
    .stage {
      grid-template-columns: 1fr;
    }
    .grid {
      grid-template-columns: 1fr;
    }
  }
</style>
