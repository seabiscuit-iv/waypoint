// The spine: linear steps, the "+" composer, streaming generation,
// regenerate/edit/copy actions, and first-class inline error states.

import { qs, el, icon, toast, toastErr, timeAgo, autoGrow, copyText, patchStreamHtml, modKey } from './util.js';
import { api } from './api.js';
import { state, findStep, drainEarlyEvents } from './state.js';
import * as notes from './views/notes.js';
import * as topics from './views/topics.js';
import { refreshTopicList } from './nav.js';

const PENDING_ID = 'pending-step';

// ------------------------------------------------------------- rendering

export function renderTopicView() {
  const t = state.topic;
  qs('#topic-view').hidden = !t;
  qs('#empty-main').hidden = !!t;
  if (!t) return;

  const priorEl = qs('#topic-prior');
  priorEl.hidden = !t.prior_knowledge;
  priorEl.textContent = '';
  if (t.prior_knowledge) {
    priorEl.append(el('b', { text: 'Starting from: ' }), t.prior_knowledge);
  }

  const seedEl = qs('#topic-seed');
  if (t.seed_context) {
    seedEl.hidden = false;
    seedEl.textContent = '';
    seedEl.append(
      el('b', { text: 'Seeded from imported text' }),
      `. The path is grounded in ${t.seed_context.length.toLocaleString()} characters of your source material.`,
    );
    seedEl.title = t.seed_context.slice(0, 600);
  } else {
    seedEl.hidden = true;
  }

  renderSteps();
  updateIntro();
  resetComposer();
}

export function renderSteps() {
  const wrap = qs('#steps');
  wrap.textContent = '';
  if (!state.topic) return;
  state.topic.steps.forEach((step, i) => wrap.append(buildStepEl(step, i)));
}

export function stepElOf(stepId) {
  return qs(`#steps [data-step-id="${CSS.escape(stepId)}"]`);
}

function buildStepEl(step, index) {
  const contentEl = el('div', { class: 'step-content' });
  const detachedEl = el('div', { class: 'detached-notes' });

  const actions = el('div', { class: 'step-actions' },
    el('button', {
      class: 'icon-btn sm', title: 'Copy step', html: icon('copy'),
      onclick: () => { copyText(step.content); toast('Step copied'); },
    }),
    el('button', {
      class: 'icon-btn sm', title: 'Edit step', html: icon('pencil'),
      onclick: () => enterEdit(step.id),
    }),
    el('button', {
      class: 'icon-btn sm', title: 'Regenerate step', html: icon('refresh'),
      onclick: () => toggleRegenForm(step.id),
    }),
  );

  const foot = el('div', { class: 'step-foot' },
    el('span', { class: 'step-time', text: stepTimeLabel(step) }),
    actions);

  const card = el('div', { class: 'step-card' });
  if (step.prompt) {
    card.append(el('div', { class: 'steer-chip', title: step.prompt },
      el('span', { html: icon('steer') }),
      el('span', { text: step.prompt })));
  }
  card.append(contentEl, detachedEl, foot);

  const stepEl = el('article', {
    class: 'step',
    dataset: { stepId: step.id },
  },
    el('div', { class: 'step-gutter' }, el('span', { class: 'step-no', text: String(index + 1) })),
    card);

  refreshStepContentEl(stepEl, step);
  return stepEl;
}

function stepTimeLabel(step) {
  const edited = step.updated_at !== step.created_at;
  return timeAgo(step.created_at) + (edited ? ' · edited' : '');
}

function refreshStepContentEl(stepEl, step) {
  const contentEl = stepEl.querySelector('.step-content');
  contentEl.innerHTML = step.html;
  notes.applyMarks(stepEl, step);
}

/** Re-render one step's content + note marks (after edits/note changes). */
export function refreshStep(stepId) {
  const step = findStep(stepId);
  const stepEl = stepElOf(stepId);
  if (step && stepEl && !stepEl.classList.contains('streaming') && !stepEl.classList.contains('editing')) {
    refreshStepContentEl(stepEl, step);
  }
}

export function refreshAllSteps() {
  if (!state.topic) return;
  for (const step of state.topic.steps) refreshStep(step.id);
}

function updateIntro() {
  const show = !!state.topic && state.topic.steps.length === 0 && !state.spineGen;
  qs('#spine-intro').hidden = !show;
  if (show) qs('.intro-topic').textContent = state.topic.title;
}

// ------------------------------------------------------------- scrolling

function scroller() {
  return qs('#spine-scroll');
}

function nearBottom() {
  const s = scroller();
  return s.scrollHeight - s.scrollTop - s.clientHeight < 180;
}

function scrollToBottom() {
  const s = scroller();
  s.scrollTop = s.scrollHeight;
}

