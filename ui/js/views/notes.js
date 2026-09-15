// Side notes: highlight → popover → anchored margin thread (§4.3).
// Anchors live as plain-text offsets + a quoted-text snapshot; marks are
// re-anchored by search when offsets drift, and fall back to "detached"
// chips under the step when the quoted text disappears entirely.

import { qs, el, icon, toast, toastErr, autoGrow, truncate, confirmModal, patchStreamHtml } from '../util.js';
import { api } from '../api.js';
import { state, findNote, findStep, notesForStep, drainEarlyEvents } from '../state.js';
import { selectionOffsets, plainText, wrapPlainRange, clonePlainRange } from '../anchors.js';
import * as spine from '../spine.js';
import * as topicsView from './topics.js';
import { refreshTopicList } from '../nav.js';

// panelCtx: null | { mode: 'create', stepId, start, end, text }
//                | { mode: 'view', noteId }
let panelCtx = null;
let popCtx = null; // { stepId, start, end, text }

export function initNotes() {
  const pop = qs('#selection-popover');
  qs('#btn-ask-selection').addEventListener('click', () => confirmSelectionAsk());

  document.addEventListener('mouseup', (e) => {
    if (pop.contains(e.target)) return;
    // Let the selection settle before measuring it.
    setTimeout(maybeShowPopover, 10);
  });
  document.addEventListener('selectionchange', () => {
    const sel = window.getSelection();
    if (!sel || sel.isCollapsed) hidePopover();
  });
  qs('#spine-scroll').addEventListener('scroll', hidePopover, { passive: true });

  // Clicking highlight marks / detached chips opens the note.
  qs('#steps').addEventListener('click', (e) => {
    const mark = e.target.closest?.('.wp-mark');
    if (mark?.dataset.noteId) {
      openNotePanel(mark.dataset.noteId);
      return;
    }
    const chip = e.target.closest?.('.detached-chip');
    if (chip?.dataset.noteId) openNotePanel(chip.dataset.noteId);
  });

  // Panel chrome.
  qs('#np-close').addEventListener('click', () => closePanel());
  qs('#np-send').addEventListener('click', () => sendFromPanel());
  const input = qs('#np-input');
  input.addEventListener('input', () => autoGrow(input));
  input.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      sendFromPanel();
    }
  });
  qs('#np-resolve').addEventListener('click', () => toggleResolved());
  qs('#np-delete').addEventListener('click', () => deleteCurrentNote());
}

// ------------------------------------------------------------- popover

function maybeShowPopover() {
  if (!state.topic || state.view !== 'spine') return;
  const sel = window.getSelection();
  if (!sel || sel.rangeCount === 0 || sel.isCollapsed) {
    hidePopover();
    return;
  }
  const range = sel.getRangeAt(0);
  const anchorNode = range.commonAncestorContainer;
  const anchorEl = anchorNode.nodeType === 1 ? anchorNode : anchorNode.parentElement;
  const contentEl = anchorEl?.closest?.('.step-content');
  if (!contentEl) {
    hidePopover();
    return;
  }
  const stepEl = contentEl.closest('.step');
  if (!stepEl || stepEl.classList.contains('streaming') || stepEl.classList.contains('editing') || stepEl.id === 'pending-step') {
    hidePopover();
    return;
  }
  const offs = selectionOffsets(contentEl);
  if (!offs || offs.text.length > 600) {
    hidePopover();
    return;
  }
  popCtx = { stepId: stepEl.dataset.stepId, ...offs };
  const rect = range.getBoundingClientRect();
  const pop = qs('#selection-popover');
  pop.hidden = false;
  const pw = pop.offsetWidth || 160;
  const ph = pop.offsetHeight || 36;
  let x = rect.left + rect.width / 2 - pw / 2;
  x = Math.max(8, Math.min(x, window.innerWidth - pw - 8));
  let y = rect.top - ph - 10;
  if (y < 70) y = rect.bottom + 10;
  pop.style.left = `${x}px`;
  pop.style.top = `${y}px`;
}

export function hidePopover() {
  qs('#selection-popover').hidden = true;
  popCtx = null;
}

export function hasSelectionContext() {
  return !!popCtx;
}

/** "Ask about this" — from the popover button or the `A` shortcut. */
export function confirmSelectionAsk() {
  if (!popCtx) return;
  const ctx = popCtx;
  hidePopover();
  window.getSelection()?.removeAllRanges();
  openCreatePanel(ctx);
}

// ------------------------------------------------------------- marks

