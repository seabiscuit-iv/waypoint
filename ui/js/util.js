// Small DOM + formatting helpers shared by every module.

export const qs = (sel, root = document) => root.querySelector(sel);
export const qsa = (sel, root = document) => [...root.querySelectorAll(sel)];

/** Element builder: el('div', { class: 'x', onclick: fn }, child, ...) */
export function el(tag, attrs = {}, ...children) {
  const node = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (v == null || v === false) continue;
    if (k === 'class') node.className = v;
    else if (k === 'dataset') Object.assign(node.dataset, v);
    else if (k === 'html') node.innerHTML = v;
    else if (k === 'text') node.textContent = v;
    else if (k.startsWith('on') && typeof v === 'function') node.addEventListener(k.slice(2), v);
    else node.setAttribute(k, v === true ? '' : v);
  }
  for (const c of children.flat(Infinity)) {
    if (c == null || c === false) continue;
    node.append(c.nodeType ? c : document.createTextNode(String(c)));
  }
  return node;
}

export function escapeHtml(s) {
  return String(s)
    .replaceAll('&', '&amp;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;')
    .replaceAll('"', '&quot;');
}

export function debounce(fn, ms) {
  let t;
  return (...args) => {
    clearTimeout(t);
    t = setTimeout(() => fn(...args), ms);
  };
}

// ------------------------------------------------------------ formatting

export function fmtCost(usd) {
  if (!usd || usd <= 0) return '$0.00';
  if (usd < 0.01) return '<$0.01';
  return '$' + usd.toFixed(2);
}

export function fmtTokens(n) {
  n = n || 0;
  if (n < 1000) return String(n);
  if (n < 1_000_000) return (n / 1000).toFixed(n < 10_000 ? 1 : 0) + 'k';
  return (n / 1_000_000).toFixed(1) + 'M';
}