export function scrollToStep(stepId, { flash = true } = {}) {
  const stepEl = stepElOf(stepId);
  if (!stepEl) return;
  stepEl.scrollIntoView({ block: 'center', behavior: 'smooth' });
  if (flash) {
    stepEl.classList.add('flash');
    setTimeout(() => stepEl.classList.remove('flash'), 1400);
  }
}

// j/k step focus navigation
let focusedIdx = -1;

export function focusStep(delta) {
  if (!state.topic || state.topic.steps.length === 0) return;
  const n = state.topic.steps.length;
  focusedIdx = focusedIdx < 0 ? (delta > 0 ? 0 : n - 1) : Math.min(n - 1, Math.max(0, focusedIdx + delta));
  document.querySelectorAll('.step.focused').forEach((s) => s.classList.remove('focused'));
  const step = state.topic.steps[focusedIdx];
  const stepEl = stepElOf(step.id);
  if (stepEl) {
    stepEl.classList.add('focused');
    stepEl.scrollIntoView({ block: 'center', behavior: 'smooth' });
  }
}

// ------------------------------------------------------------- composer

export function initSpine() {
  const plusBtn = qs('#btn-plus');
  const wrapEl = qs('#composer-input-wrap');
  const input = qs('#composer-input');
  const goBtn = qs('#composer-go');
  const stopBtn = qs('#btn-stop-gen');

  plusBtn.addEventListener('click', () => expandComposer());
  goBtn.addEventListener('click', () => submitComposer());
  input.addEventListener('input', () => autoGrow(input));
  input.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' && !e.shiftKey && !modKey(e)) {
      e.preventDefault();
      submitComposer();
    } else if (e.key === 'Escape') {
      e.preventDefault();
      collapseComposer();
    }
  });
  stopBtn.addEventListener('click', () => {
    if (state.spineGen) api.cancelGeneration(state.spineGen.genId).catch(() => {});
  });

  focusedIdx = -1;
}

export function expandComposer() {
  if (!state.topic || state.spineGen) return;
  qs('#btn-plus').hidden = true;
  qs('#composer-input-wrap').hidden = false;
  const input = qs('#composer-input');
  autoGrow(input);
  input.focus();
}

export function collapseComposer() {
  qs('#composer-input-wrap').hidden = true;
  if (!state.spineGen) qs('#btn-plus').hidden = false;
}

function resetComposer() {
  const busy = !!state.spineGen;
  qs('#composer-input-wrap').hidden = true;
  qs('#composer-input').value = '';
  qs('#btn-plus').hidden = busy;
  qs('#btn-stop-gen').hidden = !busy;
}

export function composerOpen() {
  return !qs('#composer-input-wrap').hidden;
}

async function submitComposer() {
  const input = qs('#composer-input');
  await advance(input.value.trim() || null);
}

let advancing = false;

/** Advance the spine (Ctrl+Enter path uses steering=null unless the composer is open). */
export async function advance(steering) {
  if (!state.topic || advancing) return;
  if (state.spineGen) {
    toast('A step is already being generated.', { error: true });
    return;
  }
  if (!state.online) {
    toast('You’re offline and can’t generate right now.', { error: true });
    return;
  }
  advancing = true;
  try {
    const res = await api.advanceSpine(state.topic.id, steering);
    qs('#composer-input').value = '';
    collapseComposer();
    beginSpineStream(res.gen_id, res.step_id, steering);
    drainEarlyEvents(res.gen_id, handleGenEvent);
  } catch (e) {
    toastErr(e);
  } finally {
    advancing = false;
  }
}

export function advanceViaKeyboard() {
  if (!state.topic || state.spineGen) return;
  if (composerOpen()) {
    submitComposer();
  } else {
    advance(null);
  }
}

// ------------------------------------------------------------- streaming

function thinkingRow(label) {
  return el('div', { class: 'thinking-row' },
    el('span', { class: 'thinking-dots' }, el('i'), el('i'), el('i')),
    el('span', { text: label }));
}

function beginSpineStream(genId, stepId, steering) {
  clearGenErrorCards();
  resetStreamBuffer();
  state.spineGen = { genId, stepId, kind: 'spine', steering };
  updateIntro();

  const contentEl = el('div', { class: 'step-content' }, thinkingRow('Waypoint is thinking…'));
  const card = el('div', { class: 'step-card' });
  if (steering) {
    card.append(el('div', { class: 'steer-chip', title: steering },
      el('span', { html: icon('steer') }),
      el('span', { text: steering })));
  }
  card.append(contentEl);
  const idx = state.topic.steps.length;
  const pending = el('article', { class: 'step streaming', id: PENDING_ID },
    el('div', { class: 'step-gutter' }, el('span', { class: 'step-no', text: String(idx + 1) })),
    card);
  qs('#steps').append(pending);

  qs('#btn-plus').hidden = true;
  qs('#composer-input-wrap').hidden = true;
  qs('#btn-stop-gen').hidden = false;
  scrollToBottom();
}

