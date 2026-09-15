// Topic-level chrome: header (title, status, cost, view toggle, actions),
// the concept-ledger drawer, the new-topic modal (with import-to-seed), and
// export.

import { qs, el, icon, fmtCost, fmtTokens, toast, toastErr, showMenu, openModal, promptModal, autoGrow } from '../util.js';
import { api, onEventScoped } from '../api.js';
import { state, findStep, findNote } from '../state.js';
import { selectTopic, refreshTopicList, switchView } from '../nav.js';
import * as spine from '../spine.js';
import * as notes from './notes.js';
import * as search from './search.js';
import * as sidebar from './sidebar.js';

const STATUS_LABELS = { learning: 'Learning', revisit: 'Revisit later', done: 'Done' };

export function initTopics() {
  qs('#btn-new-topic').addEventListener('click', () => openNewTopicModal());
  qs('#btn-empty-new').addEventListener('click', () => openNewTopicModal());

  qs('#topic-title').addEventListener('dblclick', () => renameCurrent());
  qs('#topic-status').addEventListener('click', (e) => {
    if (!state.topic) return;
    const r = e.currentTarget.getBoundingClientRect();
    showMenu(
      ['learning', 'revisit', 'done'].map((status) => ({
        label: STATUS_LABELS[status],
        checked: state.topic.status === status,
        onClick: () => sidebar.setStatus(state.topic.id, status),
      })),
      r.left, r.bottom + 4,
    );
  });

  qs('#btn-view-spine').addEventListener('click', () => switchView('spine'));
  qs('#btn-view-graph').addEventListener('click', () => switchView('graph'));
  qs('#btn-search').addEventListener('click', () => search.openSearch());
  qs('#btn-ledger').addEventListener('click', () => toggleLedger());
  qs('#ledger-close').addEventListener('click', () => closeLedger());
  qs('#ledger-filter').addEventListener('input', () => renderLedger());
  qs('#ledger-filter').addEventListener('keydown', (e) => {
    if (e.key === 'Escape') { e.stopPropagation(); closeLedger(); }
  });
  qs('#btn-export').addEventListener('click', () => exportCurrent());
}

async function renameCurrent() {
  if (!state.topic) return;
  const title = await promptModal({
    title: 'Rename topic', label: 'Title', value: state.topic.title, confirmLabel: 'Rename',
  });
  if (!title || title === state.topic.title) return;
  try {
    await api.renameTopic(state.topic.id, title);
    state.topic.title = title;
    renderHeader();
    await refreshTopicList();
  } catch (e) {
    toastErr(e);
  }
}

// ------------------------------------------------------------- header

export function renderHeader() {
  const t = state.topic;
  if (!t) return;
  qs('#topic-title').textContent = t.title;
  const pill = qs('#topic-status');
  pill.className = 'status-pill ' + t.status;
  pill.textContent = STATUS_LABELS[t.status] || t.status;
  renderCost();
  qs('#ledger-filter').value = '';
  renderLedger();
}

export function renderCost() {
  const t = state.topic;
  if (!t) return;
  const badge = qs('#topic-cost');
  const toks = (t.usage.input_tokens || 0) + (t.usage.output_tokens || 0);
  badge.textContent = toks > 0 ? `${fmtCost(t.usage.cost_usd)} · ${fmtTokens(toks)} tok` : '';
  badge.title = `Estimated API spend for this topic\n`
    + `${fmtTokens(t.usage.input_tokens)} input + ${fmtTokens(t.usage.output_tokens)} output tokens`;
}

// ------------------------------------------------------------- ledger

export function toggleLedger() {
  const drawer = qs('#ledger-drawer');
  const show = drawer.hidden;
  drawer.hidden = !show;
  qs('#btn-ledger').classList.toggle('active', show);
  if (show) {
    qs('#ledger-filter').value = '';
    renderLedger();
    qs('#ledger-filter').focus();
  }
}

export function closeLedger() {
  qs('#ledger-drawer').hidden = true;
  qs('#btn-ledger').classList.remove('active');
}

export function isLedgerOpen() {
  return !qs('#ledger-drawer').hidden;
}

