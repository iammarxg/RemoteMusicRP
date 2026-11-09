<!-- SPDX-License-Identifier: AGPL-3.0-only -->
<script>
  import { t } from '../lib/i18n.js';
</script>

<span class="logo">
  <svg viewBox="0 0 64 64" aria-hidden="true">
    <rect x="2" y="2" width="60" height="60" rx="16" fill="var(--text)" />
    <circle cx="32" cy="32" r="19" fill="none" stroke="var(--bg)" stroke-width="3" />
    <circle cx="32" cy="32" r="11" fill="none" stroke="var(--bg)" stroke-width="3" />
    <circle cx="32" cy="32" r="3.5" fill="var(--bg)" />
  </svg>
  <span class="word">{t('app.name')}</span>
</span>

<style>
  .logo {
    display: inline-flex;
    align-items: center;
    gap: 0.6rem;
    font-weight: 650;
    font-size: var(--step-0);
    letter-spacing: -0.02em;
  }
  svg {
    inline-size: 1.6rem;
    block-size: 1.6rem;
  }
</style>
