<!-- SPDX-License-Identifier: AGPL-3.0-only -->
<script>
  import { fly } from 'svelte/transition';
  import { toasts } from '../lib/stores.svelte.js';
</script>

<div class="toasts" role="status" aria-live="polite">
  {#each toasts.items as item (item.id)}
    <div class="toast {item.kind}" transition:fly={{ y: 12, duration: 180 }}>{item.message}</div>
  {/each}
</div>

<style>
  .toasts {
    position: fixed;
    inset-block-end: 1.25rem;
    inset-inline-end: 1.25rem;
    display: grid;
    gap: 0.5rem;
    z-index: 10;
    max-width: min(26rem, calc(100vw - 2.5rem));
  }
  .toast {
    background: var(--accent);
    color: var(--accent-ink);
    border-radius: var(--radius-m);
    padding: 0.7rem 1rem;
    box-shadow: var(--shadow);
    font-weight: 550;
    font-size: var(--step--1);
  }
  .toast.error {
    background: var(--danger);
    color: #fff;
  }
</style>
