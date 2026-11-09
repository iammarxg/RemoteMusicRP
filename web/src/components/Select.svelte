<!-- SPDX-License-Identifier: AGPL-3.0-only -->
<!-- Accessible custom dropdown (button + listbox). Keyboard: Up/Down/Home/
     End move, Enter/Space choose, Escape closes. -->
<script>
  import { fly } from 'svelte/transition';

  let { value = $bindable(), options = [], label = '' } = $props();
  // options: [{ value, label }]

  let open = $state(false);
  let active = $state(0);
  let root;
  let button;
  const id = `sel-${Math.random().toString(36).slice(2, 9)}`;

  let current = $derived(options.find((o) => o.value === value) ?? options[0]);

  function show() {
    active = Math.max(0, options.findIndex((o) => o.value === value));
    open = true;
  }
  function choose(i) {
    value = options[i].value;
    open = false;
    button?.focus();
  }
  function onKey(e) {
    if (!open && ['ArrowDown', 'ArrowUp', 'Enter', ' '].includes(e.key)) {
      e.preventDefault();
      show();
      return;
    }
    if (!open) return;
    const last = options.length - 1;
    if (e.key === 'ArrowDown') active = Math.min(last, active + 1);
    else if (e.key === 'ArrowUp') active = Math.max(0, active - 1);
    else if (e.key === 'Home') active = 0;
    else if (e.key === 'End') active = last;
    else if (e.key === 'Enter' || e.key === ' ') choose(active);
    else if (e.key === 'Escape' || e.key === 'Tab') {
      open = false;
      if (e.key === 'Escape') {
        e.preventDefault();
        return;
      }
      return;
    } else return;
    e.preventDefault();
  }
  function outside(e) {
    if (open && root && !root.contains(e.target)) open = false;
  }
</script>

<svelte:window onpointerdown={outside} />

<div class="select" bind:this={root}>
  <button
    type="button"
    bind:this={button}
    class="trigger"
    class:open
    aria-haspopup="listbox"
    aria-expanded={open}
    aria-controls={id}
    aria-label={label ? `${label}: ${current?.label ?? ''}` : undefined}
    aria-activedescendant={open ? `${id}-${active}` : undefined}
    onclick={() => (open ? (open = false) : show())}
    onkeydown={onKey}
  >
    <span class="value">{current?.label ?? ''}</span>
    <svg class="chev" viewBox="0 0 24 24" width="16" height="16" aria-hidden="true"
      ><path d="M6 9l6 6 6-6" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"
        stroke-linejoin="round" /></svg
    >
  </button>
  {#if open}
    <ul class="menu" role="listbox" {id} transition:fly={{ y: -4, duration: 140 }}>
      {#each options as o, i (o.value)}
        <li
          id={`${id}-${i}`}
          role="option"
          aria-selected={o.value === value}
          class:active={i === active}
          onpointerenter={() => (active = i)}
          onpointerdown={(e) => {
            e.preventDefault();
            choose(i);
          }}
        >
          <span>{o.label}</span>
          {#if o.value === value}
            <svg viewBox="0 0 24 24" width="15" height="15" aria-hidden="true"
              ><path d="M5 12l5 5 9-10" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"
                stroke-linejoin="round" /></svg
            >
          {/if}
        </li>
      {/each}
    </ul>
  {/if}
</div>

<style>
  .select {
    position: relative;
    max-width: 22rem;
  }
  .trigger {
    width: 100%;
    justify-content: space-between;
    gap: 1rem;
    font-weight: 500;
    font-size: var(--step-0);
    background: var(--surface);
    border: 1px solid var(--line-strong);
    border-radius: var(--radius-m);
    padding-block: 0.55rem;
    padding-inline: 0.8rem 0.75rem;
    text-align: start;
  }
  .trigger:hover:not(:disabled),
  .trigger.open {
    background: var(--surface);
    border-color: var(--text);
  }
  .value {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .chev {
    flex: none;
    color: var(--muted);
    transition:
      transform 0.2s cubic-bezier(0.3, 1.3, 0.6, 1),
      color 0.15s;
  }
  .trigger.open .chev {
    transform: rotate(180deg);
    color: var(--text);
  }
  .menu {
    position: absolute;
    inset-inline: 0;
    inset-block-start: calc(100% + 6px);
    z-index: 20;
    margin: 0;
    padding: 4px;
    list-style: none;
    background: var(--surface-2);
    border: 1px solid var(--line-strong);
    border-radius: var(--radius-m);
    box-shadow: var(--shadow);
    max-block-size: 18rem;
    overflow: auto;
  }
  li {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 0.75rem;
    padding: 0.5rem 0.65rem;
    border-radius: var(--radius-s);
    cursor: pointer;
    font-size: var(--step--1);
    color: var(--muted);
  }
  li.active {
    background: var(--surface-3);
    color: var(--text);
  }
  li[aria-selected='true'] {
    color: var(--text);
    font-weight: 600;
  }
</style>
