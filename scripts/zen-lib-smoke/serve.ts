// Zen widget library smoke (prd-zen-user-widgets-v2 UWB17): every library
// in src/shared/zen-lib.json runs its smoke (smokes.ts) inside a sealed
// srcdoc frame (sandbox="allow-scripts", null origin) with the UWB14 widget
// CSP, under the app CSP from src-tauri/tauri.conf.json, the way a Garden
// widget will get it: the library inlined as a nonced classic script.
//
//   bun scripts/zen-lib-smoke/serve.ts [--port 5198]
//   node scripts/zen-lib-smoke/run.mjs [http://127.0.0.1:5198] [out.json]
//   open http://127.0.0.1:5198/?libs=three,leaflet   (one or a few, by id)
//
// Download libraries (Babylon.js, Plotly) are fetched here from their pinned
// URL and sha256-checked, like the daemon does (cached in the OS temp dir).
// A first check in plain browsers only: the decisive runs are the signed
// app on z3mbpZ and z13flow (make-widgets.ts writes the same smokes as
// widget folders for that).

import { createHash } from 'node:crypto'
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'

import { zenLibInlineSafe } from '../../src/renderer/lib/zen/zen-lib-inline'
import { SMOKES } from './smokes'

const ROOT = resolve(import.meta.dir, '../..')
const LIB_DIR = join(ROOT, 'src/renderer/zen-lib')
const CACHE = join(tmpdir(), 'k2-zen-lib-smoke-cache')
const SEPARATOR = '\n;\n' // ZEN_LIB_FILE_SEPARATOR in zen-lib-loader.ts

interface LibFile {
  name: string
  bytes: number
  sha256: string
  url?: string
}
interface Lib {
  id: string
  version: string
  title: string
  global: string
  source: 'bundled' | 'download'
  files: LibFile[]
}
const manifest = JSON.parse(readFileSync(join(ROOT, 'src/shared/zen-lib.json'), 'utf8')) as { libs: Lib[] }
const APP_CSP = (JSON.parse(readFileSync(join(ROOT, 'src-tauri/tauri.conf.json'), 'utf8')) as { app: { security: { csp: string } } })
  .app.security.csp

/** UWB14's widget-profile CSP (B4's frame-csp.ts emits the real one). */
export function widgetCsp(nonce: string): string {
  return [
    "default-src 'none'",
    `script-src 'nonce-${nonce}' 'wasm-unsafe-eval'`,
    "style-src 'unsafe-inline'",
    'img-src data: blob:',
    'media-src data: blob:',
    'font-src data:',
    'connect-src data: blob:',
    'worker-src blob:',
    "frame-src 'none'",
    "child-src 'none'",
    "object-src 'none'",
    "manifest-src 'none'",
    "form-action 'none'",
    "base-uri 'none'",
  ].join('; ')
}

const sha256 = (b: Buffer): string => createHash('sha256').update(b).digest('hex')

async function fileBytes(lib: Lib, f: LibFile): Promise<Buffer> {
  if (lib.source === 'bundled') return readFileSync(join(LIB_DIR, `${lib.id}@${lib.version}`, f.name))
  const cached = join(CACHE, `${lib.id}@${lib.version}`, f.name)
  if (existsSync(cached)) return readFileSync(cached)
  const r = await fetch(f.url as string)
  if (!r.ok) throw new Error(`GET ${f.url}: ${r.status}`)
  const b = Buffer.from(await r.arrayBuffer())
  mkdirSync(join(CACHE, `${lib.id}@${lib.version}`), { recursive: true })
  writeFileSync(cached, b)
  return b
}

async function libText(lib: Lib): Promise<string> {
  const parts: string[] = []
  for (const f of lib.files) {
    const b = await fileBytes(lib, f)
    if (b.length !== f.bytes || sha256(b) !== f.sha256) throw new Error(`${lib.id}/${f.name}: not the pinned bytes`)
    parts.push(zenLibInlineSafe(b.toString('utf8')))
  }
  return parts.join(SEPARATOR)
}

