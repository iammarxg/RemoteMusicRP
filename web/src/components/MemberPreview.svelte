<!-- SPDX-License-Identifier: AGPL-3.0-only -->
<!-- How you appear in the member list / DM list (status_display_type). -->
<script>
  import { t } from '../lib/i18n.js';

  let { activity = null, user = null } = $props();

  const STATUS_KEYS = ['online', 'idle', 'dnd', 'invisible', 'offline', 'streaming'];

  let line = $derived.by(() => {
    if (!activity) return null;
    if (activity.status_display_type === 2 && activity.details) return activity.details;
    if (activity.status_display_type === 1 && activity.state) return activity.state;
    return activity.name ?? t('preview.app_name');
  });
  // Activity type → glyph. Competing (5) uses the same controller as Playing (0).
  let kind = $derived(
    activity?.type === 2 || activity?.type === undefined ? 'listening' : activity.type === 3 ? 'watching' : 'playing',
  );
</script>

<div class="member" aria-label={t('preview.member_list')}>
  <span class="pic">
    {#if user?.avatar}
      <img class="avatar" src={user.avatar} alt="" referrerpolicy="no-referrer" />
    {:else}
      <span class="avatar" aria-hidden="true"></span>
    {/if}
    {#if user?.status && STATUS_KEYS.includes(user.status)}
      <!-- Monochrome status badge; the shape tells them apart, like
           Discord's: filled dot, crescent, bar, ring. -->
      <span class="badge" role="img" aria-label={t(`preview.status_${user.status}`)} title={t(`preview.status_${user.status}`)}>
        <svg viewBox="0 0 10 10" width="10" height="10" aria-hidden="true">
          {#if user.status === 'online' || user.status === 'streaming'}
            <circle cx="5" cy="5" r="5" fill="currentColor" />
          {:else if user.status === 'idle'}
            <circle cx="5" cy="5" r="5" fill="currentColor" />
            <circle cx="2.6" cy="2.6" r="3.3" fill="var(--surface)" />
          {:else if user.status === 'dnd'}
            <path fill="currentColor" fill-rule="evenodd" d="M5 0a5 5 0 1 1 0 10A5 5 0 0 1 5 0zM2.5 4.1v1.8h5V4.1z" />
          {:else}
            <path fill="currentColor" fill-rule="evenodd" d="M5 0a5 5 0 1 1 0 10A5 5 0 0 1 5 0zm0 2.5a2.5 2.5 0 1 0 0 5 2.5 2.5 0 0 0 0-5z" />
          {/if}
        </svg>
      </span>
    {/if}
  </span>
  <span class="who">
    <span class="you">{user?.name ?? t('preview.you')}</span>
    {#if line}
      <span class="status small">
        <!-- Our own drawings of the three activity glyphs Discord shows
             (not Discord's assets): note, controller (Playing/Competing),
             screen (Watching). -->
        <svg class="glyph" viewBox="0 0 24 24" width="14" height="14" aria-hidden="true">
          {#if kind === 'listening'}
            <path
              fill="currentColor"
              d="M20 3.2v12.3a3 3 0 1 1-2-2.83V8.4l-8 1.9v7.2a3 3 0 1 1-2-2.83V5.8a1 1 0 0 1 .77-.97l10-2.4A1 1 0 0 1 20 3.2z"
            />
          {:else if kind === 'watching'}
            <path
              fill="currentColor"
              d="M4 4h16a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2h-6.5l.8 1.6a.7.7 0 0 1-.63 1H10.3a.7.7 0 0 1-.63-1l.8-1.6H4a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2z"
            />
          {:else}
            <path
              fill="currentColor"
              fill-rule="evenodd"
              d="M7.2 6h9.6a5 5 0 0 1 4.9 4l.97 4.86a2.9 2.9 0 0 1-5.13 2.36L16 15.5H8l-1.54 1.72a2.9 2.9 0 0 1-5.13-2.36L2.3 10a5 5 0 0 1 4.9-4zM7 9v1.6H5.4v1.8H7V14h1.8v-1.6h1.6v-1.8H8.8V9H7zm8.5.6a1.1 1.1 0 1 0 0 2.2 1.1 1.1 0 0 0 0-2.2zm2.3 2.3a1.1 1.1 0 1 0 0 2.2 1.1 1.1 0 0 0 0-2.2z"
            />
          {/if}
        </svg>
        {line}
      </span>
    {/if}
  </span>
</div>

<style>
  .member {
    display: flex;
    align-items: center;
    gap: 0.7rem;
    padding: 0.55rem 0.75rem;
    border: 1px solid var(--line);
    border-radius: var(--radius-m);
    background: var(--surface);
    max-width: 26rem;
  }
  .pic {
    position: relative;
    flex: none;
    display: grid;
  }
  .badge {
    position: absolute;
    inset-block-end: -2px;
    inset-inline-end: -2px;
    display: grid;
    place-items: center;
    inline-size: 14px;
    block-size: 14px;
    border-radius: 50%;
    background: var(--surface);
    color: var(--text);
  }
  .avatar {
    inline-size: 2rem;
    block-size: 2rem;
    border-radius: 50%;
    background: var(--surface-3);
    flex: none;
    object-fit: cover;
  }
  .who {
    display: grid;
    min-width: 0;
  }
  .you {
    font-weight: 600;
    font-size: var(--step--1);
  }
  .status {
    display: flex;
    gap: 0.35rem;
    align-items: center;
    color: var(--muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  /* Discord's activity green: the one colour Discord itself uses here. */
  .glyph {
    flex: none;
    color: #23a55a;
  }
</style>