function pendingNoteId() {
  return state.noteGen?.noteId ?? null;
}

/**
 * Applies highlight marks + detached chips for a step's notes. Called by
 * spine.js whenever a step's content is (re)rendered.
 */
export function applyMarks(stepEl, step) {
  const contentEl = stepEl.querySelector('.step-content');
  const detachedWrap = stepEl.querySelector('.detached-notes');
  if (!contentEl || !detachedWrap) return;
  detachedWrap.textContent = '';

  const stepNotes = notesForStep(step.id);
  if (stepNotes.length === 0) return;

  const plain = plainText(contentEl);
  const openId = panelCtx?.mode === 'view' ? panelCtx.noteId : null;

  for (const note of stepNotes) {
    let s = note.start_offset;
    let e = note.end_offset;
    const intact = plain.slice(s, e) === note.quoted_text;
    if (!intact) {
      // Offsets drifted (content edited/regenerated): fall back to the
      // quoted-text snapshot (§7 anchoring resilience).
      const idx = plain.indexOf(note.quoted_text);
      if (idx >= 0) {
        s = idx;
        e = idx + note.quoted_text.length;
        note.start_offset = s;
        note.end_offset = e;
        api.updateSideNoteAnchor(note.id, s, e).catch(() => {});
      } else {
        detachedWrap.append(el('button', {
          class: 'detached-chip',
          dataset: { noteId: note.id },
          title: 'The highlighted text no longer appears in this step, but the note is still here.',
        },
          el('span', { html: icon('chat') }),
          el('span', { text: '“' + truncate(note.quoted_text, 42) + '”' })));
        continue;
      }
    }
    let cls = 'wp-mark';
    if (note.resolved) cls += ' resolved';
    if (pendingNoteId() === note.id) cls += ' pending';
    if (openId === note.id) cls += ' open-note';
    wrapPlainRange(contentEl, s, e, cls, { noteId: note.id });
  }
}

// ------------------------------------------------------------- panel

export function isPanelOpen() {
  return panelCtx !== null;
}

export function openCreatePanel(ctx) {
  panelCtx = { mode: 'create', ...ctx };
  renderPanel();
  qs('#np-input').focus();
}

export function openNotePanel(noteId) {
  const note = findNote(noteId);
  if (!note) return;
  const wasOpen = panelCtx?.mode === 'view' ? panelCtx.noteId : null;
  panelCtx = { mode: 'view', noteId };
  renderPanel();
  if (wasOpen && wasOpen !== noteId) {
    // Un-highlight the previously open note's mark (possibly on another step).
    const prev = findNote(wasOpen);
    if (prev && prev.anchor_step_id !== note.anchor_step_id) {
      spine.refreshStep(prev.anchor_step_id);
    }
  }
  if (wasOpen !== noteId) {
    spine.refreshStep(note.anchor_step_id);
  }
}

export function closePanel() {
  if (!panelCtx) return;
  const prev = panelCtx;
  panelCtx = null;
  qs('#note-panel').hidden = true;
  if (prev.mode === 'view') {
    const note = findNote(prev.noteId);
    if (note) spine.refreshStep(note.anchor_step_id);
  }
}

function renderQuote(quoteEl, stepId, start, end, text) {
  quoteEl.textContent = '';
  const step = findStep(stepId);
  if (step) {
    const root = document.createElement('div');
    root.innerHTML = step.html;
    const plain = plainText(root);
    if (plain.slice(start, end) !== text) {
      start = plain.indexOf(text);
      end = start + text.length;
    }
    if (start >= 0) {
      const frag = clonePlainRange(root, start, end);
      if (frag) {
        quoteEl.append(frag);
        return;
      }
    }
  }
  quoteEl.textContent = text;
}

