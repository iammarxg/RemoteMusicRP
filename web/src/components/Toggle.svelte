<!-- SPDX-License-Identifier: AGPL-3.0-only -->
<script>
  let { checked = $bindable(false), label, hint = '', disabled = false } = $props();
</script>

<label class="toggle" class:disabled>
  <span class="text">
    <span class="label">{label}</span>
    {#if hint}<span class="hint">{hint}</span>{/if}
  </span>
  <input type="checkbox" role="switch" bind:checked {disabled} />
  <span class="track" aria-hidden="true"><span class="thumb"></span></span>
</label>

<style>
  .toggle {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 1rem;
    padding: 0.7rem 0;
    cursor: pointer;
    position: relative;
  }
  .toggle + :global(.toggle) {
    border-block-start: 1px solid var(--line);
  }
  .disabled {
    opacity: 0.55;
    cursor: not-allowed;
  }
  .text {
    display: grid;
  }
  .label {
    font-weight: 600;
  }
  .hint {
    color: var(--muted);
    font-size: var(--step--1);
  }
  input {
    position: absolute;
    opacity: 0;
    inline-size: 1px;
    block-size: 1px;
  }
  .track {
    flex: none;
    inline-size: 2.6rem;
    block-size: 1.5rem;
    border-radius: 999px;
    background: var(--line);
    position: relative;
    transition: background-color 0.18s;
  }
  .thumb {
    position: absolute;
    inset-block-start: 0.2rem;
    inset-inline-start: 0.2rem;
    inline-size: 1.1rem;
    block-size: 1.1rem;
    border-radius: 50%;
    background: var(--text);
    transition: transform 0.18s cubic-bezier(0.3, 1.4, 0.6, 1);
  }
  input:checked + .track {
    background: var(--accent);
  }
  input:checked + .track .thumb {
    transform: translateX(1.1rem);
    background: var(--accent-ink);
  }
  :global([dir='rtl']) input:checked + .track .thumb {
    transform: translateX(-1.1rem);
  }
  input:focus-visible + .track {
    outline: 3px solid var(--focus);
    outline-offset: 2px;
  }
</style>
