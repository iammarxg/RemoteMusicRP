// SPDX-License-Identifier: AGPL-3.0-only
import { mount } from 'svelte';
import './styles.css';
import './lib/theme.svelte.js';
import App from './App.svelte';

mount(App, { target: document.getElementById('app') });