export function beginRegenStream(genId, stepId, steering) {
  const step = findStep(stepId);
  const stepEl = stepElOf(stepId);
  if (!step || !stepEl) return;
  resetStreamBuffer();
  state.spineGen = {
    genId, stepId, kind: 'regen', steering,
    originalHtml: step.html,
  };
  stepEl.classList.add('streaming');
  stepEl.querySelector('.regen-form')?.remove();
  const contentEl = stepEl.querySelector('.step-content');
  contentEl.textContent = '';
  contentEl.append(thinkingRow('Rethinking this step…'));
  qs('#btn-stop-gen').hidden = false;
  qs('#btn-plus').hidden = true;
}

function streamTargetContent() {
  const g = state.spineGen;
  if (!g) return null;
  if (g.kind === 'spine') return qs(`#${PENDING_ID} .step-content`);
  return stepElOf(g.stepId)?.querySelector('.step-content') ?? null;
}

// Deltas arrive faster than the display refreshes, so they're coalesced into
// one DOM write per frame — several innerHTML swaps inside a single frame is
// work the user can never see.
let queuedHtml = null;
let flushHandle = 0;
let lastFlushedHtml = '';

function updateStreamContent(html) {
  queuedHtml = html;
  if (flushHandle) return;
  flushHandle = requestAnimationFrame(flushStreamContent);
}

function flushStreamContent() {
  flushHandle = 0;
  const html = queuedHtml;
  queuedHtml = null;
  if (html === null || html === lastFlushedHtml) return;

  const contentEl = streamTargetContent();
  if (!contentEl) return;
  lastFlushedHtml = html;

  const stick = nearBottom();
  contentEl.querySelector('.stream-caret')?.remove();
  const last = patchStreamHtml(contentEl, html);

  // Trail the caret at the end of the last line rather than orphaning it on
  // a line of its own below the paragraph.
  const caret = el('span', { class: 'stream-caret' });
  if (last && /^(P|LI|H[1-6]|BLOCKQUOTE|TD)$/.test(last.tagName)) last.append(caret);
  else contentEl.append(caret);

  if (state.spineGen?.kind === 'spine' && stick) scrollToBottom();
}

function resetStreamBuffer() {
  if (flushHandle) cancelAnimationFrame(flushHandle);
  flushHandle = 0;
  queuedHtml = null;
  lastFlushedHtml = '';
}

// ------------------------------------------------------------- events

/** Returns true if this module owns the event's generation. */
export function handleGenEvent(name, payload) {
  const g = state.spineGen;
  if (!g || payload.gen_id !== g.genId) return false;

  switch (name) {
    case 'gen:start':
      return true;
    case 'gen:delta':
      updateStreamContent(payload.html);
      return true;
    case 'gen:done':
      onDone(payload);
      return true;
    case 'gen:error':
      onError(payload);
      return true;
    default:
      return false;
  }
}

function onDone(payload) {
  const g = state.spineGen;
  state.spineGen = null;
  // Drop any queued frame — it would clobber the final render below.
  resetStreamBuffer();

  if (!state.topic || state.topic.id !== payload.topic_id) {
    refreshTopicList();
    resetComposer();
    return;
  }

  state.topic.usage = payload.usage;

  if (payload.kind === 'spine') {
    state.topic.steps.push(payload.step);
    const pending = qs(`#${PENDING_ID}`);
    const fresh = buildStepEl(payload.step, state.topic.steps.length - 1);
    if (pending) pending.replaceWith(fresh);
    else qs('#steps').append(fresh);
    if (nearBottom()) scrollToBottom();
  } else {
    const i = state.topic.steps.findIndex((s) => s.id === payload.step.id);
    if (i >= 0) {
      state.topic.steps[i] = payload.step;
      const old = stepElOf(payload.step.id);
      const fresh = buildStepEl(payload.step, i);
      if (old) old.replaceWith(fresh);
      // Anchors may have drifted after regeneration; marks were re-applied
      // by buildStepEl via quoted-text search / detached chips.
    }
  }

  resetComposer();
  updateIntro();
  topics.renderCost();
  refreshTopicList();
  if (g?.kind === 'spine') qs('#composer-input')?.focus({ preventScroll: true });
}

const ERROR_TITLES = {
  auth: 'API key problem',
  rate_limit: 'Rate limited',
  overloaded: 'Anthropic is busy',
  network: 'No connection',
  refusal: 'Claude declined',
  api: 'Something went wrong',
  db: 'Couldn’t save the step',
  invalid: 'Something went wrong',
};

