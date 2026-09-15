// Shared app state. Modules read/write this directly and call each other's
// render functions; there is no reactive framework.

export const state = {
  auth: { configured: false, masked_key: null },
  settings: { model: 'claude-opus-5', step_size: 'standard', theme: 'system', diagrams: false },
  topics: [],          // TopicSummary[]
  currentTopicId: null,
  topic: null,         // TopicDetail of the open topic
  view: 'spine',       // 'spine' | 'graph'
  online: navigator.onLine,

  // In-flight generation bookkeeping (single slot each).
  spineGen: null,      // { genId, stepId, kind: 'spine'|'regen', steering, originalHtml? }
  noteGen: null,       // { genId, noteId }
  noteErrors: new Map(), // noteId -> error message (retryable)
  reviewingSteps: new Set(), // stepIds whose diagrams are still being checked
  reviewingMessages: new Set(), // side-note messageIds, likewise

  search: null,        // { query, matches, idx }
};

export function findStep(id) {
  return state.topic?.steps.find((s) => s.id === id) ?? null;
}

export function findNote(id) {
  return state.topic?.side_notes.find((n) => n.id === id) ?? null;
}

export function notesForStep(stepId) {
  return state.topic ? state.topic.side_notes.filter((n) => n.anchor_step_id === stepId) : [];
}

// Events that arrive for a generation before the invoke() promise resolves
// (rare, but the emit/IPC race is real). They're replayed once the gen is
// registered.
export const earlyEvents = [];

export function stashEarlyEvent(name, payload) {
  earlyEvents.push({ name, payload });
  if (earlyEvents.length > 60) earlyEvents.shift();
}

export function drainEarlyEvents(genId, handler) {
  for (let i = 0; i < earlyEvents.length; i++) {
    if (earlyEvents[i].payload.gen_id === genId) {
      const { name, payload } = earlyEvents.splice(i, 1)[0];
      i--;
      handler(name, payload);
    }
  }
}
