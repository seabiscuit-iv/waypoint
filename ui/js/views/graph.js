// Map view (§5.2): the spine as a vertical chain of nodes, side notes as
// amber dots off to the side. Pan/zoom via a single CSS transform (smooth),
// hover previews, and a maximize overlay that never destroys the graph —
// so returning keeps scroll/zoom exactly where it was.

import { qs, el, icon, escapeHtml, truncate } from '../util.js';
import { state, findStep, findNote, notesForStep } from '../state.js';
import * as spine from '../spine.js';
import * as notes from './notes.js';
import { switchView } from '../nav.js';

const STEP_W = 230;
const STEP_GAP_Y = 132;
const STEP_X = 60;
const NOTE_X = STEP_X + STEP_W + 78;
const NOTE_GAP_Y = 30;

// Per-topic camera, preserved across view switches and maximize.
const cameras = new Map();
let cam = { x: 0, y: 0, k: 1 };
let builtForTopic = null;
let maxCtx = null; // { kind: 'step'|'note', id }
let focusId = null; // node id under keyboard focus (J/K)

export function initGraph() {
  const canvas = qs('#graph-canvas');

  // Pan.
  let panning = null;
  canvas.addEventListener('pointerdown', (e) => {
    if (e.target.closest('.gnode')) return;
    panning = { px: e.clientX, py: e.clientY, ox: cam.x, oy: cam.y };
    canvas.classList.add('panning');
    canvas.setPointerCapture(e.pointerId);
  });
  canvas.addEventListener('pointermove', (e) => {
    if (!panning) return;
    cam.x = panning.ox + (e.clientX - panning.px);
    cam.y = panning.oy + (e.clientY - panning.py);
    applyCam();
  });
  const endPan = () => {
    panning = null;
    canvas.classList.remove('panning');
  };
  canvas.addEventListener('pointerup', endPan);
  canvas.addEventListener('pointercancel', endPan);

  // Zoom (anchored at cursor).
  canvas.addEventListener('wheel', (e) => {
    e.preventDefault();
    const rect = canvas.getBoundingClientRect();
    const cx = e.clientX - rect.left;
    const cy = e.clientY - rect.top;
    const k2 = Math.min(2.5, Math.max(0.25, cam.k * Math.exp(-e.deltaY * 0.0012)));
    cam.x = cx - ((cx - cam.x) * k2) / cam.k;
    cam.y = cy - ((cy - cam.y) * k2) / cam.k;
    cam.k = k2;
    applyCam();
    hidePreview();
  }, { passive: false });

  // Node interactions (delegated).
  const nodesEl = qs('#graph-nodes');
  nodesEl.addEventListener('click', (e) => {
    const node = e.target.closest('.gnode');
    if (!node) return;
    // Keep J/K walking from wherever the mouse last landed.
    focusId = node.dataset.id;
    paintFocus();
    maximize(node.dataset.kind, node.dataset.id);
  });
  nodesEl.addEventListener('pointerover', (e) => {
    const node = e.target.closest('.gnode');
    if (node) showPreview(node);
  });
  nodesEl.addEventListener('pointerout', (e) => {
    if (e.target.closest('.gnode')) hidePreview();
  });

  // Maximize overlay chrome.
  qs('#nm-close').addEventListener('click', closeMax);
  qs('#nm-goto').addEventListener('click', () => {
    if (!maxCtx) return;
    const ctx = maxCtx;
    closeMax();
    switchView('spine');
    if (ctx.kind === 'step') {
      spine.scrollToStep(ctx.id);
    } else {
      const note = findNote(ctx.id);
      if (note) {
        spine.scrollToStep(note.anchor_step_id, { flash: false });
        notes.openNotePanel(note.id);
      }
    }
  });
  qs('#node-max').addEventListener('pointerdown', (e) => {
    if (e.target === qs('#node-max')) closeMax();
  });
}

function applyCam() {
  qs('#graph-world').style.transform = `translate(${cam.x}px, ${cam.y}px) scale(${cam.k})`;
}

export function isMaximized() {
  return !qs('#node-max').hidden;
}

export function closeMax() {
  qs('#node-max').hidden = true;
  maxCtx = null;
}

// ------------------------------------------------------- keyboard focus

/** Every node in visual top-to-bottom order: each step, then its own notes. */
function nodeOrder() {
  const t = state.topic;
  if (!t) return [];
  const out = [];
  for (const step of t.steps) {
    out.push({ kind: 'step', id: step.id });
    for (const note of notesForStep(step.id)) out.push({ kind: 'note', id: note.id });
  }
  return out;
}

function paintFocus() {
  const nodesEl = qs('#graph-nodes');
  for (const n of nodesEl.querySelectorAll('.gnode.focused')) n.classList.remove('focused');
  if (!focusId) return null;
  const node = nodesEl.querySelector(`.gnode[data-id="${CSS.escape(focusId)}"]`);
  if (!node) {
    focusId = null;
    return null;
  }
  node.classList.add('focused');
  return node;
}

