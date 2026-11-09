// SPDX-License-Identifier: AGPL-3.0-only
// Shared reactive state (Svelte 5 runes).

export const app = $state({
  boot: null, // /api/state payload
  route: location.hash.slice(2) || 'dashboard',
});

export const toasts = $state({ items: [] });

let nextId = 1;
export function toast(message, kind = 'ok') {
  const id = nextId++;
  toasts.items.push({ id, message, kind });
  setTimeout(() => {
    toasts.items = toasts.items.filter((x) => x.id !== id);
  }, kind === 'error' ? 6000 : 3500);
}

export function go(route) {
  location.hash = `#/${route}`;
}

window.addEventListener('hashchange', () => {
  app.route = location.hash.slice(2) || 'dashboard';
});

/** A clock that ticks once per second while something needs it. */
export const now = $state({ unix: Date.now() / 1000 });
setInterval(() => {
  now.unix = Date.now() / 1000;
}, 1000);
