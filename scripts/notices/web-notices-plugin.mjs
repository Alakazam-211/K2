// The hosted web build's licence notices (Kessel Steward findings G2 and G3).
//
// The web app bundles the kept shiki grammars, so the texts their licences
// require ship with it, as they do in the desktop app's resources
// (src-tauri/tauri.conf.json): every resources/notices/shiki-* file (the 11
// licence texts and, for the three MPL-2.0 grammars bird2, hcl and
// terraform, the Source Code Form and the converted form the bundle is built
// from), plus resources/THIRD-PARTY-NOTICES.md. They land byte for byte in
// <outDir>/notices/, next to notices/index.html, a page that shows the texts
// and links every file. Help → Third-party notices opens that page.
//
// scripts/check-third-party-notices.sh --web-out <dir> checks a build.
import { readFileSync, readdirSync } from 'node:fs'
import { resolve } from 'node:path'

/** Folder inside the web build. */
export const WEB_NOTICES_DIR = 'notices'

/** The files the web build ships: [{ name, path }]. */
export function webNoticeFiles(root) {
  const dir = resolve(root, 'resources/notices')
  const shiki = readdirSync(dir)
    .filter((n) => n.startsWith('shiki-'))
    .sort()
  if (shiki.length === 0) throw new Error(`web notices: no shiki-* files in ${dir}`)
  return [
    { name: 'THIRD-PARTY-NOTICES.md', path: resolve(root, 'resources/THIRD-PARTY-NOTICES.md') },
    ...shiki.map((name) => ({ name, path: resolve(dir, name) })),
  ]
}

function esc(s) {
  return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;')
}

/** notices/index.html: the licence texts shown in full, the JSON grammar
 *  files (Source Code Form and converted form) linked. */
export function noticesIndexHtml(files) {
  const shown = files.filter((f) => !f.name.endsWith('.json'))
  const linked = files.filter((f) => f.name.endsWith('.json'))
  const list = files
    .map((f) => `<li><a href="${esc(encodeURIComponent(f.name))}">${esc(f.name)}</a></li>`)
    .join('\n')
  const texts = shown
    .map((f) => `<section id="${esc(f.name)}"><h2>${esc(f.name)}</h2><pre>${esc(f.text)}</pre></section>`)
    .join('\n')
  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Third-party notices</title>
<style>
:root { color-scheme: light dark; --bg: #fff; --fg: #1a1a1a; --muted: #666; --rule: #ddd; }
@media (prefers-color-scheme: dark) { :root { --bg: #141414; --fg: #e6e6e6; --muted: #9a9a9a; --rule: #333; } }
body { margin: 0 auto; max-width: 960px; padding: 24px 16px 48px; background: var(--bg); color: var(--fg);
  font: 14px/1.5 system-ui, -apple-system, "Segoe UI", sans-serif; }
h1 { font-size: 20px; margin: 0 0 8px; }
h2 { font-size: 15px; margin: 32px 0 8px; padding-top: 16px; border-top: 1px solid var(--rule); }
p { color: var(--muted); }
pre { white-space: pre-wrap; overflow-wrap: anywhere; font: 12px/1.45 ui-monospace, Menlo, Consolas, monospace; }
a { color: inherit; }
</style>
</head>
<body>
<h1>Third-party notices</h1>
<p>Licence notices for third-party code and data in this web app. Each file is linked below as it ships; the licence texts follow in full. ${
    linked.length
  } JSON files are the MPL-2.0 grammars' Source Code Form and the converted form this app is built from.</p>
<ul>
${list}
</ul>
${texts}
</body>
</html>
`
}

/** Vite plugin: emits the notices into the build (production builds only). */
export function webNoticesPlugin(root) {
  return {
    name: 'k2-web-notices',
    apply: 'build',
    generateBundle() {
      const files = webNoticeFiles(root).map((f) => {
        const bytes = readFileSync(f.path)
        return { ...f, bytes, text: bytes.toString('utf8') }
      })
      for (const f of files) {
        this.emitFile({ type: 'asset', fileName: `${WEB_NOTICES_DIR}/${f.name}`, source: f.bytes })
      }
      this.emitFile({ type: 'asset', fileName: `${WEB_NOTICES_DIR}/index.html`, source: noticesIndexHtml(files) })
    },
  }
}
