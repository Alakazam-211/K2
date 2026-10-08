// Zen v2 S0 spike: serve the spike page in a plain browser under a chosen
// OUTER (app) CSP header, so the sealed srcdoc frame inherits it the way it
// inherits tauri.conf.json's CSP in the app (prd-zen-user-widgets-v2 §15).
//
//   bun scripts/zen-spike-s0/serve.ts [--port 5199]
//     http://127.0.0.1:5199/?app=v045      the v0.45.0 app CSP (today)
//     http://127.0.0.1:5199/?app=r3        v0.45.0 + R3 as approved ('wasm-unsafe-eval', worker-src 'self' blob:)
//     http://127.0.0.1:5199/?app=proposed  R3 + connect-src data: blob: (what feat/zen-v2's tauri.conf.json carries)
//     http://127.0.0.1:5199/?app=tauri     whatever src-tauri/tauri.conf.json says now
//     http://127.0.0.1:5199/?app=none      no outer CSP
//
//   bun scripts/zen-spike-s0/serve.ts --write <dir>
//     writes spike.js, index.html and csp-<mode>.txt for wkwebview.swift.
//
// Spike tooling only; never part of a build.

import { mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { join, resolve } from 'node:path'

const ROOT = resolve(import.meta.dir, '../..')

/** src-tauri/tauri.conf.json `app.security.csp` at v0.45.0 (ea805c84). */
export const V045_APP_CSP =
  "default-src 'self'; style-src 'self' 'unsafe-inline'; script-src 'self' 'unsafe-inline'; img-src 'self' asset: data: blob:; media-src 'self' asset: data: blob:; connect-src 'self' ipc://localhost http://localhost:* ws://localhost:* http://127.0.0.1:* ws://127.0.0.1:* http://*:* ws://*:* https://github.com https://objects.githubusercontent.com https://*.github.com https://*.supabase.co https://*.k2.dev wss://*.k2.dev; font-src 'self' data:"

/** R3 as approved (Rosson 2026-10-08): + 'wasm-unsafe-eval' in script-src, + worker-src 'self' blob:. */
export const R3_APP_CSP = V045_APP_CSP.replace(
  "script-src 'self' 'unsafe-inline';",
  "script-src 'self' 'unsafe-inline' 'wasm-unsafe-eval'; worker-src 'self' blob:;",
)

/**
 * R3 plus `data: blob:` in connect-src (S0 finding, 2026-10-07): a srcdoc
 * frame's fetch('data:'/'blob:') must pass the inherited app connect-src
 * too, and v0.45.0's has neither. Opens no network.
 */
export const PROPOSED_APP_CSP = R3_APP_CSP.replace("connect-src 'self' ", "connect-src 'self' data: blob: ")

function tauriCsp(): string {
  const conf = JSON.parse(readFileSync(join(ROOT, 'src-tauri/tauri.conf.json'), 'utf8')) as {
    app: { security: { csp: string } }
  }
  return conf.app.security.csp
}

const MODES: Record<string, () => string | null> = {
  v045: () => V045_APP_CSP,
  r3: () => R3_APP_CSP,
  proposed: () => PROPOSED_APP_CSP,
  tauri: tauriCsp,
  none: () => null,
}

async function buildSpike(): Promise<string> {
  const out = await Bun.build({
    entrypoints: [join(ROOT, 'src/renderer/dev/zen-spike-s0.ts')],
    target: 'browser',
    format: 'esm',
  })
  if (!out.success) throw new Error(`spike build failed: ${out.logs.map(String).join('\n')}`)
  return out.outputs[0].text()
}

const INDEX_HTML =
  '<!doctype html><html><head><meta charset="utf-8"><title>Zen S0 spike</title></head>' +
  '<body style="margin:0;background:#111"><div id="root"></div>' +
  '<script type="module">import { startZenSpikeS0 } from "./spike.js"; startZenSpikeS0(document.getElementById("root"));</script>' +
  '</body></html>'

const args = process.argv.slice(2)
const writeAt = args.indexOf('--write')
const js = await buildSpike()

if (writeAt >= 0) {
  const dir = resolve(args[writeAt + 1] ?? '')
  if (!args[writeAt + 1]) throw new Error('--write needs a directory')
  mkdirSync(dir, { recursive: true })
  writeFileSync(join(dir, 'spike.js'), js)
  writeFileSync(join(dir, 'index.html'), INDEX_HTML)
  for (const [mode, csp] of Object.entries(MODES)) writeFileSync(join(dir, `csp-${mode}.txt`), csp() ?? '')
  console.log(`wrote spike.js, index.html, csp-*.txt to ${dir}`)
} else {
  const portAt = args.indexOf('--port')
  const port = portAt >= 0 ? Number(args[portAt + 1]) : 5199
  Bun.serve({
    hostname: '127.0.0.1',
    port,
    fetch(req) {
      const url = new URL(req.url)
      const mode = url.searchParams.get('app') ?? 'proposed'
      const csp = MODES[mode]?.()
      const headers: Record<string, string> = { 'Cache-Control': 'no-store' }
      if (csp) headers['Content-Security-Policy'] = csp
      if (url.pathname === '/spike.js') return new Response(js, { headers: { ...headers, 'Content-Type': 'text/javascript' } })
      if (url.pathname === '/') return new Response(INDEX_HTML, { headers: { ...headers, 'Content-Type': 'text/html; charset=utf-8' } })
      return new Response('not found', { status: 404 })
    },
  })
  console.log(`zen spike s0 on http://127.0.0.1:${port}/?app=v045|r3|proposed|tauri|none`)
}