function centerOn(node) {
  const canvas = qs('#graph-canvas');
  const x = parseFloat(node.style.left) || 0;
  const y = parseFloat(node.style.top) || 0;
  cam.x = canvas.clientWidth / 2 - (x + node.offsetWidth / 2) * cam.k;
  cam.y = canvas.clientHeight / 2 - (y + node.offsetHeight / 2) * cam.k;
  applyCam();
}

/** J/K in map view: walk nodes, centre the camera, preview as you go. */
export function focusNode(delta) {
  const order = nodeOrder();
  if (order.length === 0) return;
  const at = order.findIndex((n) => n.id === focusId);
  const next = at < 0
    ? (delta > 0 ? 0 : order.length - 1)
    : Math.max(0, Math.min(order.length - 1, at + delta));
  focusId = order[next].id;
  const node = paintFocus();
  if (!node) return;
  centerOn(node);
  // After applyCam, so the preview measures the node's settled position.
  showPreview(node);
}

/** Enter in map view opens the focused node's reading view. */
export function maximizeFocused() {
  if (!focusId) {
    focusNode(1);
    return;
  }
  const entry = nodeOrder().find((n) => n.id === focusId);
  if (entry) maximize(entry.kind, entry.id);
}

export function resetCamera() {
  if (!state.topic) return;
  const canvas = qs('#graph-canvas');
  const stepCount = state.topic.steps.length;
  const contentW = NOTE_X + 120;
  const contentH = Math.max(1, stepCount) * STEP_GAP_Y + 120;
  const kFit = Math.min(1, (canvas.clientHeight - 60) / contentH, (canvas.clientWidth - 60) / contentW);
  cam.k = Math.max(0.25, Math.min(1, kFit));
  cam.x = Math.max(24, (canvas.clientWidth - contentW * cam.k) / 2);
  cam.y = 28;
  applyCam();
}

/** (Re)builds the graph for the current topic. Camera survives rebuilds. */
export function buildGraph() {
  const t = state.topic;
  if (!t) return;

  const nodesEl = qs('#graph-nodes');
  const edgesEl = qs('#graph-edges');
  nodesEl.textContent = '';
  edgesEl.textContent = '';
  hidePreview();

  const stepPos = new Map();
  const notePos = new Map();

  t.steps.forEach((step, i) => {
    const y = 40 + i * STEP_GAP_Y;
    stepPos.set(step.id, { x: STEP_X, y });
    const stepNotes = notesForStep(step.id);
    stepNotes.forEach((note, j) => {
      const ny = y + 6 + j * NOTE_GAP_Y;
      notePos.set(note.id, { x: NOTE_X, y: ny });
    });
  });

  // Edges (SVG sized to content bounds).
  const width = NOTE_X + 200;
  const height = 40 + Math.max(1, t.steps.length) * STEP_GAP_Y + 200;
  edgesEl.setAttribute('width', width);
  edgesEl.setAttribute('height', height);
  edgesEl.setAttribute('viewBox', `0 0 ${width} ${height}`);

  const cxSpine = STEP_X + STEP_W / 2;
  for (let i = 0; i + 1 < t.steps.length; i++) {
    const a = stepPos.get(t.steps[i].id);
    const b = stepPos.get(t.steps[i + 1].id);
    const line = document.createElementNS('http://www.w3.org/2000/svg', 'line');
    line.setAttribute('x1', cxSpine);
    line.setAttribute('y1', a.y + 70);
    line.setAttribute('x2', cxSpine);
    line.setAttribute('y2', b.y + 4);
    line.setAttribute('class', 'edge-spine');
    edgesEl.append(line);
  }

  for (const [noteId, np] of notePos) {
    const note = findNote(noteId);
    if (!note) continue;
    const sp = stepPos.get(note.anchor_step_id);
    if (!sp) continue;
    const x1 = STEP_X + STEP_W;
    const y1 = sp.y + 34;
    const x2 = np.x;
    const y2 = np.y + 9;
    const midX = (x1 + x2) / 2;
    const path = document.createElementNS('http://www.w3.org/2000/svg', 'path');
    path.setAttribute('d', `M ${x1} ${y1} C ${midX} ${y1}, ${midX} ${y2}, ${x2} ${y2}`);
    path.setAttribute('class', 'edge-note');
    edgesEl.append(path);
  }

  // Nodes.
  t.steps.forEach((step, i) => {
    const p = stepPos.get(step.id);
    const noteCount = notesForStep(step.id).length;
    const node = el('div', {
      class: 'gnode gnode-step',
      dataset: { kind: 'step', id: step.id },
      style: `left:${p.x}px; top:${p.y}px;`,
    },
      el('div', { class: 'gn-no', text: `STEP ${i + 1}` }),
      el('div', { class: 'gn-snippet', text: stepSnippet(step) }),
      noteCount > 0
        ? el('div', { class: 'gn-notes', text: `${noteCount} side note${noteCount > 1 ? 's' : ''}` })
        : null);
    nodesEl.append(node);
  });

  for (const [noteId, p] of notePos) {
    const note = findNote(noteId);
    if (!note) continue;
    nodesEl.append(el('div', {
      class: 'gnode gnode-note' + (note.resolved ? ' resolved' : ''),
      dataset: { kind: 'note', id: noteId },
      style: `left:${p.x}px; top:${p.y}px;`,
      title: note.quoted_text,
    }));
  }

  if (builtForTopic !== t.id) {
    focusId = null;
    const saved = cameras.get(t.id);
    if (saved) {
      cam = saved;
      applyCam();
    } else {
      cam = { x: 0, y: 0, k: 1 };
      cameras.set(t.id, cam);
      resetCamera();
    }
    builtForTopic = t.id;
  } else {
    applyCam();
  }

  // Nodes were just recreated — restore the focus ring (drops it if the
  // focused node is gone, e.g. a deleted side note).
  paintFocus();
}

