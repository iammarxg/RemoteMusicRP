<!-- SPDX-License-Identifier: AGPL-3.0-only -->
<script>
  import { t } from '../lib/i18n.js';
  import { theme, applyTheme } from '../lib/theme.svelte.js';
  import Icon from './Icon.svelte';

  const options = [
    ['system', 'monitor'],
    ['dark', 'moon'],
    ['light', 'sun'],
  ];
</script>

<div class="switch" role="radiogroup" aria-label={t('theme.label')}>
  {#each options as [value, icon] (value)}
    <button
      type="button"
      role="radio"
      aria-checked={theme.value === value}
      title={t(`theme.${value}`)}
      onclick={() => applyTheme(value)}
    >
      <Icon name={icon} size={15} />
      <span class="sr">{t(`theme.${value}`)}</span>
    </button>
  {/each}
</div>

<style>
  .switch {
    display: inline-grid;
    grid-auto-flow: column;
    gap: 2px;
    padding: 3px;
    border: 1px solid var(--line);
    border-radius: 999px;
    background: var(--surface);
  }
  button {
    border: 0;
    border-radius: 999px;
    min-block-size: 0;
    padding: 0.35rem 0.6rem;
    color: var(--muted);
    background: transparent;
  }
  button:hover:not(:disabled) {
    color: var(--text);
    background: transparent;
  }
  button[aria-checked='true'] {
    background: var(--surface-3);
    color: var(--text);
  }
  .sr {
    position: absolute;
    inline-size: 1px;
    block-size: 1px;
    overflow: hidden;
    clip-path: inset(50%);
  }
</style>
