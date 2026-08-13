// Boot, backend-event routing, global keyboard shortcuts, offline handling.

import { qs, qsa, toastErr, isMenuOpen, closeMenu, isModalOpen, closeTopModal, modKey, MOD_LABEL } from './util.js';
import { api, onEvent } from './api.js';
import { state, stashEarlyEvent } from './state.js';
import * as spine from './spine.js';
import * as notes from './views/notes.js';
import * as graph from './views/graph.js';
import * as search from './views/search.js';
import * as sidebar from './views/sidebar.js';
import * as topicsView from './views/topics.js';
import * as settings from './views/settings.js';
import * as quiz from './views/quiz.js';
import { selectTopic, refreshTopicList, toggleView } from './nav.js';

async function boot() {
  const [auth, appSettings, topicList] = await Promise.all([
    api.getAuthStatus(),
    api.getSettings(),
    api.listTopics(),
  ]);
  state.auth = auth;
  state.settings = appSettings;
  state.topics = topicList;
  settings.applyTheme(appSettings.theme);

  spine.initSpine();
  notes.initNotes();
  graph.initGraph();
  search.initSearch();
  topicsView.initTopics();
  quiz.initQuiz();
  settings.initGate(enterApp);

  qs('#btn-settings').addEventListener('click', () => settings.openSettings());
  qs('#btn-shortcuts').addEventListener('click', () => settings.openShortcuts());
  window.addEventListener('wp:open-settings', () => settings.openSettings());
  window.addEventListener('wp:ledger-changed', () => topicsView.renderLedger());

  sidebar.renderTopics();
  localizeShortcutHints();
  wireBackendEvents();
  wireKeyboard();
  wireOnline();

  if (state.auth.configured) {
    enterApp();
  } else {
    qs('#app').hidden = false; // sits behind the gate, ready
    settings.showGate();
  }
}

function enterApp() {
  qs('#app').hidden = false;
  if (state.topics.length > 0 && !state.currentTopicId) {
    selectTopic(state.topics[0].id).catch(toastErr);
  }
}

/** Tooltips are authored with "Ctrl"; on macOS they should read "⌘". */
function localizeShortcutHints() {
  if (MOD_LABEL === 'Ctrl') return;
  for (const node of qsa('[title*="Ctrl+"]')) {
    node.title = node.title.replace(/Ctrl\+/g, MOD_LABEL);
  }
}

// ------------------------------------------------------------- events

function routeGenEvent(name, payload) {
  const claimed =
    spine.handleGenEvent(name, payload) || notes.handleGenEvent(name, payload);
  if (!claimed) {
    // Either an emit/IPC race (gen registered a beat later) or a generation
    // for a topic that's no longer open — keep the sidebar truthful.
    if (name === 'gen:done' || name === 'note:done' || name === 'gen:error') {
      refreshTopicList();
    }
    if (payload && payload.gen_id) stashEarlyEvent(name, payload);
  }
}

function wireBackendEvents() {
  for (const name of ['gen:start', 'gen:delta', 'gen:done', 'note:done', 'gen:error']) {
    onEvent(name, (payload) => routeGenEvent(name, payload));
  }

  onEvent('ledger:updated', (payload) => {
    if (state.topic && state.topic.id === payload.topic_id) {
      state.topic.ledger = payload.entries;
      state.topic.usage = payload.usage;
      topicsView.renderLedger();
      topicsView.renderCost();
    }
    refreshTopicList();
  });

  onEvent('db:restored', () => {
    location.reload();
  });
}

// ------------------------------------------------------------- keyboard

function wireKeyboard() {
  document.addEventListener('keydown', (e) => {
    const active = document.activeElement;
    const typing =
      active && (active.tagName === 'INPUT' || active.tagName === 'TEXTAREA' || active.isContentEditable);

    if (e.key === 'Escape') {
      if (isMenuOpen()) { closeMenu(); return; }
      if (closeTopModal()) return;
      if (graph.isMaximized()) { graph.closeMax(); return; }
      if (!qs('#selection-popover').hidden) { notes.hidePopover(); return; }
      if (search.isOpen() && !typing) { search.closeSearch(); return; }
      if (notes.isPanelOpen() && !typing) { notes.closePanel(); return; }
      if (topicsView.isLedgerOpen() && !typing) { topicsView.closeLedger(); return; }
      if (spine.composerOpen() && !typing) { spine.collapseComposer(); return; }
      return;
    }

    if (!state.auth.configured) return;

    const mod = modKey(e);
    if (mod && e.key === 'Enter') {
      e.preventDefault();
      spine.advanceViaKeyboard();
      return;
    }
    if (mod && (e.key === 'f' || e.key === 'F')) {
      e.preventDefault();
      if (state.topic) search.openSearch();
      return;
    }
    if (mod && e.key === ',') {
      e.preventDefault();
      settings.openSettings();
      return;
    }
    if (mod && (e.key === 'g' || e.key === 'G')) {
      e.preventDefault();
      toggleView();
      return;
    }

    if (typing || isModalOpen() || isMenuOpen()) return;

    switch (e.key) {
      case 'n':
        if (state.topic && state.view === 'spine') {
          e.preventDefault();
          spine.expandComposer();
        }
        break;
      case 'a':
        if (notes.hasSelectionContext()) {
          e.preventDefault();
          notes.confirmSelectionAsk();
        }
        break;
      case 'g':
        toggleView();
        break;
      case 'j':
        if (state.view === 'spine') spine.focusStep(1);
        else graph.focusNode(1);
        break;
      case 'k':
        if (state.view === 'spine') spine.focusStep(-1);
        else graph.focusNode(-1);
        break;
      case 'Enter':
        if (state.view === 'graph' && !graph.isMaximized()) {
          e.preventDefault();
          graph.maximizeFocused();
        }
        break;
      case '0':
        if (state.view === 'graph') graph.resetCamera();
        break;
      case '/':
        e.preventDefault();
        if (state.topic) search.openSearch();
        break;
      case '?':
        settings.openShortcuts();
        break;
    }
  });
}

// ------------------------------------------------------------- offline

function wireOnline() {
  const banner = qs('#offline-banner');
  const update = () => {
    state.online = navigator.onLine;
    banner.hidden = state.online;
  };
  window.addEventListener('online', update);
  window.addEventListener('offline', update);
  update();
}

boot().catch((e) => {
  console.error('Waypoint failed to start', e);
  document.body.textContent = 'Waypoint failed to start: ' + (e?.message || e);
});
