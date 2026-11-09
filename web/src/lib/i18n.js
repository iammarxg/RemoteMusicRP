// SPDX-License-Identifier: AGPL-3.0-only
// All UI strings live in src/locales/<lang>.json (translated on Weblate).
// RTL languages flip the document direction; layout uses logical CSS
// properties so it mirrors correctly.

import en from '../locales/en.json';

const locales = { en };
const RTL = new Set(['ar', 'he', 'fa', 'ur']);

let current = 'en';
let messages = en;

export function setLocale(lang) {
  const base = (lang || 'en').toLowerCase().split('-')[0];
  current = locales[base] ? base : 'en';
  messages = locales[current];
  document.documentElement.lang = current;
  document.documentElement.dir = RTL.has(current) ? 'rtl' : 'ltr';
}

/** Translate `key`, replacing `{name}` placeholders from `params`. */
export function t(key, params) {
  let s = key.split('.').reduce((o, k) => (o == null ? o : o[k]), messages);
  if (typeof s !== 'string') s = key.split('.').reduce((o, k) => (o == null ? o : o[k]), en);
  if (typeof s !== 'string') return key;
  if (params) s = s.replace(/\{(\w+)\}/g, (m, k) => (k in params ? String(params[k]) : m));
  return s;
}

/** A list of strings (e.g. setup steps). */
export function tl(key) {
  const get = (m) => key.split('.').reduce((o, k) => (o == null ? o : o[k]), m);
  const v = get(messages);
  if (Array.isArray(v)) return v;
  const f = get(en);
  return Array.isArray(f) ? f : [];
}

export function locale() {
  return current;
}

/** "3 min ago"-style relative time using the browser's Intl support. */
export function ago(unix) {
  const diff = unix - Date.now() / 1000;
  const rtf = new Intl.RelativeTimeFormat(current, { numeric: 'auto' });
  const abs = Math.abs(diff);
  if (abs < 60) return rtf.format(Math.round(diff), 'second');
  if (abs < 3600) return rtf.format(Math.round(diff / 60), 'minute');
  if (abs < 86400) return rtf.format(Math.round(diff / 3600), 'hour');
  return rtf.format(Math.round(diff / 86400), 'day');
}

export function dateTime(unix) {
  return new Intl.DateTimeFormat(current, { dateStyle: 'medium', timeStyle: 'short' }).format(unix * 1000);
}

/** m:ss for track times. */
export function clock(secs) {
  const s = Math.max(0, Math.floor(secs));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
}

setLocale(navigator.language);