// The page: plain JS (no bundler), one sealed frame per library in turn.
const PAGE_JS = String.raw`
(async function () {
  const want = new URLSearchParams(location.search).get('libs');
  const libs = (await (await fetch('/manifest')).json()).libs.filter((l) => !want || want.split(',').includes(l.id));
  const smokes = await (await fetch('/smokes')).json();
  const table = document.getElementById('t');
  const results = [];
  window.__zenLibSmoke = { done: false, results, ua: navigator.userAgent };
  function nonce() { const b = new Uint8Array(16); crypto.getRandomValues(b); return btoa(String.fromCharCode(...b)).replace(/=+$/, ''); }
  for (const lib of libs) {
    const t0 = performance.now();
    let row;
    try {
      const res = await fetch('/lib?id=' + encodeURIComponent(lib.id));
      if (!res.ok) throw new Error(await res.text());
      const text = await res.text();
      const n = nonce();
      const csp = (await (await fetch('/csp?nonce=' + encodeURIComponent(n))).text());
      const smoke = smokes[lib.id];
      if (!smoke) throw new Error('no smoke for ' + lib.id);
      const reporter = "(function(){var v=[];document.addEventListener('securitypolicyviolation',function(e){v.push((e.effectiveDirective||e.violatedDirective)+' '+(e.blockedURI||''))});" +
        "var sent=false;window.__k2report=function(ok,d){if(sent)return;sent=true;parent.postMessage({k2smoke:" + JSON.stringify(lib.id) + ",ok:!!ok,detail:String(d).slice(0,300),violations:v.slice(0,5)},'*')};" +
        "window.addEventListener('error',function(e){window.__k2report(false,'error: '+(e.message||e))});" +
        "window.addEventListener('unhandledrejection',function(e){window.__k2report(false,'rejection: '+((e.reason&&e.reason.message)||e.reason))});})();";
      const runner = "(async function(){var root=document.getElementById('root');var dataPng='data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=';" +
        "var done=function(ok,d){window.__k2report(ok,d)};try{await (async function(){" + smoke + "})()}catch(e){done(false,'threw: '+((e&&e.name)||'')+' '+((e&&e.message)||e))}})();";
      const html = '<!doctype html><html><head><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="' + csp + '"></head>' +
        '<body style="margin:0"><div id="root" style="width:320px;height:200px;position:relative"></div>' +
        '<script nonce="' + n + '">' + reporter + '<\/script>' +
        '<script nonce="' + n + '">' + text + '<\/script>' +
        '<script nonce="' + n + '">' + runner + '<\/script></body></html>';
      const f = document.createElement('iframe');
      f.setAttribute('sandbox', 'allow-scripts');
      f.style.cssText = 'width:320px;height:200px;border:1px solid #444';
      const got = new Promise((res) => {
        const on = (ev) => { if (ev.source === f.contentWindow && ev.data && ev.data.k2smoke === lib.id) { window.removeEventListener('message', on); res(ev.data); } };
        window.addEventListener('message', on);
        setTimeout(() => { window.removeEventListener('message', on); res({ ok: false, detail: 'no result in 25 s', violations: [] }); }, 25000);
      });
      f.srcdoc = html;
      document.getElementById('frames').appendChild(f);
      const r = await got;
      f.remove();
      row = { id: lib.id, version: lib.version, source: lib.source, ok: r.ok, detail: r.detail, violations: r.violations || [], ms: Math.round(performance.now() - t0) };
    } catch (e) {
      row = { id: lib.id, version: lib.version, source: lib.source, ok: false, detail: 'harness: ' + ((e && e.message) || e), violations: [], ms: Math.round(performance.now() - t0) };
    }
    results.push(row);
    const tr = document.createElement('tr');
    for (const c of [row.id, row.ok ? 'PASS' : 'FAIL', row.ms + ' ms', row.detail, row.violations.join(' | ')]) { const td = document.createElement('td'); td.textContent = c; tr.appendChild(td); }
    table.appendChild(tr);
  }
  window.__zenLibSmoke.done = true;
})();
`

const PAGE =
  '<!doctype html><html><head><meta charset="utf-8"><title>Zen library smoke</title>' +
  '<style>body{font:12px system-ui;background:#111;color:#ddd}td{padding:2px 6px;vertical-align:top}</style></head>' +
  '<body><table id="t"></table><div id="frames"></div><script>' +
  PAGE_JS +
  '</script></body></html>'

const args = process.argv.slice(2)
const portAt = args.indexOf('--port')
const port = portAt >= 0 ? Number(args[portAt + 1]) : 5198

Bun.serve({
  hostname: '127.0.0.1',
  port,
  idleTimeout: 120,
  async fetch(req) {
    const url = new URL(req.url)
    const headers: Record<string, string> = { 'Cache-Control': 'no-store', 'Content-Security-Policy': APP_CSP }
    if (url.pathname === '/') return new Response(PAGE, { headers: { ...headers, 'Content-Type': 'text/html; charset=utf-8' } })
    if (url.pathname === '/manifest') return Response.json(manifest, { headers })
    if (url.pathname === '/smokes') return Response.json(SMOKES, { headers })
    if (url.pathname === '/csp') return new Response(widgetCsp(url.searchParams.get('nonce') ?? ''), { headers })
    if (url.pathname === '/lib') {
      const lib = manifest.libs.find((l) => l.id === url.searchParams.get('id'))
      if (!lib) return new Response('unknown lib', { status: 404, headers })
      try {
        return new Response(await libText(lib), { headers: { ...headers, 'Content-Type': 'text/javascript; charset=utf-8' } })
      } catch (e) {
        return new Response(String(e), { status: 502, headers })
      }
    }
    return new Response('not found', { status: 404, headers })
  },
})
console.log(`zen library smoke on http://127.0.0.1:${port}/  (?libs=id,id for a few)`)
