// Zen v2 S0 spike (prd-zen-user-widgets-v2 §14.2, UWB13, UWB14).
//
// Question: inside the sealed widget frame (srcdoc, sandbox="allow-scripts",
// null origin) with the proposed widget CSP, under the proposed APP CSP
// (R3: + 'wasm-unsafe-eval', worker-src 'self' blob:), do these work on
// WKWebView (signed macOS build) and WebKitGTK (z13flow)?
//   WebGL2 · wasm · a blob: worker (+ wasm and fetch('data:') inside it) ·
//   fetch('data:') / fetch(blob:) · a 2 MB inline nonced script
// …while the negatives stay blocked: network, eval, new Function, an
// un-nonced script, an injected onerror handler, storage, window.parent,
// window.open, a data: worker.
//
// Two frames run the same suite: `proposed` (the UWB14 widget CSP) and
// `control` (the same minus 'wasm-unsafe-eval', worker-src blob: and
// connect-src data: blob:), so the table shows which directive buys what.
//
// How it runs:
//  - In the app: build with `VITE_K2_ZEN_SPIKE=s0` (a signed build via
//    ./scripts/build-app.sh, see .k2/prds/prd-zen-user-widgets-v2.md §15).
//    index.tsx then renders this page instead of the app. A normal build
//    never sets the variable, so the branch and this module drop out.
//  - In dev: `?zenspike=s0` on the Vite dev page (import.meta.env.DEV only).
//  - In a plain browser: scripts/zen-spike-s0/serve.ts serves this module
//    under a chosen outer (app) CSP header; run-browsers.mjs drives
//    Playwright WebKit and Chromium; wkwebview.swift drives the system
//    WKWebView through a custom URL scheme (closest local stand-in for
//    tauri://localhost).
//
// Results land in a table, in `window.__zenSpikeS0` ({done, results, env})
// for automation, and in a "Copy results" text box.
//
// Spike code: never shipped in a normal build, never imported by the app.

export type SpikeExpect = 'pass' | 'blocked' | 'info'
export type SpikeVariant = 'proposed' | 'control'

export interface SpikeResult {
  variant: SpikeVariant
  id: string
  expect: SpikeExpect
  /** For pass: it worked. For blocked: it was blocked. For info: it worked. */
  ok: boolean
  detail: string
  ms: number
}

export interface SpikeEnv {
  userAgent: string
  outerCsp: string
  isTauri: boolean
  startedAt: string
}