function renderPanel() {
  const panel = qs('#note-panel');
  if (!panelCtx) {
    panel.hidden = true;
    return;
  }
  panel.hidden = false;

  const quoteEl = qs('#np-quote');
  const thread = qs('#np-thread');
  const resolveBtn = qs('#np-resolve');
  const deleteBtn = qs('#np-delete');
  const composer = qs('#np-composer');
  const input = qs('#np-input');
  thread.textContent = '';

  if (panelCtx.mode === 'create') {
    renderQuote(quoteEl, panelCtx.stepId, panelCtx.start, panelCtx.end, panelCtx.text);
    resolveBtn.hidden = true;
    deleteBtn.hidden = true;
    composer.hidden = false;
    input.placeholder = 'What do you want to ask about this?';
    qs('#np-send').textContent = 'Ask';
    thread.append(el('div', { class: 'np-resolved-banner' },
      'A side note stays in the margin and never interrupts the main path.'));
    return;
  }

  const note = findNote(panelCtx.noteId);
  if (!note) {
    closePanel();
    return;
  }

  renderQuote(quoteEl, note.anchor_step_id, note.start_offset, note.end_offset, note.quoted_text);
  resolveBtn.hidden = false;
  resolveBtn.textContent = note.resolved ? 'Reopen' : 'Resolve';
  deleteBtn.hidden = false;

  for (const msg of note.messages) {
    const bubble = el('div', {
      class: 'np-msg ' + (msg.role === 'user' ? 'user' : 'assistant'),
      html: msg.html,
    });
    if (state.reviewingMessages.has(msg.id)) {
      bubble.querySelectorAll('.diagram').forEach((d) => d.replaceWith(el('div', { class: 'diagram-pending' })));
    }
    thread.append(bubble);
  }

  const generating = state.noteGen?.noteId === note.id;
  if (generating) {
    thread.append(el('div', { class: 'np-msg assistant', id: 'np-streaming' },
      el('div', { class: 'thinking-row' },
        el('span', { class: 'thinking-dots' }, el('i'), el('i'), el('i')),
        el('span', { text: 'Thinking…' }))));
  }

  const err = state.noteErrors.get(note.id);
  if (err && !generating) {
    thread.append(el('div', { class: 'np-error' },
      el('b', { text: 'Couldn’t answer' }),
      el('span', { text: err }),
      el('div', { class: 'ge-actions' },
        el('button', { class: 'btn btn-ghost btn-sm', text: 'Retry', onclick: () => retryNote(note.id) }),
        el('button', {
          class: 'btn btn-ghost btn-sm', text: 'Dismiss',
          onclick: () => { state.noteErrors.delete(note.id); renderPanel(); },
        }))));
  }

  if (note.resolved) {
    thread.append(el('div', { class: 'np-resolved-banner', text: 'Resolved and collapsed to a dot in the text.' }));
  }

  composer.hidden = note.resolved || generating;
  input.placeholder = 'Follow up…';
  qs('#np-send').textContent = 'Send';
  thread.scrollTop = thread.scrollHeight;
}

// ------------------------------------------------------------- actions

async function sendFromPanel() {
  const input = qs('#np-input');
  const text = input.value.trim();
  if (!text || !state.topic) return;
  if (state.noteGen) {
    toast('A side note is already being answered.', { error: true });
    return;
  }
  if (!state.online) {
    toast('You’re offline and can’t generate right now.', { error: true });
    return;
  }

  try {
    if (panelCtx?.mode === 'create') {
      const ctx = panelCtx;
      const res = await api.createSideNote({
        topicId: state.topic.id,
        stepId: ctx.stepId,
        startOffset: ctx.start,
        endOffset: ctx.end,
        quotedText: ctx.text,
        question: text,
      });
      state.topic.side_notes.push(res.note);
      state.noteGen = { genId: res.gen_id, noteId: res.note.id };
      panelCtx = { mode: 'view', noteId: res.note.id };
      input.value = '';
      autoGrow(input);
      renderPanel();
      spine.refreshStep(ctx.stepId);
      refreshTopicList();
      drainEarlyEvents(res.gen_id, handleGenEvent);
    } else if (panelCtx?.mode === 'view') {
      const noteId = panelCtx.noteId;
      state.noteErrors.delete(noteId);
      const res = await api.replySideNote(noteId, text);
      replaceNoteInState(res.note);
      state.noteGen = { genId: res.gen_id, noteId };
      input.value = '';
      autoGrow(input);
      renderPanel();
      drainEarlyEvents(res.gen_id, handleGenEvent);
    }
  } catch (e) {
    toastErr(e);
  }
}

async function retryNote(noteId) {
  if (state.noteGen) return;
  try {
    state.noteErrors.delete(noteId);
    const res = await api.retrySideNote(noteId);
    replaceNoteInState(res.note);
    state.noteGen = { genId: res.gen_id, noteId };
    renderPanel();
    drainEarlyEvents(res.gen_id, handleGenEvent);
  } catch (e) {
    toastErr(e);
  }
}

