// In-topic search (§5.5): live matches across step content and side-note
// threads, with inline highlighting and Enter/Shift+Enter navigation.

import { qs, debounce } from '../util.js';
import { state } from '../state.js';
import { plainText, wrapPlainRange } from '../anchors.js';
import * as spine from '../spine.js';
import * as notes from './notes.js';

const MAX_MATCHES = 200;
let open = false;

export function initSearch() {
  const input = qs('#search-input');
  input.addEventListener('input', debounce(() => runSearch(input.value), 160));
  input.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') {
      e.preventDefault();
      step(e.shiftKey ? -1 : 1);
    } else if (e.key === 'Escape') {
      e.preventDefault();
      closeSearch();
    }
  });
  qs('#search-next').addEventListener('click', () => step(1));
  qs('#search-prev').addEventListener('click', () => step(-1));
  qs('#search-close').addEventListener('click', () => closeSearch());
}

export function isOpen() {
  return open;
}

export function openSearch() {
  if (!state.topic) return;
  open = true;
  qs('#search-bar').hidden = false;
  const input = qs('#search-input');
  input.focus();
  input.select();
  if (input.value) runSearch(input.value);
}

export function closeSearch() {
  if (!open) return;
  open = false;
  qs('#search-bar').hidden = true;
  qs('#search-input').value = '';
  clearSearchState();
}

function clearSearchState() {
  if (state.search) {
    state.search = null;
    spine.refreshAllSteps(); // strips search-hit marks, keeps note marks
  }
  updateCount();
}

function runSearch(raw) {
  const q = raw.trim().toLowerCase();
  // Re-render steps first so stale hit marks never survive a query change.
  if (state.search) spine.refreshAllSteps();
  state.search = null;

  if (!q || !state.topic) {
    updateCount();
    return;
  }

  const matches = [];

  // Step content matches (highlighted in place).
  for (const stepItem of state.topic.steps) {
    const stepEl = spine.stepElOf(stepItem.id);
    const contentEl = stepEl?.querySelector('.step-content');
    if (!contentEl) continue;
    const plain = plainText(contentEl);
    const lower = plain.toLowerCase();
    let idx = lower.indexOf(q);
    const stepRanges = [];
    while (idx >= 0 && matches.length + stepRanges.length < MAX_MATCHES) {
      stepRanges.push({ kind: 'step', stepId: stepItem.id, start: idx, end: idx + q.length });
      idx = lower.indexOf(q, idx + q.length);
    }
    // Wrap from the end so earlier offsets stay valid within this pass.
    for (let i = stepRanges.length - 1; i >= 0; i--) {
      const m = stepRanges[i];
      const mi = matches.length + i;
      wrapPlainRange(contentEl, m.start, m.end, 'search-hit', { mi: String(mi) });
    }
    matches.push(...stepRanges);
    if (matches.length >= MAX_MATCHES) break;
  }

  // Side-note matches (navigate-to, not highlighted inline).
  if (matches.length < MAX_MATCHES) {
    for (const note of state.topic.side_notes) {
      const hay = (note.quoted_text + ' ' + note.messages.map((m) => m.content).join(' ')).toLowerCase();
      if (hay.includes(q)) {
        matches.push({ kind: 'note', noteId: note.id });
        if (matches.length >= MAX_MATCHES) break;
      }
    }
  }

  state.search = { query: q, matches, idx: matches.length ? 0 : -1 };
  updateCount();
  if (matches.length) activate(0, { openNote: false });
}

function step(delta) {
  const s = state.search;
  if (!s || s.matches.length === 0) return;
  s.idx = (s.idx + delta + s.matches.length) % s.matches.length;
  activate(s.idx, { openNote: true });
  updateCount();
}

function activate(i, { openNote }) {
  const s = state.search;
  if (!s) return;
  s.idx = i;
  document.querySelectorAll('.search-hit.current').forEach((m) => m.classList.remove('current'));
  const m = s.matches[i];
  if (!m) return;

  if (m.kind === 'step') {
    const marks = document.querySelectorAll(`.search-hit[data-mi="${i}"]`);
    marks.forEach((mk) => mk.classList.add('current'));
    if (marks[0]) marks[0].scrollIntoView({ block: 'center', behavior: 'smooth' });
  } else if (openNote) {
    const note = state.topic.side_notes.find((n) => n.id === m.noteId);
    if (note) {
      spine.scrollToStep(note.anchor_step_id, { flash: false });
      notes.openNotePanel(note.id);
    }
  }
  updateCount();
}

function updateCount() {
  const s = state.search;
  const label = qs('#search-count');
  if (!s || !s.query) {
    label.textContent = '';
  } else if (s.matches.length === 0) {
    label.textContent = 'No matches';
  } else {
    label.textContent = `${s.idx + 1} of ${s.matches.length}`;
  }
}