function stepSnippet(step) {
  const tmp = document.createElement('div');
  tmp.innerHTML = step.html;
  tmp.querySelectorAll('.diagram').forEach((d) => d.remove());
  return truncate(tmp.textContent.trim().replace(/\s+/g, ' '), 110);
}

// ------------------------------------------------------------- preview

function showPreview(node) {
  const prev = qs('#graph-preview');
  const kind = node.dataset.kind;
  let kicker = '';
  let bodyHtml = '';

  if (kind === 'step') {
    const step = findStep(node.dataset.id);
    if (!step) return;
    const idx = state.topic.steps.findIndex((s) => s.id === step.id);
    kicker = `Step ${idx + 1}`;
    bodyHtml = step.html;
    prev.classList.remove('note');
  } else {
    const note = findNote(node.dataset.id);
    if (!note) return;
    kicker = note.resolved ? 'Side note · resolved' : 'Side note';
    const q = note.messages.find((m) => m.role === 'user');
    const a = note.messages.find((m) => m.role === 'assistant');
    bodyHtml =
      `<p><em>“${escapeHtml(truncate(note.quoted_text, 140))}”</em></p>` +
      (q ? `<p><b>Q:</b> ${escapeHtml(truncate(q.content, 160))}</p>` : '') +
      (a ? a.html : '');
    prev.classList.add('note');
  }

  prev.innerHTML = `<div class="gp-kicker">${escapeHtml(kicker)}</div><div class="gp-body">${bodyHtml}</div>`;
  prev.hidden = false;

  const canvasRect = qs('#graph-canvas').getBoundingClientRect();
  const nodeRect = node.getBoundingClientRect();
  const pw = prev.offsetWidth;
  const ph = prev.offsetHeight;
  let x = nodeRect.right - canvasRect.left + 14;
  if (x + pw > canvasRect.width - 12) x = nodeRect.left - canvasRect.left - pw - 14;
  x = Math.max(8, x);
  let y = nodeRect.top - canvasRect.top;
  y = Math.max(8, Math.min(y, canvasRect.height - ph - 12));
  prev.style.left = `${x}px`;
  prev.style.top = `${y}px`;
}

function hidePreview() {
  qs('#graph-preview').hidden = true;
}

// ------------------------------------------------------------- maximize

function maximize(kind, id) {
  hidePreview();
  const overlay = qs('#node-max');
  const kickerEl = qs('#nm-kicker');
  const titleEl = qs('#nm-title');
  const bodyEl = qs('#nm-body');
  bodyEl.textContent = '';

  if (kind === 'step') {
    const step = findStep(id);
    if (!step) return;
    const idx = state.topic.steps.findIndex((s) => s.id === id);
    kickerEl.textContent = `Step ${idx + 1} of ${state.topic.steps.length}`;
    kickerEl.classList.remove('note');
    titleEl.textContent = step.prompt ? `Steered: ${step.prompt}` : state.topic.title;
    bodyEl.innerHTML = step.html;
  } else {
    const note = findNote(id);
    if (!note) return;
    kickerEl.textContent = note.resolved ? 'Side note · resolved' : 'Side note';
    kickerEl.classList.add('note');
    titleEl.textContent = `“${truncate(note.quoted_text, 90)}”`;
    for (const msg of note.messages) {
      bodyEl.append(el('div', { class: 'nm-thread-msg' },
        el('span', { class: 'who', text: msg.role === 'user' ? 'You asked' : 'Waypoint' }),
        el('div', { html: msg.html })));
    }
  }

  maxCtx = { kind, id };
  overlay.hidden = false;
}
