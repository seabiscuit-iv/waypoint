// Topic library sidebar (§5.6): select, rename, duplicate, reorder (drag),
// delete, status, export — plus the cross-topic spend total.

import { qs, el, icon, fmtCost, fmtTokens, toast, toastErr, showMenu, confirmModal, promptModal } from '../util.js';
import { api } from '../api.js';
import { state } from '../state.js';
import { selectTopic, refreshTopicList } from '../nav.js';
import * as topicsView from './topics.js';

let draggedId = null;

const STATUS_LABELS = { learning: 'Learning', revisit: 'Revisit later', done: 'Done' };

export function renderTopics() {
  const list = qs('#topic-list');
  list.textContent = '';
  for (const t of state.topics) list.append(topicItem(t));
  qs('#sidebar-empty').hidden = state.topics.length > 0;
  renderUsageTotal();
}

function renderUsageTotal() {
  const cost = state.topics.reduce((a, t) => a + (t.cost_usd || 0), 0);
  const toks = state.topics.reduce((a, t) => a + (t.input_tokens || 0) + (t.output_tokens || 0), 0);
  qs('#usage-total').textContent = state.topics.length
    ? `${fmtCost(cost)} · ${fmtTokens(toks)} tok`
    : '';
}

function topicItem(t) {
  const menuBtn = el('button', {
    class: 'icon-btn sm ti-menu', html: icon('dots'), title: 'Topic options',
    onclick: (e) => {
      e.stopPropagation();
      const r = e.currentTarget.getBoundingClientRect();
      showTopicMenu(t, r.left, r.bottom + 4);
    },
  });

  const meta = [`${t.step_count} step${t.step_count === 1 ? '' : 's'}`];
  if (t.open_note_count > 0) meta.push(`${t.open_note_count} open note${t.open_note_count === 1 ? '' : 's'}`);
  if (t.cost_usd > 0) meta.push(fmtCost(t.cost_usd));

  const item = el('div', {
    class: 'topic-item' + (t.id === state.currentTopicId ? ' active' : ''),
    dataset: { topicId: t.id },
    draggable: 'true',
    role: 'button',
    tabindex: '0',
    onclick: () => selectTopic(t.id).catch(toastErr),
    oncontextmenu: (e) => {
      e.preventDefault();
      showTopicMenu(t, e.clientX, e.clientY);
    },
  },
    el('div', { class: 'ti-row' },
      el('span', { class: 'ti-status ' + t.status, title: STATUS_LABELS[t.status] || t.status }),
      el('span', { class: 'ti-title', text: t.title }),
      menuBtn),
    el('div', { class: 'ti-meta', text: meta.join(' · ') }));

  // Drag to reorder.
  item.addEventListener('dragstart', (e) => {
    draggedId = t.id;
    item.classList.add('dragging');
    e.dataTransfer.effectAllowed = 'move';
    e.dataTransfer.setData('text/plain', t.id);
  });
  item.addEventListener('dragend', () => {
    draggedId = null;
    document.querySelectorAll('.topic-item').forEach((n) =>
      n.classList.remove('dragging', 'drop-above', 'drop-below'));
  });
  item.addEventListener('dragover', (e) => {
    if (!draggedId || draggedId === t.id) return;
    e.preventDefault();
    e.dataTransfer.dropEffect = 'move';
    const rect = item.getBoundingClientRect();
    const above = e.clientY < rect.top + rect.height / 2;
    item.classList.toggle('drop-above', above);
    item.classList.toggle('drop-below', !above);
  });
  item.addEventListener('dragleave', () => {
    item.classList.remove('drop-above', 'drop-below');
  });
  item.addEventListener('drop', (e) => {
    e.preventDefault();
    if (!draggedId || draggedId === t.id) return;
    const rect = item.getBoundingClientRect();
    const above = e.clientY < rect.top + rect.height / 2;
    applyReorder(draggedId, t.id, above);
  });

  return item;
}

function applyReorder(movedId, targetId, before) {
  const ids = state.topics.map((t) => t.id).filter((id) => id !== movedId);
  let idx = ids.indexOf(targetId);
  if (idx < 0) return;
  if (!before) idx += 1;
  ids.splice(idx, 0, movedId);
  const byId = new Map(state.topics.map((t) => [t.id, t]));
  state.topics = ids.map((id) => byId.get(id));
  renderTopics();
  api.reorderTopics(ids).catch(toastErr);
}

export function showTopicMenu(t, x, y) {
  showMenu([
    {
      label: 'Rename', icon: 'pencil',
      onClick: async () => {
        const title = await promptModal({
          title: 'Rename topic', label: 'Title', value: t.title, confirmLabel: 'Rename',
        });
        if (!title || title === t.title) return;
        try {
          await api.renameTopic(t.id, title);
          if (state.topic?.id === t.id) {
            state.topic.title = title;
            topicsView.renderHeader();
          }
          await refreshTopicList();
        } catch (e) { toastErr(e); }
      },
    },
    {
      label: 'Duplicate', icon: 'copy',
      onClick: async () => {
        try {
          await api.duplicateTopic(t.id);
          await refreshTopicList();
          toast('Topic duplicated');
        } catch (e) { toastErr(e); }
      },
    },
    {
      label: 'Export as Markdown…', icon: 'export',
      onClick: async () => {
        try {
          const path = await api.exportTopicMarkdown(t.id);
          if (path) toast('Exported to ' + path);
        } catch (e) { toastErr(e); }
      },
    },
    'sep',
    { header: 'Status' },
    ...['learning', 'revisit', 'done'].map((status) => ({
      label: STATUS_LABELS[status],
      checked: t.status === status,
      onClick: () => setStatus(t.id, status),
    })),
    'sep',
    {
      label: 'Delete topic', icon: 'trash', danger: true,
      onClick: async () => {
        const ok = await confirmModal({
          title: 'Delete topic?',
          message: `“${t.title}” — every step, side note, and its concept ledger will be permanently removed.`,
          confirmLabel: 'Delete',
        });
        if (!ok) return;
        try {
          await api.deleteTopic(t.id);
          if (state.currentTopicId === t.id) await selectTopic(null, { force: true });
          await refreshTopicList();
        } catch (e) { toastErr(e); }
      },
    },
  ], x, y);
}

export async function setStatus(topicId, status) {
  try {
    await api.setTopicStatus(topicId, status);
    if (state.topic?.id === topicId) {
      state.topic.status = status;
      topicsView.renderHeader();
    }
    await refreshTopicList();
  } catch (e) {
    toastErr(e);
  }
}