/** The UWB14 widget-profile CSP, as B4 will emit it from frame-csp.ts. */
export function proposedWidgetCsp(nonce: string): string {
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

/** The control: today's UW13 widget CSP (no wasm, no workers, no data: fetch). */
export function controlWidgetCsp(nonce: string): string {
  return [
    "default-src 'none'",
    `script-src 'nonce-${nonce}'`,
    "style-src 'unsafe-inline'",
    'img-src data: blob:',
    'media-src data: blob:',
    'font-src data:',
    "connect-src 'none'",
    "frame-src 'none'",
    "child-src 'none'",
    "worker-src 'none'",
    "object-src 'none'",
    "manifest-src 'none'",
    "form-action 'none'",
    "base-uri 'none'",
  ].join('; ')
}

/** 2,000,000 characters from a fixed PRNG, and its FNV-1a 32 hash. */
export const BIG_SCRIPT_CHARS = 2_000_000

function bigPayload(): { text: string; hash: number } {
  const alphabet = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789'
  let seed = 0x2f6b2a1d
  const parts: string[] = []
  for (let i = 0; i < BIG_SCRIPT_CHARS; i++) {
    seed = (seed * 1664525 + 1013904223) >>> 0
    parts.push(alphabet[seed % 62])
  }
  const text = parts.join('')
  return { text, hash: fnv1a(text) }
}

export function fnv1a(s: string): number {
  let h = 0x811c9dc5
  for (let i = 0; i < s.length; i++) {
    h ^= s.charCodeAt(i)
    h = Math.imul(h, 0x01000193) >>> 0
  }
  return h >>> 0
}

function randomNonce(): string {
  const b = new Uint8Array(16)
  crypto.getRandomValues(b)
  return btoa(String.fromCharCode(...b)).replace(/=+$/, '')
}

// The in-frame suite. Plain JS text (never a compiled function), so a
// bundler can't rename what it relies on. Placeholders: __VARIANT__,
// __BIG_HASH__, __BIG_LEN__.
const FRAME_SUITE = String.raw`
(function () {
  var VARIANT = '__VARIANT__';
  var results = [];
  var violations = [];
  document.addEventListener('securitypolicyviolation', function (e) {
    violations.push((e.effectiveDirective || e.violatedDirective) + ' ' + (e.blockedURI || ''));
  });
  function now() { return (performance && performance.now) ? performance.now() : Date.now(); }
  function rec(id, expect, ok, detail, t0) {
    results.push({ variant: VARIANT, id: id, expect: expect, ok: !!ok, detail: String(detail).slice(0, 300), ms: Math.round(now() - t0) });
  }
  function timeout(p, ms) {
    return Promise.race([p, new Promise(function (_, rej) { setTimeout(function () { rej(new Error('timeout ' + ms + 'ms')); }, ms); })]);
  }
  function errText(e) { return (e && (e.name ? e.name + ': ' : '') + (e.message || e)) || String(e); }
  var WASM = new Uint8Array([0,97,115,109,1,0,0,0,1,7,1,96,2,127,127,1,127,3,2,1,0,7,7,1,3,97,100,100,0,0,10,9,1,7,0,32,0,32,1,106,11]);
  function blobUrl(src, type) { return URL.createObjectURL(new Blob([src], { type: type || 'text/javascript' })); }
  function workerRoundTrip(url, msg) {
    return new Promise(function (res, rej) {
      var w;
      try { w = new Worker(url); } catch (e) { rej(e); return; }
      w.onmessage = function (ev) { res(ev.data); w.terminate(); };
      w.onerror = function (ev) { rej(new Error('worker error: ' + (ev.message || 'load failed'))); w.terminate(); };
      w.postMessage(msg);
    });
  }

  var tests = [
    ['big-inline-script', 'pass', function () {
      var b = window.__big;
      if (!b) throw new Error('2 MB nonced script did not run');
      if (b.len !== __BIG_LEN__ || b.hash !== __BIG_HASH__) throw new Error('len ' + b.len + ' hash ' + b.hash);
      return 'len ' + b.len + ', parse+run ' + b.ms + ' ms';
    }],
    ['webgl2', 'pass', function () {
      var c = document.createElement('canvas'); c.width = 64; c.height = 64; document.body.appendChild(c);
      var gl = c.getContext('webgl2', { preserveDrawingBuffer: true });
      if (!gl) throw new Error('getContext(webgl2) = null');
      function sh(type, src) { var s = gl.createShader(type); gl.shaderSource(s, src); gl.compileShader(s);
        if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) throw new Error(gl.getShaderInfoLog(s)); return s; }
      var p = gl.createProgram();
      gl.attachShader(p, sh(gl.VERTEX_SHADER, '#version 300 es\nin vec2 a;void main(){gl_Position=vec4(a,0.,1.);}'));
      gl.attachShader(p, sh(gl.FRAGMENT_SHADER, '#version 300 es\nprecision mediump float;out vec4 o;void main(){o=vec4(0.2,0.6,1.,1.);}'));
      gl.linkProgram(p); if (!gl.getProgramParameter(p, gl.LINK_STATUS)) throw new Error(gl.getProgramInfoLog(p));
      gl.useProgram(p);
      var vao = gl.createVertexArray(); gl.bindVertexArray(vao);
      var buf = gl.createBuffer(); gl.bindBuffer(gl.ARRAY_BUFFER, buf);
      gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1,-1, 3,-1, -1,3]), gl.STATIC_DRAW);
      gl.enableVertexAttribArray(0); gl.vertexAttribPointer(0, 2, gl.FLOAT, false, 0, 0);
      gl.viewport(0, 0, 64, 64); gl.drawArrays(gl.TRIANGLES, 0, 3);
      var px = new Uint8Array(4); gl.readPixels(32, 32, 1, 1, gl.RGBA, gl.UNSIGNED_BYTE, px);
      var good = Math.abs(px[0] - 51) < 3 && Math.abs(px[1] - 153) < 3 && px[2] > 250;
      var dbg = gl.getExtension('WEBGL_debug_renderer_info');
      var r = dbg ? gl.getParameter(dbg.UNMASKED_RENDERER_WEBGL) : gl.getParameter(gl.RENDERER);
      if (!good) throw new Error('pixel ' + Array.prototype.join.call(px, ','));
      return gl.getParameter(gl.VERSION) + ' · ' + r;
    }],
    ['raf-frames-500ms', 'info', function () {
      return new Promise(function (res) { var n = 0, t = now();
        function f() { n++; if (now() - t < 500) requestAnimationFrame(f); else res(n + ' frames'); }
        requestAnimationFrame(f); });
    }],
    ['wasm-instantiate', 'pass', function () {
      return WebAssembly.instantiate(WASM).then(function (m) {
        var v = m.instance.exports.add(40, 2); if (v !== 42) throw new Error('add = ' + v); return 'add(40,2) = 42';
      });
    }],
    ['wasm-compile-streaming', 'info', function () {
      if (!WebAssembly.compileStreaming) return 'no compileStreaming';
      return WebAssembly.compileStreaming(new Response(WASM, { headers: { 'Content-Type': 'application/wasm' } }))
        .then(function () { return 'compiled from a Response'; });
    }],
    ['fetch-data-url', 'pass', function () {
      return fetch('data:text/plain;base64,aGVsbG8=').then(function (r) { return r.text(); })
        .then(function (t) { if (t !== 'hello') throw new Error('got ' + t); return 'hello'; });
    }],
    ['fetch-blob-url', 'pass', function () {
      return fetch(blobUrl('blob-ok', 'text/plain')).then(function (r) { return r.text(); })
        .then(function (t) { if (t !== 'blob-ok') throw new Error('got ' + t); return 'blob-ok'; });
    }],
    ['blob-worker', 'pass', function () {
      return workerRoundTrip(blobUrl('onmessage=function(e){postMessage("echo:"+e.data)}'), 'hi')
        .then(function (d) { if (d !== 'echo:hi') throw new Error('got ' + d); return d; });
    }],
    ['blob-worker-wasm', 'pass', function () {
      var src = 'onmessage=function(e){WebAssembly.instantiate(e.data).then(function(m){postMessage(m.instance.exports.add(1,2))},function(x){postMessage("ERR "+x)})}';
      return workerRoundTrip(blobUrl(src), WASM).then(function (d) { if (d !== 3) throw new Error(String(d)); return 'add(1,2) = 3 in worker'; });
    }],
    ['blob-worker-fetch-data', 'pass', function () {
      var src = 'onmessage=function(){fetch("data:text/plain,inner").then(function(r){return r.text()}).then(function(t){postMessage(t)},function(x){postMessage("ERR "+x)})}';
      return workerRoundTrip(blobUrl(src), 1).then(function (d) { if (d !== 'inner') throw new Error(String(d)); return d; });
    }],
    ['offscreen-webgl2-in-worker', 'info', function () {
      var src = 'onmessage=function(){try{var c=new OffscreenCanvas(8,8);var g=c.getContext("webgl2");postMessage(g?"webgl2 ok":"null")}catch(x){postMessage("ERR "+x)}}';
      return workerRoundTrip(blobUrl(src), 1);
    }],
    ['audio-worklet-blob', 'info', function () {
      var AC = window.AudioContext || window.webkitAudioContext; if (!AC) return 'no AudioContext';
      var ac = new AC(); if (!ac.audioWorklet) { ac.close(); return 'no audioWorklet (state ' + ac.state + ')'; }
      return ac.audioWorklet.addModule(blobUrl('registerProcessor("p",class extends AudioWorkletProcessor{process(){return true}})'))
        .then(function () { ac.close(); return 'addModule ok (state ' + ac.state + ')'; });
    }],
    ['img-data-url', 'pass', function () {
      return new Promise(function (res, rej) { var i = new Image();
        i.onload = function () { res(i.width + 'x' + i.height); }; i.onerror = function () { rej(new Error('img error')); };
        i.src = 'data:image/gif;base64,R0lGODlhAQABAIAAAP///wAAACwAAAAAAQABAAACAkQBADs='; });
    }],
    ['data-worker', 'blocked', function () {
      return workerRoundTrip('data:text/javascript,onmessage=function(){postMessage(1)}', 1).then(function () { return 'RAN'; });
    }],
    ['fetch-loopback', 'blocked', function () {
      return fetch('http://127.0.0.1:1/').then(function () { return 'RAN'; });
    }],
    ['fetch-https', 'blocked', function () {
      return fetch('https://example.com/').then(function () { return 'RAN'; });
    }],
    ['eval', 'blocked', function () { return 'RAN ' + eval('1+1'); }],
    ['new-function', 'blocked', function () { return 'RAN ' + new Function('return 2')(); }],
    ['unnonced-inline-script', 'blocked', function () {
      if (window.__unnonced) return 'RAN'; throw new Error('did not run');
    }],
    ['injected-onerror', 'blocked', function () {
      var d = document.createElement('div'); d.innerHTML = '<img src="x-missing" onerror="window.__onerr=1">';
      document.body.appendChild(d);
      return new Promise(function (res, rej) { setTimeout(function () { if (window.__onerr) res('RAN'); else rej(new Error('handler did not run')); }, 400); });
    }],
    ['localStorage', 'blocked', function () { localStorage.setItem('k', 'v'); return 'RAN'; }],
    ['parent-document', 'blocked', function () { return 'RAN ' + typeof window.parent.document.body; }],
    ['window-open', 'blocked', function () { var w = window.open('about:blank'); if (w) { try { w.close(); } catch (e) {} return 'RAN'; } throw new Error('returned null'); }],
    ['ipc-handles-visible', 'info', function () {
      var w = window;
      return 'webkit.messageHandlers=' + typeof (w.webkit && w.webkit.messageHandlers) +
        ' __TAURI_INTERNALS__=' + typeof w.__TAURI_INTERNALS__ + ' ipc=' + typeof w.ipc + ' origin=' + location.origin;
    }],
  ];

  function run(i) {
    if (i >= tests.length) {
      results.push({ variant: VARIANT, id: 'csp-violations-seen', expect: 'info', ok: true, detail: violations.join(' | ').slice(0, 600), ms: 0 });
      parent.postMessage({ k2spike: 's0', variant: VARIANT, results: results }, '*');
      return;
    }
    var t = tests[i], id = t[0], expect = t[1], fn = t[2], t0 = now();
    var p;
    try { p = Promise.resolve(fn()); } catch (e) { p = Promise.reject(e); }
    timeout(p, 5000).then(function (v) {
      var ran = String(v).indexOf('RAN') === 0;
      if (expect === 'blocked') rec(id, expect, !ran, ran ? 'NOT BLOCKED: ' + v : v, t0);
      else rec(id, expect, true, v, t0);
    }, function (e) {
      if (expect === 'blocked') rec(id, expect, true, 'blocked: ' + errText(e), t0);
      else rec(id, expect, false, errText(e), t0);
    }).then(function () { run(i + 1); });
  }
  run(0);
})();
`

/** The whole srcdoc for one variant. Exported for tests. */
export function buildSpikeSrcDoc(variant: SpikeVariant, nonce: string, big: { text: string; hash: number }): string {
  const csp = variant === 'proposed' ? proposedWidgetCsp(nonce) : controlWidgetCsp(nonce)
  const suite = FRAME_SUITE.replace(/__VARIANT__/g, variant)
    .replace(/__BIG_HASH__/g, String(big.hash))
    .replace(/__BIG_LEN__/g, String(big.text.length))
  const bigScript =
    `var __t0 = performance.now(); var __s = "${big.text}";` +
    `var __h = 0x811c9dc5; for (var __i = 0; __i < __s.length; __i++) { __h ^= __s.charCodeAt(__i); __h = Math.imul(__h, 0x01000193) >>> 0; }` +
    `window.__big = { len: __s.length, hash: __h >>> 0, ms: Math.round(performance.now() - __t0) };`
  return (
    '<!doctype html><html><head>' +
    `<meta http-equiv="Content-Security-Policy" content="${csp}">` +
    '<meta charset="utf-8"><style>body{margin:0;background:transparent;color:#888;font:11px system-ui}</style>' +
    '</head><body>' +
    '<script>window.__unnonced = 1</script>' +
    `<script nonce="${nonce}">${bigScript}</script>` +
    `<script nonce="${nonce}">${suite}</script>` +
    '</body></html>'
  )
}

declare global {
  interface Window {
    __zenSpikeS0?: { done: boolean; results: SpikeResult[]; env: SpikeEnv }
  }
}

async function outerCsp(): Promise<string> {
  try {
    const r = await fetch(window.location.href, { cache: 'no-store' })
    return r.headers.get('content-security-policy') ?? '(no CSP header on this page)'
  } catch (e) {
    return `(could not read: ${String(e)})`
  }
}

function runVariant(host: HTMLElement, variant: SpikeVariant, big: { text: string; hash: number }): Promise<SpikeResult[]> {
  return new Promise((resolve) => {
    const frame = document.createElement('iframe')
    frame.setAttribute('sandbox', 'allow-scripts')
    frame.title = `zen spike s0 ${variant}`
    frame.style.cssText = 'width:96px;height:96px;border:1px solid #888;margin:4px'
    const timer = setTimeout(() => {
      window.removeEventListener('message', onMessage)
      resolve([
        { variant, id: 'frame-answered', expect: 'pass', ok: false, detail: 'no results within 60 s', ms: 60000 },
      ])
    }, 60_000)
    function onMessage(e: MessageEvent): void {
      if (e.source !== frame.contentWindow) return
      const d = e.data as { k2spike?: string; variant?: string; results?: SpikeResult[] }
      if (d?.k2spike !== 's0' || d.variant !== variant || !Array.isArray(d.results)) return
      clearTimeout(timer)
      window.removeEventListener('message', onMessage)
      resolve(d.results)
    }
    window.addEventListener('message', onMessage)
    frame.srcdoc = buildSpikeSrcDoc(variant, randomNonce(), big)
    host.appendChild(frame)
  })
}

function render(root: HTMLElement, env: SpikeEnv, results: SpikeResult[], done: boolean): void {
  const verdict = (r: SpikeResult): string =>
    r.expect === 'info' ? 'INFO' : r.variant === 'control' ? (r.ok ? 'as-proposed' : 'differs') : r.ok ? 'PASS' : 'FAIL'
  const fails = results.filter((r) => r.variant === 'proposed' && r.expect !== 'info' && !r.ok)
  const rows = results
    .map(
      (r) =>
        `<tr><td>${r.variant}</td><td>${r.id}</td><td>${r.expect}</td><td><b>${verdict(r)}</b></td>` +
        `<td>${r.ms}</td><td>${r.detail.replace(/[<&]/g, (c) => (c === '<' ? '&lt;' : '&amp;'))}</td></tr>`,
    )
    .join('')
  const head = done
    ? `<h2 data-testid="zen-spike-verdict">${fails.length === 0 ? 'S0: ALL PROPOSED CHECKS PASS' : `S0: ${fails.length} PROPOSED CHECK(S) FAIL`}</h2>`
    : '<h2>S0: running…</h2>'
  const text = JSON.stringify({ env, results }, null, 1)
  root.innerHTML =
    '<div style="font:12px ui-monospace,monospace;padding:12px;color:#ddd;background:#111;min-height:100vh;box-sizing:border-box;overflow:auto">' +
    head +
    `<div>UA: ${env.userAgent}</div><div>Outer (app) CSP: ${env.outerCsp.replace(/</g, '&lt;')}</div>` +
    `<div>Tauri: ${env.isTauri}</div><div id="zen-spike-frames"></div>` +
    '<table border="1" cellpadding="3" style="border-collapse:collapse;margin-top:8px">' +
    '<tr><th>variant</th><th>check</th><th>expect</th><th>verdict</th><th>ms</th><th>detail</th></tr>' +
    rows +
    '</table>' +
    '<p>Copy everything below into the report:</p>' +
    `<textarea data-testid="zen-spike-json" style="width:100%;height:200px">${text.replace(/</g, '&lt;')}</textarea>` +
    '</div>'
}

/** Mount the spike page in `root` and run both variants in turn. */
export async function startZenSpikeS0(root: HTMLElement): Promise<SpikeResult[]> {
  const env: SpikeEnv = {
    userAgent: navigator.userAgent,
    outerCsp: await outerCsp(),
    isTauri: '__TAURI_INTERNALS__' in window,
    startedAt: new Date().toISOString(),
  }
  window.__zenSpikeS0 = { done: false, results: [], env }
  render(root, env, [], false)
  const big = bigPayload()
  const results: SpikeResult[] = []
  for (const variant of ['proposed', 'control'] as const) {
    const host = document.getElementById('zen-spike-frames') ?? root
    results.push(...(await runVariant(host, variant, big)))
    window.__zenSpikeS0 = { done: false, results: [...results], env }
  }
  window.__zenSpikeS0 = { done: true, results, env }
  render(root, env, results, true)
  return results
}
