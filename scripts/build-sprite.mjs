// Regenerates the Lucide icon sprite embedded at the top of ui/index.html.
//
//   npm pack lucide-static && tar -xzf lucide-static-*.tgz
//   node scripts/build-sprite.mjs package/icons sprite.txt
//
// Then paste sprite.txt over the <svg id="icon-sprite"> block. Add a name
// here and re-run when a new icon is needed; do not hand-write SVG paths.

import { readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

const SRC = process.argv[2];
const OUT = process.argv[3];

// app icon name -> lucide file name
const MAP = {
  plus: 'plus',
  x: 'x',
  check: 'check',
  search: 'search',
  settings: 'settings',
  keyboard: 'keyboard',
  offline: 'wifi-off',
  book: 'book-open',
  quiz: 'circle-question-mark',
  export: 'download',
  'chevron-up': 'chevron-up',
  'chevron-down': 'chevron-down',
  stop: 'square',
  trash: 'trash-2',
  chat: 'message-square',
  pencil: 'pencil',
  refresh: 'refresh-cw',
  copy: 'copy',
  doc: 'file-text',
  dots: 'ellipsis',
  alert: 'triangle-alert',
  steer: 'corner-down-right',
};

const symbols = [];
for (const [name, file] of Object.entries(MAP)) {
  const raw = readFileSync(join(SRC, `${file}.svg`), 'utf8');
  const inner = raw
    .replace(/<!--[\s\S]*?-->/g, '')
    .replace(/^[\s\S]*?<svg[^>]*>/, '')
    .replace(/<\/svg>\s*$/, '')
    .split('\n')
    .map((l) => l.trim())
    .filter(Boolean)
    .join('');
  if (!inner) throw new Error(`empty icon: ${file}`);
  symbols.push(`<symbol id="i-${name}" viewBox="0 0 24 24">${inner}</symbol>`);
}

const sprite =
  '  <svg id="icon-sprite" aria-hidden="true" style="display:none">\n' +
  symbols.map((s) => '    ' + s).join('\n') +
  '\n  </svg>';

writeFileSync(OUT, sprite);
console.log(`${symbols.length} symbols, ${sprite.length} bytes`);
