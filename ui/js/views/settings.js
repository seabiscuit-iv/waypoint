// Auth gate (§5.1), settings modal (§5.1/5.3, backup §5.7, usage §5.9),
// theme handling, and the keyboard-shortcut cheat sheet.

import { qs, el, fmtCost, fmtTokens, toast, toastErr, openModal, confirmModal } from '../util.js';
import { api } from '../api.js';
import { state } from '../state.js';

const MODELS = [
  { id: 'claude-opus-5', name: 'Claude Opus 5', sub: 'Deepest teaching quality — the default', price: '$5 / $25 per MTok' },
  { id: 'claude-sonnet-5', name: 'Claude Sonnet 5', sub: 'Near-Opus quality, faster and cheaper', price: '$3 / $15 per MTok' },
  { id: 'claude-haiku-4-5', name: 'Claude Haiku 4.5', sub: 'Fastest and most economical', price: '$1 / $5 per MTok' },
];

const STEP_SIZES = [
  { id: 'brief', label: 'Brief', sub: '1–2 short paragraphs per step — the tightest focus.' },
  { id: 'standard', label: 'Standard', sub: '2–3 short paragraphs per step — the design default.' },
  { id: 'deep', label: 'Roomier', sub: '3–5 paragraphs per step, worked examples welcome.' },
];

const THEMES = [
  { id: 'system', label: 'System' },
  { id: 'light', label: 'Light' },
  { id: 'dark', label: 'Dark' },
];

export function applyTheme(theme) {
  const root = document.documentElement;
  if (theme === 'light' || theme === 'dark') root.dataset.theme = theme;
  else delete root.dataset.theme;
}

async function saveSettings(patch) {
  try {
    state.settings = await api.updateSettings({ ...state.settings, ...patch });
    applyTheme(state.settings.theme);
  } catch (e) {
    toastErr(e);
  }
}

// ------------------------------------------------------------- gate

let onEnteredCb = null;

export function initGate(onEntered) {
  onEnteredCb = onEntered;
  qs('#gate-save').addEventListener('click', () => connect());
  qs('#gate-key').addEventListener('keydown', (e) => {
    if (e.key === 'Enter') connect();
  });
  qs('#gate-anyway').addEventListener('click', () => enterApp());
}

export function showGate() {
  qs('#gate').hidden = false;
  qs('#gate-key').focus();
}

function setGateStatus(kind, msg) {
  const status = qs('#gate-status');
  status.hidden = false;
  status.className = 'gate-status ' + kind;
  status.textContent = msg;
}

async function connect() {
  const input = qs('#gate-key');
  const key = input.value.trim();
  if (!key) {
    input.focus();
    return;
  }
  qs('#gate-anyway').hidden = true;
  qs('#gate-save').disabled = true;
  setGateStatus('busy', 'Storing the key in Windows Credential Manager and testing the connection…');
  try {
    state.auth = await api.setApiKey(key);
    const res = await api.testApiKey(null);
    if (res.ok) {
      setGateStatus('ok', res.message);
      setTimeout(() => enterApp(), 350);
    } else {
      setGateStatus('err', res.message + ' The key was stored — fix it here or continue anyway.');
      qs('#gate-anyway').hidden = false;
    }
  } catch (e) {
    setGateStatus('err', e?.message || String(e));
  } finally {
    qs('#gate-save').disabled = false;
  }
}

function enterApp() {
  if (!state.auth.configured) return;
  qs('#gate').hidden = true;
  qs('#gate-key').value = '';
  qs('#gate-status').hidden = true;
  onEnteredCb?.();
}

// ------------------------------------------------------------- settings modal

export function openSettings() {
  const body = el('div', {});
  body.append(connectionSection(), modelSection(), stepSizeSection(), themeSection(), usageSection(), dataSection());
  openModal({ title: 'Settings', body, wide: true });
}

function sectionEl(title, ...children) {
  return el('div', { class: 'set-section' }, el('h3', { text: title }), ...children);
}

function connectionSection() {
  const statusRow = el('div', { class: 'key-status-row' });
  const renderStatus = () => {
    statusRow.textContent = '';
    if (state.auth.configured) {
      statusRow.append(
        el('span', { class: 'dot-ok' }),
        el('span', { class: 'mono', text: state.auth.masked_key || '•••' }),
        el('span', { class: 'dim', text: 'stored in Credential Manager' }));
    } else {
      statusRow.append(
        el('span', { class: 'dot-bad' }),
        el('span', { class: 'mono', text: 'No API key configured' }));
    }
  };
  renderStatus();

  const keyInput = el('input', { type: 'password', placeholder: 'Paste a new key: sk-ant-…', spellcheck: 'false', class: 'mono' });
  const feedback = el('div', { class: 'set-note' });

  const saveBtn = el('button', {
    class: 'btn btn-primary btn-sm', text: 'Save & test',
    onclick: async () => {
      const key = keyInput.value.trim();
      if (!key) return;
      saveBtn.disabled = true;
      feedback.textContent = 'Testing…';
      try {
        state.auth = await api.setApiKey(key);
        renderStatus();
        keyInput.value = '';
        const res = await api.testApiKey(null);
        feedback.textContent = res.message;
      } catch (e) {
        feedback.textContent = e?.message || String(e);
      } finally {
        saveBtn.disabled = false;
      }
    },
  });

  const testBtn = el('button', {
    class: 'btn btn-ghost btn-sm', text: 'Test connection',
    onclick: async () => {
      feedback.textContent = 'Testing…';
      try {
        const res = await api.testApiKey(null);
        feedback.textContent = res.message;
      } catch (e) {
        feedback.textContent = e?.message || String(e);
      }
    },
  });

  const removeBtn = el('button', {
    class: 'btn btn-ghost btn-sm', text: 'Remove key',
    onclick: async () => {
      const ok = await confirmModal({
        title: 'Remove API key?',
        message: 'Waypoint can’t generate anything without a key. Your topics stay on disk.',
        confirmLabel: 'Remove',
      });
      if (!ok) return;
      try {
        state.auth = await api.clearApiKey();
        renderStatus();
        feedback.textContent = 'Key removed.';
        showGate();
      } catch (e) {
        toastErr(e);
      }
    },
  });

  return sectionEl('Connection',
    statusRow,
    el('div', { class: 'field-row' }, keyInput, saveBtn),
    el('div', { class: 'btn-row', style: 'margin-top:8px' }, testBtn, removeBtn),
    feedback,
    el('div', { class: 'set-note', text: 'Anthropic account sign-in isn’t available to third-party apps yet — an API key is the supported way to connect.' }));
}

