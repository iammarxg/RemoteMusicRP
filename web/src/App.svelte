<!-- SPDX-License-Identifier: AGPL-3.0-only -->
<script>
  import { onMount } from 'svelte';
  import { fade } from 'svelte/transition';
  import { loadState, api } from './lib/api.js';
  import { t } from './lib/i18n.js';
  import { app, go, toast } from './lib/stores.svelte.js';
  import Toasts from './components/Toasts.svelte';
  import Logo from './components/Logo.svelte';
  import Icon from './components/Icon.svelte';
  import ThemeSwitch from './components/ThemeSwitch.svelte';
  import Unlock from './pages/Unlock.svelte';
  import Login from './pages/Login.svelte';
  import Wizard from './pages/Wizard.svelte';
  import Dashboard from './pages/Dashboard.svelte';
  import Presence from './pages/Presence.svelte';
  import Connections from './pages/Connections.svelte';
  import Settings from './pages/Settings.svelte';
  import Security from './pages/Security.svelte';
  import Activity from './pages/Activity.svelte';
  import Rules from './pages/Rules.svelte';
  import About from './pages/About.svelte';

  const pages = {
    dashboard: Dashboard,
    presence: Presence,
    rules: Rules,
    connections: Connections,
    settings: Settings,
    security: Security,
    activity: Activity,
    about: About,
  };
  const nav = ['dashboard', 'presence', 'rules', 'connections', 'settings', 'security', 'activity', 'about'];

  let failed = $state(false);
  let menuOpen = $state(false);

  async function refresh() {
    try {
      app.boot = await loadState();
      failed = false;
    } catch {
      failed = true;
    }
  }

  // Discord sends the browser back here after a server-mode sign-in.
  let callback = null;
  if (location.pathname === '/discord-callback') {
    const q = new URLSearchParams(location.search);
    callback = { code: q.get('code'), state: q.get('state'), error: q.get('error') };
    history.replaceState(null, '', '/#/connections');
    app.route = 'connections';
  }
  async function finishCallback() {
    const cb = callback;
    callback = null;
    if (cb.error || !cb.code || !cb.state) {
      toast(t('sdk.callback_denied'), 'error');
      return;
    }
    try {
      await api.post('/api/discord/sdk/finish', { code: cb.code, state: cb.state });
      toast(t('sdk.signed_in'));
    } catch (e) {
      toast(e.message, 'error');
    }
  }
  $effect(() => {
    if (mode === 'app' && callback) finishCallback();
  });

  onMount(refresh);

  // Close the mobile menu after navigating.
  $effect(() => {
    app.route;
    menuOpen = false;
  });

  let Page = $derived(pages[app.route] ?? Dashboard);
  let b = $derived(app.boot);
  let mode = $derived(
    !b ? 'loading' : b.locked ? 'locked' : !b.authenticated ? 'login' : !b.setup.complete ? 'wizard' : 'app',
  );

  async function logout() {
    try {
      await api.post('/api/auth/logout');
    } finally {
      await refresh();
    }
  }
</script>

{#if failed}
  <main class="center">
    <div class="panel stack narrow" role="alert">
      <Logo />
      <p>{t('app.unreachable')}</p>
      <div><button class="primary" onclick={() => location.reload()}>{t('app.reload')}</button></div>
    </div>
  </main>
{:else if mode === 'loading'}
  <main class="center" aria-busy="true"><p class="muted">{t('app.loading')}</p></main>
{:else if mode === 'locked'}
  <Unlock onDone={refresh} />
{:else if mode === 'login'}
  <Login onDone={refresh} />
{:else if mode === 'wizard'}
  <Wizard boot={b} onChange={refresh} />
{:else}
  <div class="shell">
    <header class="side">
      <div class="spread brand-row">
        <Logo />
        <button
          class="ghost menu-btn"
          aria-expanded={menuOpen}
          aria-controls="main-nav"
          aria-label={t('nav.menu')}
          onclick={() => (menuOpen = !menuOpen)}><Icon name="menu" /></button
        >
      </div>
      <nav id="main-nav" class:open={menuOpen} aria-label={t('nav.menu')}>
        {#each nav as r (r)}
          <a href={`#/${r}`} aria-current={app.route === r ? 'page' : undefined}>
            <Icon name={r} />{t(`nav.${r}`)}
          </a>
        {/each}
        <div class="nav-foot">
          <ThemeSwitch />
          {#if b.auth_required}
            <button class="ghost logout" onclick={logout}><Icon name="logout" />{t('nav.logout')}</button>
          {/if}
        </div>
      </nav>
    </header>
    <main class="content">
      {#key app.route}
        <div in:fade={{ duration: 140 }}>
          <Page boot={b} onChange={refresh} />
        </div>
      {/key}
    </main>
  </div>
{/if}

<Toasts />

<style>
  .center {
    min-height: 100dvh;
    display: grid;
    place-items: center;
    padding: 1.25rem;
  }
  .narrow {
    max-width: 30rem;
  }
  .shell {
    display: grid;
    grid-template-columns: 14.5rem 1fr;
    min-height: 100dvh;
  }
  .side {
    position: sticky;
    top: 0;
    align-self: start;
    height: 100dvh;
    padding: 1.25rem 0.85rem;
    display: grid;
    grid-template-rows: auto 1fr;
    gap: 1.75rem;
    border-inline-end: 1px solid var(--line);
    background: var(--bg);
  }
  .brand-row {
    padding-inline: 0.4rem;
  }
  nav {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  nav a {
    display: flex;
    align-items: center;
    gap: 0.7rem;
    color: var(--muted);
    text-decoration: none;
    padding: 0.5rem 0.7rem;
    border-radius: var(--radius-m);
    font-weight: 500;
    font-size: var(--step--1);
    transition:
      color 0.15s,
      background-color 0.15s;
  }
  nav a:hover {
    color: var(--text);
    background: var(--surface-2);
  }
  nav a[aria-current='page'] {
    color: var(--accent-ink);
    background: var(--accent);
    font-weight: 600;
  }
  .nav-foot {
    margin-block-start: auto;
    display: grid;
    gap: 0.5rem;
    justify-items: start;
    padding-block-start: 1rem;
    border-block-start: 1px solid var(--line);
  }
  .logout {
    gap: 0.7rem;
    padding-inline: 0.7rem;
  }
  .menu-btn {
    display: none;
  }
  .content {
    padding: clamp(1.25rem, 4vw, 3rem);
    max-width: 68rem;
    width: 100%;
  }
  @media (max-width: 52rem) {
    .shell {
      grid-template-columns: 1fr;
    }
    .side {
      position: static;
      height: auto;
      padding: 0.9rem 1rem;
      gap: 0.5rem;
      border-inline-end: 0;
      border-block-end: 1px solid var(--line);
    }
    .menu-btn {
      display: inline-flex;
    }
    nav {
      display: none;
    }
    nav.open {
      display: flex;
    }
  }
</style>
