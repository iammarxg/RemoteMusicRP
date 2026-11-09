<!-- SPDX-License-Identifier: AGPL-3.0-only -->
<script>
  import { onMount } from 'svelte';
  import { api } from '../lib/api.js';
  import { t } from '../lib/i18n.js';
  import { toast, now } from '../lib/stores.svelte.js';
  import Toggle from '../components/Toggle.svelte';
  import PresencePreview from '../components/PresencePreview.svelte';
  import MemberPreview from '../components/MemberPreview.svelte';
  import Select from '../components/Select.svelte';

  const opts = (keys, prefix) => keys.map(([v, k]) => ({ value: v, label: t(`${prefix}${k}`) }));

  let s = $state(null);
  let saved = $state('');
  let sample = $state(null);
  let me = $state(null);
  let busy = $state(false);
  let error = $state('');

  onMount(async () => {
    try {
      s = await api.get('/api/presence');
      saved = JSON.stringify(s);
      const st = await api.get('/api/status');
      sample = st.service?.now_playing ?? null;
      const sv = st.service;
      if (sv?.discord_user) me = { name: sv.discord_display_name ?? sv.discord_user, avatar: sv.discord_avatar_url, status: sv.discord_status };
    } catch (e) {
      error = e.message;
    }
  });

  let dirty = $derived(s && JSON.stringify(s) !== saved);

  // ---- mirror of crates/providers/src/cleanup.rs (keyword hiding) ----
  const SUGGESTED = 'live, remastered, remaster, deluxe, anniversary, expanded, bonus track, demo';
  function parseKw(list) {
    const out = [];
    for (let k of (list ?? '').split(',')) {
      k = k.trim().toLowerCase();
      if (k && k.length <= 40 && !out.includes(k)) out.push(k);
      if (out.length === 32) break;
    }
    return out;
  }
  const isAlnum = (c) => c !== undefined && /[\p{L}\p{N}]/u.test(c);
  function hasWord(text, kws) {
    const h = text.toLowerCase();
    return kws.some((k) => {
      for (let i = h.indexOf(k); i !== -1; i = h.indexOf(k, i + 1)) {
        if (!isAlnum(h[i - 1]) && !isAlnum(h[i + k.length])) return true;
      }
      return false;
    });
  }
  function stripKw(str, kws) {
    if (!kws.length || !str) return str;
    const x = str.replace(/\(([^)]*)\)|\[([^\]]*)\]/g, (m, a, b) => (hasWord(a ?? b, kws) ? '' : m));
    const sep = [' - ', ' – ', ' — '].find((t) => x.includes(t)) ?? ' - ';
    const kept = x
      .split(/ [-–—] /)
      .filter((part, i) => i === 0 || !hasWord(part, kws))
      .map((part) => part.trim())
      .filter(Boolean);
    const out = kept
      .join(sep)
      .replace(/\(\)|\[\]/g, '')
      .replace(/\s+/g, ' ')
      .replace(/^[\s\-–—·|,:]+|[\s\-–—·|,:]+$/g, '');
    return out || str;
  }

  // ---- mirror of crates/core/src/presence.rs, for the live preview ----
  function tidy(out) {
    const x = out.replace(/\s+/g, ' ').replace(/^[\s\-–—·|]+|[\s\-–—·|]+$/g, '');
    return ['by', 'on', 'from'].includes(x) ? '' : x;
  }
  function render(tpl, track, artist, album) {
    return tidy(tpl.replace(/\{(track|artist|album)\}/g, (_, k) => ({ track, artist, album })[k] ?? ''));
  }
  function templates(c) {
    if (c.custom_templates) return [c.details_template, c.state_template, c.album_template];
    const artist = c.prefix_by ? 'by {artist}' : '{artist}';
    const album = c.prefix_on ? 'on {album}' : '{album}';
    if (c.one_line) return [c.swap_title_artist ? '{artist} – {track}' : '{track} – {artist}', album, ''];
    if (c.swap_title_artist) return ['{artist}', '{track}', album];
    return ['{track}', artist, album];
  }

  function unprefixed(tpl) {
    const m = /^\s*(?:by|on) \s*(.+)$/.exec(tpl ?? '');
    return m ? m[1] : null;
  }
  // The unprefixed template the member list shows instead, if any.
  function memberBare(c, d, st) {
    if (c.status_line === 'first_line') return unprefixed(d);
    if (c.status_line === 'second_line') return unprefixed(st);
    return null;
  }
  let nameOverridden = $derived(s ? !!memberBare(s, ...templates(s)) : false);

  let ex = $derived(
    sample ?? {
      title: t('preview.sample_title'),
      artist: t('preview.sample_artist'),
      album: t('preview.sample_album'),
      art_url: null,
      track_url: 'https://www.last.fm/',
      started_unix: Math.floor(now.unix) - 42,
      duration_ms: 214000,
    },
  );

  let preview = $derived.by(() => {
    if (!s) return null;
    const kws = parseKw(s.strip_keywords);
    const cleanTitle = stripKw(ex.title, kws);
    const cleanAlbum = ex.album ? stripKw(ex.album, kws) : ex.album;
    const track = s.show_title ? cleanTitle : null;
    const artist = s.show_artist ? ex.artist : null;
    const album = s.show_album ? cleanAlbum : null;
    const [d, st, al] = templates(s);
    const detailsLine = render(d, track, artist, album);
    const stateLine = render(st, track, artist, album);
    const rawAlbum = render(al, track, artist, album);
    // Mirror of the server: never repeat a line Discord already shows.
    const albumLine = rawAlbum === detailsLine || rawAlbum === stateLine ? '' : rawAlbum;
    const image = s.show_art ? (ex.art_url ?? (s.missing_art === 'custom' ? s.missing_art_url : null)) : null;
    const buttons = [];
    if (s.link_button === 'last_fm') buttons.push({ label: 'View on Last.fm', url: ex.track_url ?? 'https://www.last.fm/' });
    if (s.link_button === 'apple_music') buttons.push({ label: 'Listen on Apple Music', url: 'https://music.apple.com/' });
    if (s.custom_button && s.custom_button_label.trim()) buttons.push({ label: s.custom_button_label.trim(), url: s.custom_button_url || '#' });
    if (s.get_status_button) buttons.push({ label: 'Get this status', url: 'https://github.com/iammarxg/RemoteMusicRP' });
    const name = {
      app: null,
      artist: ex.artist,
      title: cleanTitle,
      album: cleanAlbum,
      custom: render(s.custom_name, cleanTitle, ex.artist, cleanAlbum),
    }[s.name_source];
    // Mirror of the server: a "by …"/"on …" member-list line is shown as
    // "Listening to <artist>" (unprefixed text in the name) instead.
    let shownName = name;
    let display = { name: 0, first_line: 2, second_line: 1 }[s.status_line];
    const bareTpl = memberBare(s, d, st);
    if (bareTpl) {
      const bare = render(bareTpl, track, artist, album);
      if (bare) {
        shownName = bare;
        display = 0;
      }
    }
    return {
      type: { listening: 2, playing: 0, watching: 3, competing: 5 }[s.activity_type],
      name: shownName || undefined,
      details: detailsLine || undefined,
      state: stateLine || undefined,
      status_display_type: display,
      assets: image ? { large_image: image, large_text: albumLine || undefined } : undefined,
      timestamps: s.show_progress
        ? { start: ex.started_unix, end: ex.duration_ms ? ex.started_unix + Math.ceil(ex.duration_ms / 1000) : undefined }
        : undefined,
      buttons: buttons.slice(0, 2),
    };
  });
  let tooManyButtons = $derived(
    s && [s.link_button !== 'none', s.custom_button, s.get_status_button].filter(Boolean).length > 2,
  );

  async function save(e) {
    e.preventDefault();
    busy = true;
    error = '';
    try {
      s = await api.put('/api/presence', s);
      saved = JSON.stringify(s);
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
    <h1>{t('presence.title')}</h1>
    <p class="muted">{t('presence.intro')}</p>
  </header>

  {#if !s}
    {#if error}<p class="error-text" role="alert">{error}</p>{:else}<div class="skeleton sk"></div>{/if}
  {:else}
    <div class="layout">
      <form class="stack" onsubmit={save}>
        <section class="panel stack">
          <h2>{t('presence.general')}</h2>
          <div class="field">
            <span>{t('presence.activity_type')}</span>
            <Select
              bind:value={s.activity_type}
              label={t('presence.activity_type')}
              options={opts(['listening', 'playing', 'watching', 'competing'].map((v) => [v, v]), 'presence.type_')}
            />
          </div>
          <div class="field">
            <span>{t('presence.name_source')}</span>
            <span class="hint">{nameOverridden ? t('presence.name_source_overridden') : t('presence.name_source_hint')}</span>
            <Select
              bind:value={s.name_source}
              label={t('presence.name_source')}
              options={opts(['app', 'artist', 'title', 'album', 'custom'].map((v) => [v, v]), 'presence.name_')}
            />
          </div>
          {#if s.name_source === 'custom'}
            <label class="field">
              {t('presence.custom_name')}
              <span class="hint">{t('presence.templates_hint')}</span>
              <input bind:value={s.custom_name} maxlength="128" placeholder="music" />
            </label>
          {/if}
          <div class="field">
            <span>{t('presence.status_line')}</span>
            <span class="hint">{t('presence.status_line_hint')}</span>
            <Select
              bind:value={s.status_line}
              label={t('presence.status_line')}
              options={opts([['name', 'name'], ['first_line', 'first'], ['second_line', 'second']], 'presence.status_')}
            />
            {#if nameOverridden}
              <p class="hint tip">{t('presence.status_prefixed')}</p>
            {/if}
          </div>
        </section>

        <section class="panel">
          <h2>{t('presence.song_info')}</h2>
          <Toggle bind:checked={s.show_title} label={t('presence.show_title')} />
          <Toggle bind:checked={s.show_artist} label={t('presence.show_artist')} />
          <Toggle bind:checked={s.show_album} label={t('presence.show_album')} />
          <Toggle bind:checked={s.show_art} label={t('presence.show_art')} />
          <Toggle bind:checked={s.show_progress} label={t('presence.show_progress')} />
        </section>

        <section class="panel">
          <h2>{t('presence.layout')}</h2>
          <Toggle bind:checked={s.one_line} label={t('presence.one_line')} disabled={s.custom_templates} />
          <Toggle bind:checked={s.swap_title_artist} label={t('presence.swap')} disabled={s.custom_templates} />
          <Toggle bind:checked={s.prefix_by} label={t('presence.prefix_by')} disabled={s.custom_templates || s.one_line} />
          <Toggle bind:checked={s.prefix_on} label={t('presence.prefix_on')} disabled={s.custom_templates} />
          <Toggle bind:checked={s.custom_templates} label={t('presence.custom_templates')} hint={t('presence.templates_hint')} />
          {#if s.custom_templates}
            <div class="stack inset">
              <label class="field">{t('presence.details_template')}<input bind:value={s.details_template} maxlength="128" /></label>
              <label class="field">{t('presence.state_template')}<input bind:value={s.state_template} maxlength="128" /></label>
              <label class="field">{t('presence.album_template')}<input bind:value={s.album_template} maxlength="128" /></label>
            </div>
          {/if}
        </section>

        <section class="panel stack">
          <h2>{t('presence.hide_words')}</h2>
          <p class="muted small">{t('presence.hide_words_hint')}</p>
          <label class="field">
            {t('presence.hide_words_label')}
            <input bind:value={s.strip_keywords} maxlength="500" placeholder={SUGGESTED} />
          </label>
          {#if !s.strip_keywords.trim()}
            <div>
              <button type="button" onclick={() => (s.strip_keywords = SUGGESTED)}>{t('presence.hide_words_suggest')}</button>
            </div>
          {/if}
        </section>

        <section class="panel">
          <h2>{t('presence.links')}</h2>
          <p class="muted small sub">{t('presence.links_hint')}</p>
          <Toggle bind:checked={s.link_title} label={t('presence.link_title')} />
          <Toggle bind:checked={s.link_artist} label={t('presence.link_artist')} />
          <Toggle bind:checked={s.link_album} label={t('presence.link_album')} />
        </section>

        <section class="panel stack">
          <h2>{t('presence.buttons')}</h2>
          <p class="muted small">{t('presence.buttons_hint')}</p>
          <fieldset class="radios">
            <legend class="small">{t('presence.link_button')}</legend>
            {#each [['last_fm', 'link_lastfm'], ['apple_music', 'link_apple'], ['none', 'link_none']] as [v, k] (v)}
              <label class="radio">
                <input type="radio" name="link" value={v} bind:group={s.link_button} />
                {t(`presence.${k}`)}
              </label>
            {/each}
          </fieldset>
          <div>
            <Toggle bind:checked={s.custom_button} label={t('presence.custom_button')} />
            {#if s.custom_button}
              <div class="row two">
                <label class="field">{t('presence.custom_button_label')}<input bind:value={s.custom_button_label} maxlength="32" /></label>
                <label class="field">{t('presence.custom_button_url')}<input bind:value={s.custom_button_url} placeholder="https://" /></label>
              </div>
            {/if}
            <Toggle bind:checked={s.get_status_button} label={t('presence.get_status')} hint={t('presence.get_status_hint')} />
          </div>
          {#if tooManyButtons}<p class="muted small">{t('presence.too_many_buttons')}</p>{/if}
        </section>

        <section class="panel stack">
          <h2>{t('presence.missing_art')}</h2>
          <fieldset class="radios">
            <label class="radio"><input type="radio" name="missing" value="none" bind:group={s.missing_art} />{t('presence.missing_none')}</label>
            <label class="radio"><input type="radio" name="missing" value="custom" bind:group={s.missing_art} />{t('presence.missing_custom')}</label>
          </fieldset>
          {#if s.missing_art === 'custom'}
            <label class="field">{t('presence.missing_url')}<input bind:value={s.missing_art_url} placeholder="https://" /></label>
          {/if}
        </section>

        {#if error}<p class="error-text" role="alert">{error}</p>{/if}
        <div class="savebar" class:show={dirty}>
          <button class="primary" disabled={busy || !dirty}>{t('common.save')}</button>
        </div>
      </form>

      <aside class="preview">
        <h2 class="small muted">{t('presence.preview')}</h2>
        <MemberPreview activity={preview} user={me} />
        <PresencePreview activity={preview} />
      </aside>
    </div>
  {/if}
</div>

<style>
  .page {
    gap: 1.5rem;
  }
  .head {
    gap: 0.4rem;
  }
  .sk {
    block-size: 20rem;
  }
  .layout {
    display: grid;
    grid-template-columns: minmax(0, 1fr) minmax(16rem, 23rem);
    gap: 1.5rem;
    align-items: start;
  }
  .panel h2 {
    margin-block-end: 0.15rem;
  }
  .sub {
    margin-block-end: 0.25rem;
  }
  .inset {
    padding-block: 0.5rem 0.25rem;
  }
  .two {
    display: grid;
    grid-template-columns: 1fr 1.6fr;
    gap: 0.75rem;
    padding-block-end: 0.5rem;
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
  .tip {
    max-width: 34rem;
  }
  .preview {
    position: sticky;
    top: 1.5rem;
    display: grid;
    gap: 0.75rem;
  }
  .radios {
    border: 0;
    padding: 0;
    margin: 0;
    display: flex;
    flex-wrap: wrap;
    gap: 0.5rem;
  }
  .radios legend {
    font-weight: 550;
    margin-block-end: 0.4rem;
  }
  .radio {
    display: inline-flex;
    gap: 0.45rem;
    align-items: center;
    padding: 0.4rem 0.8rem;
    border: 1px solid var(--line-strong);
    border-radius: 999px;
    cursor: pointer;
    font-size: var(--step--1);
  }
  .radio:has(input:checked) {
    border-color: var(--text);
    background: var(--surface-3);
  }
  .radio input {
    accent-color: var(--text);
  }
  .savebar.show {
    position: sticky;
    bottom: 1rem;
  }
  @media (max-width: 52rem) {
    .layout {
      grid-template-columns: 1fr;
    }
    .preview {
      position: static;
      order: -1;
    }
    .two {
      grid-template-columns: 1fr;
    }
  }
</style>
