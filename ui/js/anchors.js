// Text anchoring: side-note offsets refer to the *plain text* of a step's
// rendered content (the concatenation of its text nodes, which matches
// Range.toString()). The quoted_text snapshot allows re-anchoring by search
// if content is edited or regenerated (§7 of the design doc).

/** Concatenated text-node content of an element. */
export function plainText(root) {
  let out = '';
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
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
  const pre = document.createRange();
  pre.selectNodeContents(containerEl);
  pre.setEnd(range.startContainer, range.startOffset);
  const start = pre.toString().length;
  const text = range.toString();
  if (!text.trim()) return null;
  return { start, end: start + text.length, text };
}

/**
 * Wraps the plain-text range [start, end) in <mark> elements — one per
 * intersected text node, so markup boundaries are never violated.
 * Returns the created marks.
 */
export function wrapPlainRange(root, start, end, className, dataset = {}) {
  if (end <= start) return [];
  const segments = [];
  let pos = 0;
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  let node;
  while ((node = walker.nextNode())) {
    const len = node.nodeValue.length;
    const nodeStart = pos;
    const nodeEnd = pos + len;
    if (nodeEnd > start && nodeStart < end) {
      const s = Math.max(0, start - nodeStart);
      const e = Math.min(len, end - nodeStart);
      if (e > s) segments.push({ node, s, e });
    }
    pos = nodeEnd;
    if (pos >= end) break;
  }
  const marks = [];
  for (const { node, s, e } of segments) {
    const range = document.createRange();
    range.setStart(node, s);
    range.setEnd(node, e);
    const mark = document.createElement('mark');
    mark.className = className;
    Object.assign(mark.dataset, dataset);
    try {
      range.surroundContents(mark);
      marks.push(mark);
    } catch {
      // A pathological partial-element range; skip this segment.
    }
  }
  return marks;
}
