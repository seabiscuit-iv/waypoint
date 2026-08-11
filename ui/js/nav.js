// Topic selection and spine/map view switching.

import { qs } from './util.js';
import { api } from './api.js';
import { state } from './state.js';
import * as spine from './spine.js';
import * as sidebar from './views/sidebar.js';
import * as topics from './views/topics.js';
import * as graph from './views/graph.js';
import * as notes from './views/notes.js';
import * as search from './views/search.js';

let savedSpineScroll = 0;

export function switchView(v) {
  if (!state.topic) return;
  const scroller = qs('#spine-scroll');
  if (state.view === 'spine' && v !== 'spine') {
    savedSpineScroll = scroller.scrollTop;
  }
  state.view = v;
  qs('#spine-view').hidden = v !== 'spine';
  qs('#graph-view').hidden = v !== 'graph';
  qs('#btn-view-spine').classList.toggle('active', v === 'spine');
  qs('#btn-view-graph').classList.toggle('active', v === 'graph');
  qs('#node-max').hidden = true;
  if (v === 'graph') {
    graph.buildGraph();
  } else {
    scroller.scrollTop = savedSpineScroll;
  }
}

export function toggleView() {
  switchView(state.view === 'spine' ? 'graph' : 'spine');
}

export async function selectTopic(id, { force = false } = {}) {
  if (state.currentTopicId === id && !force) return;
  notes.closePanel();
  search.closeSearch();
  topics.closeLedger();
  state.spineGen = null;
  state.noteGen = null;
  state.noteErrors.clear();
  state.currentTopicId = id;
  state.topic = id ? await api.getTopic(id) : null;
  state.view = 'spine';
  savedSpineScroll = 0;
  sidebar.renderTopics();
  topics.renderHeader();
  spine.renderTopicView();
  if (state.topic) switchView('spine');
}

/** Re-fetch topic summaries (cheap local query) and repaint the sidebar. */
export async function refreshTopicList() {
  try {
    state.topics = await api.listTopics();
    sidebar.renderTopics();
  } catch {
    /* sidebar refresh is best-effort */
  }
}