export function renderLedger() {
  const t = state.topic;
  const list = qs('#ledger-list');
  if (!t) return;
  const q = qs('#ledger-filter').value.trim().toLowerCase();
  const shown = q ? t.ledger.filter((e) => e.label.toLowerCase().includes(q)) : t.ledger;

  list.textContent = '';
  const empty = qs('#ledger-empty');
  empty.hidden = shown.length > 0;
  empty.textContent = t.ledger.length === 0
    ? 'Nothing yet. Concepts appear here as steps are generated.'
    : `No concept matches “${q}”.`;

  for (const entry of shown) {
    list.append(el('button', {
      class: 'ledger-chip' + (entry.source_kind === 'side_note' ? ' from-note' : ''),
      title: entry.source_kind === 'side_note' ? 'Learned in a side note (click to open)' : 'Taught in a step (click to jump)',
      onclick: () => jumpToSource(entry),
    },
      el('span', { class: 'lc-dot' }),
      el('span', { text: entry.label })));
  }
}

function jumpToSource(entry) {
  if (entry.source_kind === 'side_note') {
    const note = findNote(entry.source_id);
    if (!note) {
      toast('That side note no longer exists', { error: true });
      return;
    }
    switchView('spine');
    spine.scrollToStep(note.anchor_step_id, { flash: false });
    notes.openNotePanel(note.id);
  } else {
    const step = findStep(entry.source_id);
    if (!step) {
      toast('That step no longer exists', { error: true });
      return;
    }
    switchView('spine');
    spine.scrollToStep(step.id);
  }
}

// ------------------------------------------------------------- export

export async function exportCurrent() {
  if (!state.topic) return;
  try {
    const path = await api.exportTopicMarkdown(state.topic.id);
    if (path) toast('Exported to ' + path);
  } catch (e) {
    toastErr(e);
  }
}

// ------------------------------------------------------------- new topic

// Mirrors MAX_SEED_CHARS in prompts.rs — anything past this is trimmed off
// before the seed reaches the model, so the modal says so up front.
const SEED_BUDGET_CHARS = 200000;
// Mirrors MAX_PRIOR_KNOWLEDGE_CHARS in prompts.rs.
const PRIOR_KNOWLEDGE_MAX_CHARS = 4000;

/** Free-text notes first, then each document under its own filename header. */
function buildSeed(typedNotes, attached) {
  const parts = [];
  const typed = typedNotes.trim();
  if (typed) parts.push(typed);
  for (const f of attached) {
    parts.push(`--- ${f.name} ---\n${f.content.trim()}`);
  }
  return parts.join('\n\n');
}

/**
 * Accepts files dropped anywhere on the window while the dialog is open —
 * the modal is the only interactive surface, and matching the drop position
 * against the zone's rect would mean converting physical to logical pixels.
 * Returns a function that detaches the listeners.
 */
function listenForDrop(zone, onFiles) {
  const pending = [
    onEventScoped('tauri://drag-enter', () => zone.classList.add('dragging')),
    onEventScoped('tauri://drag-leave', () => zone.classList.remove('dragging')),
    onEventScoped('tauri://drag-drop', async (payload) => {
      zone.classList.remove('dragging');
      const paths = payload?.paths ?? [];
      if (paths.length === 0) return;
      try {
        onFiles(await api.readDroppedFiles(paths));
      } catch (e) { toastErr(e); }
    }),
  ];
  let detached = false;
  for (const p of pending) {
    p.then((un) => { if (detached) un(); }).catch(() => {});
  }
  return () => {
    detached = true;
    for (const p of pending) p.then((un) => un()).catch(() => {});
  };
}