export function timeAgo(iso) {
  const then = new Date(iso).getTime();
  if (Number.isNaN(then)) return '';
  const s = Math.max(0, (Date.now() - then) / 1000);
  if (s < 60) return 'just now';
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ago`;
  if (s < 86400 * 7) return `${Math.floor(s / 86400)}d ago`;
  return new Date(iso).toLocaleDateString();
}

export function truncate(s, n) {
  s = String(s ?? '');
  return s.length > n ? s.slice(0, n - 1).trimEnd() + '…' : s;
}

const patchScratch = document.createElement('div');

/**
 * Applies streamed HTML to `host` by replacing only the trailing blocks that
 * actually changed. Streaming markdown is append-only at the block level, so
 * this normally rewrites just the last paragraph — assigning innerHTML would
 * instead destroy and reflow every settled block on every frame.
 * Returns the last element child, for caret placement.
 */
export function patchStreamHtml(host, html) {
  patchScratch.innerHTML = html;
  const next = Array.from(patchScratch.children);
  const cur = Array.from(host.children);

  let i = 0;
  while (i < cur.length && i < next.length && cur[i].outerHTML === next[i].outerHTML) i++;

  for (let j = cur.length - 1; j >= i; j--) cur[j].remove();
  for (let j = i; j < next.length; j++) host.append(next[j]);

  patchScratch.textContent = '';
  return host.lastElementChild;
}

/** Grow a textarea to fit its content (capped by CSS max-height). */
export function autoGrow(ta) {
  ta.style.height = 'auto';
  ta.style.height = ta.scrollHeight + 'px';
}

export function copyText(text) {
  const ta = el('textarea', { style: 'position:fixed;opacity:0;left:-9999px' });
  ta.value = text;
  document.body.append(ta);
  ta.select();
  try { document.execCommand('copy'); } catch { /* best effort */ }
  ta.remove();
}

// ------------------------------------------------------------ icons

const ICONS = {
  pencil: '<path d="M12.9 3.1a1.6 1.6 0 0 1 2.3 2.3l-8.6 8.6-3.1.8.8-3.1 8.6-8.6Z"/>',
  refresh: '<path d="M14 8a6 6 0 1 1-1.7-4.2M14 2.5V6h-3.5"/>',
  copy: '<rect x="5.5" y="5.5" width="8" height="8" rx="1.5"/><path d="M3 10.5V3.8A1.3 1.3 0 0 1 4.3 2.5H11"/>',
  doc: '<path d="M9 1.5H4.5A1.5 1.5 0 0 0 3 3v10a1.5 1.5 0 0 0 1.5 1.5h7A1.5 1.5 0 0 0 13 13V5.5L9 1.5Z"/><path d="M9 1.5V5.5H13"/>',
  dots: '<circle cx="3.2" cy="8" r="1.3" fill="currentColor" stroke="none"/><circle cx="8" cy="8" r="1.3" fill="currentColor" stroke="none"/><circle cx="12.8" cy="8" r="1.3" fill="currentColor" stroke="none"/>',
  chat: '<path d="M3 3.5h10a1 1 0 0 1 1 1V10a1 1 0 0 1-1 1H8l-3 2.8V11H3a1 1 0 0 1-1-1V4.5a1 1 0 0 1 1-1Z"/>',
  check: '<path d="M3 8.5 6.5 12 13 4.5"/>',
  alert: '<path d="M8 2.5 14.5 13.5H1.5L8 2.5Z"/><path d="M8 7v3M8 12.2h.01"/>',
  x: '<path d="M4 4l8 8M12 4l-8 8"/>',
  trash: '<path d="M3 4.5h10M6.5 4.5V3.4a.9.9 0 0 1 .9-.9h1.2a.9.9 0 0 1 .9.9v1.1m2 0-.5 8a1.4 1.4 0 0 1-1.4 1.3H6.4A1.4 1.4 0 0 1 5 12.5l-.5-8M6.8 7v4M9.2 7v4"/>',
  spark: '<path d="M8 2v3M8 11v3M2 8h3M11 8h3M4 4l2 2M10 10l2 2M12 4l-2 2M6 10l-2 2"/>',
  export: '<path d="M8 2.5v7M5.2 6.7 8 9.5l2.8-2.8"/><path d="M3 11.5v.8A1.7 1.7 0 0 0 4.7 14h6.6a1.7 1.7 0 0 0 1.7-1.7v-.8"/>',
  steer: '<path d="M2.5 8h8M8 4.5 11.5 8 8 11.5"/><circle cx="13.2" cy="8" r="1" fill="currentColor" stroke="none"/>',
  book: '<path d="M3.5 3h7a1.5 1.5 0 0 1 1.5 1.5V13H5a1.5 1.5 0 0 1-1.5-1.5V3Z"/><path d="M3.5 10.8A1.7 1.7 0 0 1 5.2 9.3H12"/>',
  plus: '<path d="M8 3v10M3 8h10"/>',
  stop: '<rect x="4" y="4" width="8" height="8" rx="1.5"/>',
};

export function icon(name, cls = 'ic') {
  return `<svg viewBox="0 0 16 16" class="${cls}">${ICONS[name] || ''}</svg>`;
}

// ------------------------------------------------------------ toasts

export function toast(msg, { error = false, ms = 3200 } = {}) {
  const root = qs('#toast-root');
  const t = el('div', { class: 'toast' + (error ? ' err' : '') },
    el('span', { html: error ? icon('alert') : icon('check') }),
    el('span', { text: msg }));
  root.append(t);
  setTimeout(() => {
    t.classList.add('leaving');
    setTimeout(() => t.remove(), 260);
  }, ms);
}

export function toastErr(e) {
  const msg = e && typeof e === 'object' && e.message ? e.message : String(e);
  toast(msg, { error: true, ms: 4500 });
}

// ------------------------------------------------------------ menus

let activeMenu = null;

export function closeMenu() {
  if (activeMenu) {
    activeMenu.remove();
    activeMenu = null;
  }
}

/**
 * items: array of
 *   { label, icon?, danger?, checked?, onClick } | 'sep' | { header: label }
 */
export function showMenu(items, x, y) {
  closeMenu();
  const menu = el('div', { class: 'menu', role: 'menu' });
  for (const item of items) {
    if (item === 'sep') {
      menu.append(el('div', { class: 'menu-sep' }));
      continue;
    }
    if (item.header) {
      menu.append(el('div', { class: 'menu-label', text: item.header }));
      continue;
    }
    const btn = el('button', {
      class: 'menu-item' + (item.danger ? ' danger' : ''),
      onclick: () => {
        closeMenu();
        item.onClick?.();
      },
    });
    if (item.icon) btn.append(el('span', { html: icon(item.icon) }));
    btn.append(el('span', { text: item.label }));
    if (item.checked) btn.append(el('span', { class: 'check', html: icon('check') }));
    menu.append(btn);
  }
  qs('#menu-root').append(menu);
  const r = menu.getBoundingClientRect();
  menu.style.left = Math.min(x, window.innerWidth - r.width - 8) + 'px';
  menu.style.top = Math.min(y, window.innerHeight - r.height - 8) + 'px';
  activeMenu = menu;
  setTimeout(() => {
    document.addEventListener('pointerdown', onAway, { once: true, capture: true });
  }, 0);
  function onAway(e) {
    if (activeMenu && !activeMenu.contains(e.target)) closeMenu();
    else if (activeMenu) {
      document.addEventListener('pointerdown', onAway, { once: true, capture: true });
    }
  }
}

export function isMenuOpen() {
  return !!activeMenu;
}

// ------------------------------------------------------------ modals

const modalStack = [];

export function openModal({ title, body, foot, wide = false, onClose }) {
  const backdrop = el('div', { class: 'modal-backdrop' });
  const modal = el('div', { class: 'modal' + (wide ? ' wide' : '') });
  const head = el('div', { class: 'modal-head' },
    el('h2', { text: title }),
    el('button', { class: 'icon-btn', html: icon('x'), onclick: close }));
  const bodyEl = el('div', { class: 'modal-body' });
  if (body) bodyEl.append(body);
  modal.append(head, bodyEl);
  if (foot) {
    const footEl = el('div', { class: 'modal-foot' });
    footEl.append(...foot);
    modal.append(footEl);
  }
  backdrop.append(modal);
  backdrop.addEventListener('pointerdown', (e) => {
    if (e.target === backdrop) close();
  });
  qs('#modal-root').append(backdrop);
  const entry = { backdrop, close, onClose };
  modalStack.push(entry);

  function close() {
    const i = modalStack.indexOf(entry);
    if (i >= 0) modalStack.splice(i, 1);
    backdrop.remove();
    onClose?.();
  }
  return { close, modal, body: bodyEl };
}

export function closeTopModal() {
  const entry = modalStack[modalStack.length - 1];
  if (entry) {
    entry.close();
    return true;
  }
  return false;
}

export function isModalOpen() {
  return modalStack.length > 0;
}

export function confirmModal({ title, message, confirmLabel = 'Delete', danger = true }) {
  return new Promise((resolve) => {
    let done = false;
    const finish = (v) => {
      if (!done) {
        done = true;
        resolve(v);
      }
    };
    const cancelBtn = el('button', { class: 'btn btn-ghost', text: 'Cancel' });
    const okBtn = el('button', {
      class: 'btn ' + (danger ? 'btn-danger' : 'btn-primary'),
      text: confirmLabel,
    });
    const m = openModal({
      title,
      body: el('p', { class: 'dim', style: 'line-height:1.55; font-size:13.5px', text: message }),
      foot: [cancelBtn, okBtn],
      onClose: () => finish(false),
    });
    cancelBtn.onclick = () => m.close();
    okBtn.onclick = () => {
      finish(true);
      m.close();
    };
    okBtn.focus();
  });
}

export function promptModal({ title, label, value = '', placeholder = '', confirmLabel = 'Save' }) {
  return new Promise((resolve) => {
    let done = false;
    const finish = (v) => {
      if (!done) {
        done = true;
        resolve(v);
      }
    };
    const input = el('input', { type: 'text', value, placeholder, spellcheck: 'false' });
    const cancelBtn = el('button', { class: 'btn btn-ghost', text: 'Cancel' });
    const okBtn = el('button', { class: 'btn btn-primary', text: confirmLabel });
    const field = el('div', { class: 'field' });
    if (label) field.append(el('label', { text: label }));
    field.append(input);
    const m = openModal({
      title,
      body: field,
      foot: [cancelBtn, okBtn],
      onClose: () => finish(null),
    });
    const submit = () => {
      const v = input.value.trim();
      if (!v) return;
      finish(v);
      m.close();
    };
    cancelBtn.onclick = () => m.close();
    okBtn.onclick = submit;
    input.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') submit();
    });
    input.focus();
    input.select();
  });
}