async function toggleResolved() {
  if (panelCtx?.mode !== 'view') return;
  const note = findNote(panelCtx.noteId);
  if (!note) return;
  try {
    const updated = await api.setSideNoteResolved(note.id, !note.resolved);
    replaceNoteInState(updated);
    renderPanel();
    spine.refreshStep(updated.anchor_step_id);
    refreshTopicList();
    if (updated.resolved) toast('Side note resolved');
  } catch (e) {
    toastErr(e);
  }
}

async function deleteCurrentNote() {
  if (panelCtx?.mode !== 'view') return;
  const note = findNote(panelCtx.noteId);
  if (!note) return;
  const ok = await confirmModal({
    title: 'Delete side note?',
    message: 'The question and its answers will be removed. Concepts it already added to the ledger stay learned.',
    confirmLabel: 'Delete',
  });
  if (!ok) return;
  try {
    await api.deleteSideNote(note.id);
    state.topic.side_notes = state.topic.side_notes.filter((n) => n.id !== note.id);
    // The backend drops this note's ledger rows too — mirror that locally so
    // the drawer doesn't keep showing concepts with no source.
    state.topic.ledger = state.topic.ledger.filter((c) => c.source_id !== note.id);
    window.dispatchEvent(new CustomEvent('wp:ledger-changed'));
    closePanel();
    spine.refreshStep(note.anchor_step_id);
    refreshTopicList();
  } catch (e) {
    toastErr(e);
  }
}

function replaceNoteInState(note) {
  if (!state.topic) return;
  const i = state.topic.side_notes.findIndex((n) => n.id === note.id);
  if (i >= 0) state.topic.side_notes[i] = note;
  else state.topic.side_notes.push(note);
}

// ------------------------------------------------------------- events

// One DOM write per frame, same as the spine stream.
let queuedNoteHtml = null;
let noteFlushHandle = 0;

function queueNoteDelta(html) {
  queuedNoteHtml = html;
  if (noteFlushHandle) return;
  noteFlushHandle = requestAnimationFrame(() => {
    noteFlushHandle = 0;
    const pending = queuedNoteHtml;
    queuedNoteHtml = null;
    const bubble = qs('#np-streaming');
    if (pending === null || !bubble) return;
    bubble.querySelector('.stream-caret')?.remove();
    const last = patchStreamHtml(bubble, pending);
    const caret = el('span', { class: 'stream-caret' });
    if (last && /^(P|LI|H[1-6]|BLOCKQUOTE)$/.test(last.tagName)) last.append(caret);
    else bubble.append(caret);
    const thread = qs('#np-thread');
    thread.scrollTop = thread.scrollHeight;
  });
}

function resetNoteStreamBuffer() {
  if (noteFlushHandle) cancelAnimationFrame(noteFlushHandle);
  noteFlushHandle = 0;
  queuedNoteHtml = null;
}

/** A side-note answer's diagram review finished (revised or not). */
export function onMessageUpdated(payload) {
  state.reviewingMessages.delete(payload.message.id);
  if (!state.topic || state.topic.id !== payload.topic_id) return;
  const note = findNote(payload.note_id);
  const i = note ? note.messages.findIndex((m) => m.id === payload.message.id) : -1;
  if (i < 0) return;
  note.messages[i] = payload.message;
  if (panelCtx?.mode === 'view' && panelCtx.noteId === note.id) renderPanel();
}

export function handleGenEvent(name, payload) {
  const g = state.noteGen;
  if (!g || payload.gen_id !== g.genId) return false;

  switch (name) {
    case 'gen:start':
      return true;
    case 'gen:delta':
      queueNoteDelta(payload.html);
      return true;
    case 'note:done': {
      state.noteGen = null;
      resetNoteStreamBuffer();
      if (payload.reviewing) state.reviewingMessages.add(payload.message.id);
      const note = findNote(payload.note_id);
      if (note && state.topic && state.topic.id === payload.topic_id) {
        note.messages.push(payload.message);
        state.topic.usage = payload.usage;
        if (panelCtx?.mode === 'view' && panelCtx.noteId === note.id) renderPanel();
        spine.refreshStep(note.anchor_step_id);
        topicsView.renderCost();
      }
      refreshTopicList();
      return true;
    }
    case 'gen:error': {
      state.noteGen = null;
      resetNoteStreamBuffer();
      if (payload.kind !== 'cancelled') {
        state.noteErrors.set(payload.note_id, payload.message);
      }
      const note = findNote(payload.note_id);
      if (note) {
        if (panelCtx?.mode === 'view' && panelCtx.noteId === note.id) renderPanel();
        spine.refreshStep(note.anchor_step_id);
      }
      return true;
    }
    default:
      return false;
  }
}