export function openNewTopicModal() {
  if (!state.auth.configured) return;

  const titleInput = el('input', {
    type: 'text',
    placeholder: 'What do you want to learn? e.g. “ReSTIR”, “Rust async”…',
    spellcheck: 'false',
  });
  const priorTa = el('textarea', {
    rows: '3',
    maxlength: String(PRIOR_KNOWLEDGE_MAX_CHARS),
    placeholder: 'Optional. e.g. “I already understand path tracing and Monte Carlo integration, but I’ve never worked with resampling.”',
  });
  priorTa.addEventListener('input', () => autoGrow(priorTa));
  const seedTa = el('textarea', {
    rows: '5',
    placeholder: 'Optional. Paste a paper abstract, notes, or docs. The first steps will be grounded in it.',
  });
  seedTa.addEventListener('input', () => autoGrow(seedTa));

  const attached = [];
  const fileList = el('div', { class: 'attach-list' });
  const budgetNote = el('div', { class: 'attach-budget' });

  const seedLength = () => buildSeed(seedTa.value, attached).length;

  const renderAttachments = () => {
    fileList.textContent = '';
    for (const f of attached) {
      fileList.append(el('div', { class: 'attach-item' },
        el('span', { class: 'ai-icon', html: icon('doc') }),
        el('span', { class: 'ai-name', text: f.name, title: f.name }),
        el('span', { class: 'ai-size', text: `${f.content.length.toLocaleString()} chars` }),
        el('button', {
          class: 'icon-btn sm', title: 'Remove',
          onclick: () => {
            attached.splice(attached.indexOf(f), 1);
            renderAttachments();
          },
        }, el('span', { html: icon('x') }))));
    }
    const total = seedLength();
    if (total === 0) {
      budgetNote.textContent = '';
    } else if (total > SEED_BUDGET_CHARS) {
      budgetNote.className = 'attach-budget over';
      budgetNote.textContent =
        `${total.toLocaleString()} chars, but only the first ${SEED_BUDGET_CHARS.toLocaleString()} are sent to the model. Trim it, or attach the most relevant sections.`;
    } else {
      budgetNote.className = 'attach-budget';
      budgetNote.textContent = `${total.toLocaleString()} chars of context`;
    }
  };

  const addFiles = (files) => {
    for (const f of files) {
      if (f.error) {
        toast(`${f.name}: ${f.error}`, { error: true });
        continue;
      }
      if (attached.some((a) => a.name === f.name && a.content === f.content)) continue;
      attached.push(f);
    }
    renderAttachments();
  };

  const importBtn = el('button', {
    class: 'btn btn-ghost btn-sm', text: 'Attach documents…',
    onclick: async () => {
      try {
        addFiles(await api.importDocuments());
      } catch (e) { toastErr(e); }
    },
  });

  seedTa.addEventListener('input', renderAttachments);

  const createBtn = el('button', { class: 'btn btn-primary', text: 'Create topic' });
  const cancelBtn = el('button', { class: 'btn btn-ghost', text: 'Cancel' });

  const dropZone = el('div', { class: 'attach-zone' },
    el('div', { class: 'attach-head' },
      importBtn,
      el('span', { class: 'attach-hint', text: 'or drop files here' })),
    fileList,
    budgetNote);

  const body = el('div', {},
    el('div', { class: 'field' },
      el('label', { text: 'Topic' }),
      titleInput,
      el('div', { class: 'sub', text: 'One topic = one learning path. Keep it focused.' })),
    el('div', { class: 'field' },
      el('label', { text: 'What you already know (optional)' }),
      priorTa,
      el('div', { class: 'sub', text: 'A few sentences on where your understanding is now, so Waypoint starts at your level instead of from scratch.' })),
    el('div', { class: 'field' },
      el('label', { text: 'Seed context (optional)' }),
      seedTa,
      el('div', { class: 'sub', text: 'PDF, Word, Markdown, plain text and source files. The first steps are grounded in whatever you add.' }),
      dropZone));

  let stopDrop = () => {};
  const m = openModal({
    title: 'New topic', body, foot: [cancelBtn, createBtn],
    onClose: () => stopDrop(),
  });
  cancelBtn.onclick = () => m.close();
  stopDrop = listenForDrop(dropZone, addFiles);

  const submit = async () => {
    const title = titleInput.value.trim();
    if (!title) {
      titleInput.focus();
      return;
    }
    createBtn.disabled = true;
    try {
      const summary = await api.createTopic(
        title,
        buildSeed(seedTa.value, attached) || null,
        priorTa.value.trim() || null,
      );
      m.close();
      await refreshTopicList();
      await selectTopic(summary.id);
      spine.expandComposer();
    } catch (e) {
      createBtn.disabled = false;
      toastErr(e);
    }
  };
  createBtn.onclick = submit;
  titleInput.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') submit();
  });
  titleInput.focus();
}
