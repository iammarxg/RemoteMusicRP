// SPDX-License-Identifier: AGPL-3.0-only
// Theme choice: 'system' | 'dark' | 'light'. Remembered per browser in
// localStorage (a per-viewer convenience; works without it).

const KEY = 'rsrp-theme';

function read() {
  try {
    const v = localStorage.getItem(KEY);
    return v === 'dark' || v === 'light' ? v : 'system';
  } catch {
    return 'system';
  }
}

export const theme = $state({ value: read() });

export function applyTheme(value) {
  theme.value = value;
  const root = document.documentElement;
  if (value === 'system') root.removeAttribute('data-theme');
  else root.setAttribute('data-theme', value);
  try {
    if (value === 'system') localStorage.removeItem(KEY);
    else localStorage.setItem(KEY, value);
  } catch {
    /* storage unavailable: theme still applies for this visit */
  }
}

applyTheme(theme.value);
