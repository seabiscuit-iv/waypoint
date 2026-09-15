// Text anchoring: side-note offsets refer to the *plain text* of a step's
// rendered content (the concatenation of its text nodes, which matches
// Range.toString()). The quoted_text snapshot allows re-anchoring by search
// if content is edited or regenerated (§7 of the design doc).

/** Text nodes under root, skipping diagrams: their labels aren't prose. */
function textWalker(root) {
  return document.createTreeWalker(root, NodeFilter.SHOW_ELEMENT | NodeFilter.SHOW_TEXT, {
    acceptNode: (n) => {
      if (n.nodeType === Node.TEXT_NODE) return NodeFilter.FILTER_ACCEPT;
      return n.classList?.contains('diagram') ? NodeFilter.FILTER_REJECT : NodeFilter.FILTER_SKIP;
    },
  });
}

/** Concatenated text-node content of an element. */
export function plainText(root) {
  let out = '';
  const walker = textWalker(root);
  let n;
  while ((n = walker.nextNode())) out += n.nodeValue;
  return out;
}

/**
 * Current selection expressed as plain-text offsets within `containerEl`.
 * Returns { start, end, text } or null.
 */
export function selectionOffsets(containerEl) {
  const sel = window.getSelection();
  if (!sel || sel.rangeCount === 0 || sel.isCollapsed) return null;
  const range = sel.getRangeAt(0);
  if (!containerEl.contains(range.startContainer) || !containerEl.contains(range.endContainer)) {
    return null;
  }
  const rawStart = plainOffset(containerEl, range.startContainer, range.startOffset);
  const rawEnd = plainOffset(containerEl, range.endContainer, range.endOffset);
  const raw = plainText(containerEl).slice(rawStart, rawEnd);
  const text = raw.trim();
  if (!text) return null;
  const start = rawStart + (raw.length - raw.trimStart().length);
  return { start, end: start + text.length, text };
}

/** Plain-text offset of a DOM boundary point within root. */
function plainOffset(root, container, offset) {
  const before = document.createRange();
  before.setStart(root, 0);
  before.setEnd(container, offset);
  let pos = 0;
  const walker = textWalker(root);
  let node;
  while ((node = walker.nextNode())) {
    if (node === container) return pos + offset;
    if (!before.intersectsNode(node)) break;
    pos += node.nodeValue.length;
  }
  return pos;
}

const BLOCK_TAGS = /^(P|DIV|UL|OL|LI|PRE|BLOCKQUOTE|TABLE|THEAD|TBODY|TFOOT|TR|TH|TD|H[1-6]|HR)$/;

function isBlockGap(node) {
  if (node.nodeValue.trim()) return false;
  const inline = (sib) => sib && (sib.nodeType === Node.TEXT_NODE ||
    !(BLOCK_TAGS.test(sib.nodeName) || sib.classList?.contains('math-block')));
  return !inline(node.previousSibling) && !inline(node.nextSibling);
}

/**
 * Wraps the plain-text range [start, end) in <mark> elements — one per
 * intersected text node, so markup boundaries are never violated.
 * Math is wrapped whole rather than per token, and whitespace between block
 * elements is skipped. The first and last marks get mark-start / mark-end.
 * Returns the created marks.
 */
export function wrapPlainRange(root, start, end, className, dataset = {}) {
  if (end <= start) return [];
  const segments = [];
  let pos = 0;
  const walker = textWalker(root);
  let node;
  while ((node = walker.nextNode())) {
    const len = node.nodeValue.length;
    const nodeStart = pos;
    const nodeEnd = pos + len;
    if (nodeEnd > start && nodeStart < end) {
      const math = node.parentElement?.closest('math');
      if (math) {
        if (segments[segments.length - 1]?.node !== math) segments.push({ node: math });
      } else if (!isBlockGap(node)) {
        const s = Math.max(0, start - nodeStart);
        const e = Math.min(len, end - nodeStart);
        if (e > s) segments.push({ node, s, e });
      }
    }
    pos = nodeEnd;
    if (pos >= end) break;
  }
  const marks = [];
  for (const { node, s, e } of segments) {
    const range = document.createRange();
    if (s === undefined) {
      range.selectNode(node);
    } else {
      range.setStart(node, s);
      range.setEnd(node, e);
    }
    const mark = document.createElement('mark');
    mark.className = className;
    if (s === undefined && node.parentElement?.classList.contains('math-block')) {
      mark.classList.add('mark-block');
    }
    Object.assign(mark.dataset, dataset);
    try {
      range.surroundContents(mark);
      marks.push(mark);
    } catch {
      // A pathological partial-element range; skip this segment.
    }
  }
  if (marks.length) {
    marks[0].classList.add('mark-start');
    marks[marks.length - 1].classList.add('mark-end');
  }
  return marks;
}

/**
 * A copy of the rendered markup covering the plain-text range [start, end),
 * widened to whole equations where the range starts or ends inside math.
 * Returns a DocumentFragment, or null if the range is out of bounds.
 */
export function clonePlainRange(root, start, end) {
  if (end <= start) return null;
  const range = document.createRange();
  let pos = 0;
  let started = false;
  const walker = textWalker(root);
  let node;
  while ((node = walker.nextNode())) {
    const len = node.nodeValue.length;
    const math = node.parentElement?.closest('math');
    if (!started && pos + len > start) {
      if (math) range.setStartBefore(math);
      else range.setStart(node, start - pos);
      started = true;
    }
    if (started && pos + len >= end) {
      if (math) range.setEndAfter(math);
      else range.setEnd(node, end - pos);
      return range.cloneContents();
    }
    pos += len;
  }
  return null;
}