function modelSection() {
  const wrap = el('div', { class: 'radio-cards' });
  const render = () => {
    wrap.textContent = '';
    for (const m of MODELS) {
      wrap.append(el('button', {
        class: 'radio-card' + (state.settings.model === m.id ? ' selected' : ''),
        onclick: async () => {
          await saveSettings({ model: m.id });
          render();
        },
      },
        el('span', { class: 'rc-radio' }),
        el('span', { class: 'rc-body' },
          el('span', { class: 'rc-title', text: m.name }),
          el('div', { class: 'rc-sub', text: m.sub })),
        el('span', { class: 'rc-price', text: m.price })));
    }
  };
  render();
  return sectionEl('Model', wrap,
    el('div', { class: 'set-note', text: 'Concept-ledger housekeeping always runs on Haiku 4.5 to keep it nearly free.' }));
}

function stepSizeSection() {
  const seg = el('div', { class: 'seg' });
  const sub = el('div', { class: 'seg-sub' });
  const render = () => {
    seg.textContent = '';
    for (const s of STEP_SIZES) {
      seg.append(el('button', {
        class: state.settings.step_size === s.id ? 'active' : '',
        text: s.label,
        onclick: async () => {
          await saveSettings({ step_size: s.id });
          render();
        },
      }));
    }
    sub.textContent = STEP_SIZES.find((s) => s.id === state.settings.step_size)?.sub || '';
  };
  render();
  return sectionEl('Step size', seg, sub);
}

function themeSection() {
  const seg = el('div', { class: 'seg' });
  const render = () => {
    seg.textContent = '';
    for (const t of THEMES) {
      seg.append(el('button', {
        class: state.settings.theme === t.id ? 'active' : '',
        text: t.label,
        onclick: async () => {
          await saveSettings({ theme: t.id });
          render();
        },
      }));
    }
  };
  render();
  return sectionEl('Appearance', seg);
}

function usageSection() {
  const cost = state.topics.reduce((a, t) => a + (t.cost_usd || 0), 0);
  const inToks = state.topics.reduce((a, t) => a + (t.input_tokens || 0), 0);
  const outToks = state.topics.reduce((a, t) => a + (t.output_tokens || 0), 0);
  return sectionEl('Usage across all topics',
    el('div', { class: 'usage-grid' },
      el('div', { class: 'usage-cell' }, el('b', { text: fmtCost(cost) }), el('span', { text: 'est. spend' })),
      el('div', { class: 'usage-cell' }, el('b', { text: fmtTokens(inToks) }), el('span', { text: 'input tokens' })),
      el('div', { class: 'usage-cell' }, el('b', { text: fmtTokens(outToks) }), el('span', { text: 'output tokens' }))),
    el('div', { class: 'set-note', text: 'Computed from the token counts the API reports for every call, at list prices.' }));
}

function dataSection() {
  const backupBtn = el('button', {
    class: 'btn btn-ghost btn-sm', text: 'Back up…',
    onclick: async () => {
      try {
        const path = await api.backupDatabase();
        if (path) toast('Backed up to ' + path);
      } catch (e) { toastErr(e); }
    },
  });
  const restoreBtn = el('button', {
    class: 'btn btn-ghost btn-sm', text: 'Restore…',
    onclick: async () => {
      const ok = await confirmModal({
        title: 'Restore from backup?',
        message: 'Current data is replaced by the backup you pick. A safety copy of the current data is kept next to the database first.',
        confirmLabel: 'Restore',
        danger: true,
      });
      if (!ok) return;
      try {
        await api.restoreDatabase();
        // On success the backend emits db:restored and the app reloads.
      } catch (e) { toastErr(e); }
    },
  });
  return sectionEl('Data',
    el('div', { class: 'btn-row' }, backupBtn, restoreBtn),
    el('div', { class: 'set-note', text: 'Everything autosaves to a local SQLite database as you work. Back it up to a single file; restore replaces the library.' }));
}

// ------------------------------------------------------------- shortcuts

const SHORTCUTS = [
  ['Next step (unprompted)', ['Ctrl', 'Enter']],
  ['Steer the next step', ['N']],
  ['Ask about selection', ['A']],
  ['Search in topic', ['Ctrl', 'F']],
  ['Toggle spine / map', ['G']],
  ['Step / node down · up', ['J', 'K']],
  ['Open focused node (map)', ['Enter']],
  ['Re-centre the map', ['0']],
  ['Settings', ['Ctrl', ',']],
  ['Close / back', ['Esc']],
  ['This cheat sheet', ['?']],
];

export function openShortcuts() {
  const grid = el('div', { class: 'sc-grid' });
  for (const [label, keys] of SHORTCUTS) {
    grid.append(el('div', { class: 'sc-row' },
      el('span', { text: label }),
      el('span', { class: 'sc-keys' }, keys.map((k) => el('kbd', { text: k })))));
  }
  openModal({ title: 'Keyboard shortcuts', body: grid });
}
