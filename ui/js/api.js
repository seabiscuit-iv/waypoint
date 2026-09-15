// Bridge to the Rust backend. Command args are camelCase here; Tauri maps
// them onto the snake_case Rust parameters.

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

export const api = {
  // auth
  getAuthStatus: () => invoke('get_auth_status'),
  setApiKey: (key) => invoke('set_api_key', { key }),
  clearApiKey: () => invoke('clear_api_key'),
  testApiKey: (key) => invoke('test_api_key', { key: key ?? null }),

  // settings
  getSettings: () => invoke('get_settings'),
  updateSettings: (settings) => invoke('update_settings', { settings }),

  // topics
  listTopics: () => invoke('list_topics'),
  createTopic: (title, seedContext) => invoke('create_topic', { title, seedContext: seedContext ?? null }),
  getTopic: (topicId) => invoke('get_topic', { topicId }),
  renameTopic: (topicId, title) => invoke('rename_topic', { topicId, title }),
  deleteTopic: (topicId) => invoke('delete_topic', { topicId }),
  duplicateTopic: (topicId) => invoke('duplicate_topic', { topicId }),
  setTopicStatus: (topicId, status) => invoke('set_topic_status', { topicId, status }),
  reorderTopics: (ids) => invoke('reorder_topics', { ids }),

  // spine
  advanceSpine: (topicId, steering) => invoke('advance_spine', { topicId, steering: steering ?? null }),
  regenerateStep: (stepId, steering) => invoke('regenerate_step', { stepId, steering: steering ?? null }),
  editStep: (stepId, content) => invoke('edit_step', { stepId, content }),
  cancelGeneration: (genId) => invoke('cancel_generation', { genId }),

  // side notes
  createSideNote: ({ topicId, stepId, startOffset, endOffset, quotedText, question }) =>
    invoke('create_side_note', { topicId, stepId, startOffset, endOffset, quotedText, question }),
  replySideNote: (noteId, content) => invoke('reply_side_note', { noteId, content }),
  retrySideNote: (noteId) => invoke('retry_side_note', { noteId }),
  setSideNoteResolved: (noteId, resolved) => invoke('set_side_note_resolved', { noteId, resolved }),
  deleteSideNote: (noteId) => invoke('delete_side_note', { noteId }),
  updateSideNoteAnchor: (noteId, startOffset, endOffset) =>
    invoke('update_side_note_anchor', { noteId, startOffset, endOffset }),

  // data
  exportTopicMarkdown: (topicId) => invoke('export_topic_markdown', { topicId }),
  importDocuments: () => invoke('import_documents'),
  readDroppedFiles: (paths) => invoke('read_dropped_files', { paths }),
  backupDatabase: () => invoke('backup_database'),
  restoreDatabase: () => invoke('restore_database'),
};

export function onEvent(name, handler) {
  listen(name, (event) => handler(event.payload));
}

/** Like onEvent, but resolves to an unlisten function for temporary listeners. */
export function onEventScoped(name, handler) {
  return listen(name, (event) => handler(event.payload));
}
