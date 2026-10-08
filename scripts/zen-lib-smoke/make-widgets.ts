// Write the library smokes as Garden widget folders, for the S6 signed-build
// run (prd-zen-user-widgets-v2 UWB17): one folder per library, asking for it
// with `requires.libs`, drawing PASS or FAIL in its box. Never writes into a
// real ~/.k2: pass the target folder explicitly (a scratch HOME's
// `.k2/zen/widgets/`), and place them in a scratch Garden.
//
//   bun scripts/zen-lib-smoke/make-widgets.ts --out <folder> [--libs three,leaflet]

import { mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { join, resolve } from 'node:path'
import { SMOKES } from './smokes'

const ROOT = resolve(import.meta.dir, '../..')
const manifest = JSON.parse(readFileSync(join(ROOT, 'src/shared/zen-lib.json'), 'utf8')) as {
  libs: { id: string; version: string; title: string }[]
}

const args = process.argv.slice(2)
const outAt = args.indexOf('--out')
if (outAt < 0 || !args[outAt + 1]) throw new Error('--out <folder> is required')
const out = resolve(args[outAt + 1])
if (/\/\.k2\/zen\/widgets\/?$/.test(out) && out.startsWith(resolve(process.env.HOME ?? '/nonexistent'))) {
  throw new Error(`refusing to write into the real ${out}; use a scratch HOME`)
}
const libsAt = args.indexOf('--libs')
const only = libsAt >= 0 ? new Set(args[libsAt + 1].split(',')) : null

const PNG = 'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII='

for (const lib of manifest.libs) {
  if (only && !only.has(lib.id)) continue
  const smoke = SMOKES[lib.id]
  if (!smoke) throw new Error(`no smoke for ${lib.id}`)
  const name = `lib-${lib.id.replace(/[^a-z0-9]+/g, '-')}`.slice(0, 40)
  const dir = join(out, name)
  mkdirSync(dir, { recursive: true })
  writeFileSync(
    join(dir, 'manifest.json'),
    `${JSON.stringify(
      {
        schema: 1,
        name: `Smoke: ${lib.title}`.slice(0, 40),
        description: `Runs ${lib.title} ${lib.version} once and shows PASS or FAIL.`,
        caps: [],
        requires: { libs: [`${lib.id}@${lib.version}`] },
      },
      null,
      2,
    )}\n`,
  )
  writeFileSync(
    join(dir, 'index.html'),
    '<!doctype html><html><head><meta charset="utf-8"><link rel="stylesheet" href="style.css"></head>' +
      '<body><p id="status">running…</p><div id="root"></div><script src="main.js"></script></body></html>\n',
  )
  writeFileSync(join(dir, 'style.css'), 'body{font:12px system-ui;color:var(--zen-text,#ddd)}#root{width:320px;height:200px;position:relative}\n')
  writeFileSync(
    join(dir, 'main.js'),
    `// ${lib.title} ${lib.version} smoke (scripts/zen-lib-smoke/smokes.ts).
(async function () {
  var status = document.getElementById('status');
  var root = document.getElementById('root');
  var dataPng = ${JSON.stringify(PNG)};
  var sent = false;
  function done(ok, detail) {
    if (sent) return;
    sent = true;
    status.textContent = (ok ? 'PASS ' : 'FAIL ') + ${JSON.stringify(lib.id)} + ': ' + String(detail).slice(0, 200);
  }
  try {
    await (async function () {
${smoke}
    })();
  } catch (e) {
    done(false, 'threw: ' + ((e && e.message) || e));
  }
  if (window.k2 && typeof window.k2.ready === 'function') window.k2.ready();
})();
`,
  )
  console.log(`${dir}`)
}
