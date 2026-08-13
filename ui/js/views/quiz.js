// Quick quiz: one multiple-choice question over what the spine has covered.
// Nothing is persisted — this is a self-check, not part of the learning path.

import { qs, el, toastErr, openModal } from '../util.js';
import { api } from '../api.js';
import { state } from '../state.js';
import * as topicsView from './topics.js';

// Questions asked this session, so "Another question" doesn't repeat itself.
// Cleared when the topic changes.
let askedFor = null;
let asked = [];

export function initQuiz() {
  qs('#btn-quiz').addEventListener('click', () => openQuiz());
}

export function openQuiz() {
  if (!state.topic) return;
  if (state.topic.id !== askedFor) {
    askedFor = state.topic.id;
    asked = [];
  }

  const body = el('div', { class: 'quiz-body' });
  const m = openModal({ title: 'Quick quiz', body });
  loadQuestion(body, m);
}

async function loadQuestion(body, m) {
  body.textContent = '';
  body.append(el('div', { class: 'quiz-loading' },
    el('span', { class: 'thinking-dots' }, el('i'), el('i'), el('i')),
    el('span', { text: 'Writing a question…' })));

  let q;
  try {
    q = await api.generateQuiz(state.topic.id, asked);
  } catch (e) {
    body.textContent = '';
    body.append(
      el('p', { class: 'quiz-error', text: e?.message || String(e) }),
      el('div', { class: 'quiz-foot' },
        el('button', {
          class: 'btn btn-ghost btn-sm', text: 'Close', onclick: () => m.close(),
        }),
        el('button', {
          class: 'btn btn-primary btn-sm', text: 'Try again',
          onclick: () => loadQuestion(body, m),
        })));
    return;
  }

  asked.push(q.question);
  topicsView.renderCost();
  renderQuestion(body, m, q);
}

function renderQuestion(body, m, q) {
  body.textContent = '';
  body.append(el('div', { class: 'quiz-question', html: q.question_html }));

  const list = el('div', { class: 'quiz-options' });
  const buttons = [];
  let answered = false;

  q.options_html.forEach((optHtml, i) => {
    const btn = el('button', {
      class: 'quiz-option',
      onclick: () => {
        if (answered) return;
        answered = true;
        reveal(i);
      },
    },
      el('span', { class: 'qo-key', text: 'ABCD'[i] }),
      el('span', { class: 'qo-text', html: optHtml }));
    buttons.push(btn);
    list.append(btn);
  });
  body.append(list);

  const foot = el('div', { class: 'quiz-foot' });
  body.append(foot);

  function reveal(chosen) {
    const correct = q.correct_index;
    buttons.forEach((b, i) => {
      b.classList.add('revealed');
      if (i === correct) b.classList.add('correct');
      else if (i === chosen) b.classList.add('wrong');
    });

    body.insertBefore(
      el('div', { class: 'quiz-verdict ' + (chosen === correct ? 'right' : 'wrong') },
        el('b', { text: chosen === correct ? 'Correct' : 'Not quite' }),
        el('div', { class: 'quiz-explain', html: q.explanation_html })),
      foot,
    );

    foot.append(
      el('button', {
        class: 'btn btn-ghost btn-sm', text: 'Done', onclick: () => m.close(),
      }),
      el('button', {
        class: 'btn btn-primary btn-sm', text: 'Another question',
        onclick: () => loadQuestion(body, m),
      }));
  }
}