function onError(payload) {
  const g = state.spineGen;
  state.spineGen = null;
  resetStreamBuffer();

  const stepEl = g && g.kind === 'regen' ? stepElOf(g.stepId) : null;

  // Restore regenerated step's original content in every error path.
  if (g?.kind === 'regen' && stepEl) {
    stepEl.classList.remove('streaming');
    const step = findStep(g.stepId);
    if (step) refreshStepContentEl(stepEl, step);
  }
  if (g?.kind === 'spine') {
    qs(`#${PENDING_ID}`)?.remove();
  }

  resetComposer();
  updateIntro();

  if (payload.kind === 'cancelled') return;

  const card = buildErrorCard(payload, g);
  if (g?.kind === 'regen' && stepEl) {
    stepEl.querySelector('.step-card')?.append(card);
  } else {
    qs('#steps').append(card);
    scrollToBottom();
  }
}

function buildErrorCard(payload, gen) {
  const actions = el('div', { class: 'ge-actions' });
  const retryBtn = el('button', {
    class: 'btn btn-ghost btn-sm', text: 'Retry',
    onclick: async () => {
      card.remove();
      if (!gen) return;
      if (gen.kind === 'regen') {
        try {
          const res = await api.regenerateStep(gen.stepId, gen.steering);
          beginRegenStream(res.gen_id, gen.stepId, gen.steering);
          drainEarlyEvents(res.gen_id, handleGenEvent);
        } catch (e) { toastErr(e); }
      } else {
        advance(gen.steering ?? null);
      }
    },
  });
  actions.append(retryBtn);
  if (payload.kind === 'auth') {
    actions.append(el('button', {
      class: 'btn btn-ghost btn-sm', text: 'Open Settings',
      onclick: () => { card.remove(); window.dispatchEvent(new CustomEvent('wp:open-settings')); },
    }));
  }
  actions.append(el('button', {
    class: 'btn btn-ghost btn-sm', text: 'Dismiss',
    onclick: () => card.remove(),
  }));

  const card = el('div', { class: 'gen-error' },
    el('span', { html: icon('alert') }),
    el('div', { class: 'ge-body' },
      el('div', { class: 'ge-title', text: ERROR_TITLES[payload.kind] || 'Something went wrong' }),
      el('div', { class: 'ge-msg', text: payload.message }),
      actions));
  return card;
}

function clearGenErrorCards() {
  document.querySelectorAll('#steps > .gen-error').forEach((n) => n.remove());
}

// ------------------------------------------------------------- editing

function enterEdit(stepId) {
  if (state.spineGen) return;
  const step = findStep(stepId);
  const stepEl = stepElOf(stepId);
  if (!step || !stepEl) return;

  stepEl.classList.add('editing');
  const card = stepEl.querySelector('.step-card');
  const prevChildren = [...card.children];
  prevChildren.forEach((c) => { c.hidden = true; });

  const ta = el('textarea', { spellcheck: 'false' });
  ta.value = step.content;
  const editor = el('div', { class: 'step-edit' },
    ta,
    el('div', { class: 'step-edit-actions' },
      el('button', {
        class: 'btn btn-ghost btn-sm', text: 'Cancel',
        onclick: () => restore(),
      }),
      el('button', {
        class: 'btn btn-primary btn-sm', text: 'Save',
        onclick: async () => {
          const content = ta.value.trim();
          if (!content) return;
          try {
            const updated = await api.editStep(stepId, content);
            const i = state.topic.steps.findIndex((s) => s.id === stepId);
            if (i >= 0) {
              state.topic.steps[i] = updated;
              stepEl.replaceWith(buildStepEl(updated, i));
            }
            toast('Step updated');
          } catch (e) {
            toastErr(e);
            restore();
          }
        },
      })));
  card.append(editor);
  ta.focus();

  function restore() {
    editor.remove();
    prevChildren.forEach((c) => { c.hidden = false; });
    stepEl.classList.remove('editing');
  }
}

// ------------------------------------------------------------- regen form

function toggleRegenForm(stepId) {
  if (state.spineGen) return;
  const stepEl = stepElOf(stepId);
  if (!stepEl) return;
  const existing = stepEl.querySelector('.regen-form');
  if (existing) {
    existing.remove();
    return;
  }
  const input = el('input', {
    type: 'text',
    placeholder: 'Optional new steering, e.g. “use an example instead”',
    spellcheck: 'false',
  });
  const go = async () => {
    const steering = input.value.trim() || null;
    form.remove();
    try {
      const res = await api.regenerateStep(stepId, steering);
      beginRegenStream(res.gen_id, stepId, steering);
      drainEarlyEvents(res.gen_id, handleGenEvent);
    } catch (e) {
      toastErr(e);
    }
  };
  const form = el('div', { class: 'regen-form' },
    input,
    el('button', { class: 'btn btn-primary btn-sm', text: 'Regenerate', onclick: go }));
  input.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') go();
    if (e.key === 'Escape') form.remove();
  });
  stepEl.querySelector('.step-card').append(form);
  input.focus();
}
