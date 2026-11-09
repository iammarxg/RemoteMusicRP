<!-- SPDX-License-Identifier: AGPL-3.0-only -->
<!-- Renders the exact activity JSON RemoteMusicRP sends to Discord. -->
<script>
  import { t, clock } from '../lib/i18n.js';
  import { now } from '../lib/stores.svelte.js';

  let { activity = null } = $props();

  let ts = $derived(activity?.timestamps);
  let elapsed = $derived(ts?.start ? now.unix - ts.start : null);
  let total = $derived(ts?.start && ts?.end ? ts.end - ts.start : null);
  let pct = $derived(total ? Math.min(100, Math.max(0, (elapsed / total) * 100)) : null);
</script>

<div class="card" aria-label={t('dashboard.on_discord')}>
  <p class="head small">{t(`preview.verb_${activity?.type ?? 2}`)} {activity?.name ?? t('preview.app_name')}</p>
  {#if activity}
    <div class="body">
      {#if activity.assets?.large_image?.startsWith('https://')}
        <img
          class="art"
          src={activity.assets.large_image}
          alt={activity.assets.large_text ?? ''}
          title={activity.assets.large_text ?? ''}
          referrerpolicy="no-referrer"
          loading="lazy"
        />
      {:else}
        <div class="art placeholder" aria-hidden="true"></div>
      {/if}
      <div class="lines">
        {#if activity.details}<p class="details">{activity.details}</p>{/if}
        {#if activity.state}<p class="state">{activity.state}</p>{/if}
        {#if activity.assets?.large_text}<p class="state">{activity.assets.large_text}</p>{/if}
        {#if pct !== null}
          <div class="progress tabular small">
            <span>{clock(elapsed)}</span>
            <span class="bar"><span style:inline-size={`${pct}%`}></span></span>
            <span>{clock(total)}</span>
          </div>
        {:else if elapsed !== null}
          <p class="small tabular muted">{clock(elapsed)}</p>
        {/if}
      </div>
    </div>
    {#if activity.buttons?.length}
      <div class="buttons">
        {#each activity.buttons as b (b.url + b.label)}
          <a class="pbtn" href={b.url} target="_blank" rel="noopener noreferrer">{b.label}</a>
        {/each}
      </div>
    {/if}
  {:else}
    <p class="muted small empty">{t('preview.empty')}</p>
  {/if}
</div>

<style>
  .card {
    background: color-mix(in srgb, var(--ink) 70%, var(--surface));
    border: 1px solid var(--line);
    border-radius: var(--radius-m);
    padding: 0.9rem;
    display: grid;
    gap: 0.6rem;
    max-width: 26rem;
  }
  .head {
    font-weight: 650;
  }
  .body {
    display: grid;
    grid-template-columns: 5.5rem 1fr;
    gap: 0.85rem;
    align-items: center;
  }
  .art {
    inline-size: 5.5rem;
    block-size: 5.5rem;
    border-radius: var(--radius-s);
    object-fit: cover;
    background: var(--surface-2);
  }
  .lines {
    min-width: 0;
    display: grid;
    gap: 0.1rem;
  }
  .lines p {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .details {
    font-weight: 700;
  }
  .state {
    font-size: var(--step--1);
  }
  .progress {
    display: grid;
    grid-template-columns: auto 1fr auto;
    align-items: center;
    gap: 0.5rem;
    margin-block-start: 0.35rem;
  }
  .bar {
    block-size: 4px;
    background: var(--line);
    border-radius: 2px;
    overflow: hidden;
  }
  .bar span {
    display: block;
    block-size: 100%;
    background: var(--text);
    transition: inline-size 1s linear;
  }
  .buttons {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 0.5rem;
  }
  .pbtn {
    background: var(--surface-2);
    color: var(--text);
    text-decoration: none;
    text-align: center;
    padding: 0.45rem 0.6rem;
    border-radius: var(--radius-s);
    font-weight: 600;
    font-size: var(--step--1);
  }
  .pbtn:hover {
    background: var(--line);
  }
  .empty {
    padding-block: 0.6rem;
  }
</style>
