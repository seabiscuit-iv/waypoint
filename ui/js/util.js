// Small DOM + formatting helpers shared by every module.

export const qs = (sel, root = document) => root.querySelector(sel);
export const qsa = (sel, root = document) => [...root.querySelectorAll(sel)];

// Platform: macOS drives Cmd rather than Ctrl for app shortcuts, so the
// modifier is picked once here and used for both handling and display.
export const IS_MAC = /Mac|iPhone|iPad/.test(
  navigator.userAgentData?.platform || navigator.platform || navigator.userAgent,
);
export const MOD_LABEL = IS_MAC ? '⌘' : 'Ctrl';

/** True when the platform's app-shortcut modifier is held. */
export function modKey(e) {
  return IS_MAC ? e.metaKey : e.ctrlKey;
}

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

// Names refer to Lucide symbols in the sprite at the top of index.html.
// Same-document <use> deliberately: WebKit has never handled external
// sprite references reliably.
export function icon(name, cls = 'ic') {
  return `<svg viewBox="0 0 24 24" class="${cls}" aria-hidden="true"><use href="#i-${name}"/></svg>`;
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
