// Sealed-frame capability check, page generator. Writes one top page per
// variant into <out-dir>; `wkwebview-caps.swift` loads them in the system
// WKWebView and reports which features each frame still has. Capability
// checks only: the probe never sets a URL, so nothing touches the network.
//
//   npx vite-node --config vitest.config.ts scripts/sealed-frame-check/make-pages.ts <out-dir>
//   swift scripts/sealed-frame-check/wkwebview-caps.swift <out-dir>
//
// Variants (the Swift harness registers the init script for `init*`/`both*`):
//   nocsp     no guard and no frame CSP (baseline)
//   none      no guard anywhere, frame CSP on (baseline: the engine as is)
//   initonly  desktop all-frames init script only; srcdoc without the guard
//   inframe   in-frame guard only (frameSrcDoc `scripted`), no init script
//   both      init script + frameSrcDoc `scripted` (the shipped desktop shape)
//   widget    init script + `widget` profile with the real prelude guard

import { mkdirSync, writeFileSync } from 'node:fs'
import { join, resolve } from 'node:path'
import { DNS_PREFETCH_OFF_META, frameCspMeta, frameSrcDoc } from '../../src/renderer/lib/frame-csp'
import { SEALED_FRAME_GUARD } from '../../src/renderer/lib/sealed-frame-guard'

const out = resolve(process.argv[2] ?? '')
if (!process.argv[2]) throw new Error('usage: make-pages.ts <out-dir>')
mkdirSync(out, { recursive: true })

const NONCE = 'c2VhbGVkLWZyYW1lLWNoZWNr'

// Runs inside the sealed frame. `frames[i]` is used (not contentWindow) so the
// in-frame guard's contentWindow hook can't make the answer look better.
const PROBE = `(function () {
  var r = {};
  function t(w) { try { return w ? typeof w.RTCPeerConnection : 'no-window'; } catch (e) { return 'threw:' + e.name; } }
  r.sealed = typeof RTCPeerConnection;
  r.sealedWebkit = typeof webkitRTCPeerConnection;
  r.sealedDataChannel = typeof RTCDataChannel;
  try { window.RTCPeerConnection = function () {}; } catch (e) { r.assignThrew = e.name; }
  try { Object.defineProperty(window, 'RTCPeerConnection', { value: function () {} }); } catch (e) { r.defineThrew = e.name; }
  r.sealedAfterAssign = typeof RTCPeerConnection;
  var a = document.createElement('iframe'); document.body.appendChild(a);
  r.childNoSrc = t(window.frames[0]);
  var b = document.createElement('iframe'); b.src = 'about:blank'; document.body.appendChild(b);
  r.childAboutBlank = t(window.frames[1]);
  var c = document.createElement('iframe'); document.body.appendChild(c);
  var cw = window.frames[2];
  try { cw.document.open(); cw.document.write('<p>x</p>'); cw.document.close(); r.childDocOpen = t(cw); } catch (e) { r.childDocOpen = 'threw:' + e.name; }
  r.childNoSrcViaContentWindow = t(a.contentWindow);
  try { void window.frames[0].document; r.childNoSrcSameOrigin = true; } catch (e) { r.childNoSrcSameOrigin = false; r.childNoSrcWhy = String(e.message).slice(0, 160); }
  // A child that runs its own script (srcdoc inherits this frame's CSP).
  window.addEventListener('message', function (e) { if (e.data && e.data.k2child) r.childSrcdocOwnScript = e.data.k2child; });
  var d = document.createElement('iframe');
  // In a nonce-only frame a page can read its own nonce; use it, as a widget could.
  var n = (document.currentScript && document.currentScript.nonce) || '';
  d.srcdoc = '<scr' + 'ipt nonce="' + n + '">parent.postMessage({ k2child: typeof RTCPeerConnection }, "*")</scr' + 'ipt>';
  document.body.appendChild(d);
  // A child given a javascript: URL (runs only if the engine lets the parent script it).
  window.addEventListener('message', function (e) { if (e.data && e.data.k2js) r.childJavascriptUrl = e.data.k2js; });
  var j = document.createElement('iframe');
  j.src = "javascript:parent.postMessage({ k2js: typeof RTCPeerConnection }, '*')";
  document.body.appendChild(j);
  setTimeout(function () {
    r.childNoSrcLater = t(window.frames[0]);
    r.childAboutBlankLater = t(window.frames[1]);
    r.childDocOpenLater = t(window.frames[2]);
    var l = document.createElement('link'); l.rel = 'preconnect'; document.head.appendChild(l);
    r.linkRelAfterSet = l.getAttribute('rel');
    var p = document.createElement('div'); p.innerHTML = '<link rel="dns-prefetch"><a ping="p">x</a>'; document.body.appendChild(p);
    var a2 = document.createElement('a'); a2.setAttribute('ping', 'p'); a2.ping = 'p'; document.body.appendChild(a2);
    r.pingAfterSet = a2.getAttribute('ping');
    setTimeout(function () {
      r.parsedHintLinksLeft = p.querySelectorAll('link').length;
      r.parsedPingLeft = p.querySelectorAll('a[ping]').length;
      r.dnsMetaFirst = !!document.head.firstElementChild && document.head.firstElementChild.getAttribute('http-equiv') === 'x-dns-prefetch-control';
      parent.postMessage({ k2caps: r }, '*');
    }, 50);
  }, 300);
})();`

function srcFor(variant: string): string {
  const body = '<!doctype html><html><head></head><body>'
  switch (variant) {
    case 'nocsp':
      return `<!doctype html><html><head></head><body><script>${PROBE}</script></body></html>`
    case 'none':
    case 'initonly':
      // Same head as frameSrcDoc minus the in-frame guard.
      return '<!doctype html>' + DNS_PREFETCH_OFF_META + frameCspMeta('scripted') + `<html><head></head><body><script>${PROBE}</script></body></html>`
    case 'inframe':
    case 'both':
      return frameSrcDoc(`${body}<script>${PROBE}</script></body></html>`, 'scripted')
    case 'widget':
      return frameSrcDoc(`${body}<script nonce="${NONCE}">${PROBE}</script></body></html>`, 'widget', {
        nonce: NONCE,
        prelude: `<script nonce="${NONCE}">${SEALED_FRAME_GUARD}</script>`,
      })
    default:
      throw new Error(`unknown variant ${variant}`)
  }
}

const VARIANTS = ['nocsp', 'none','initonly', 'inframe', 'both', 'widget']
for (const v of VARIANTS) {
  const sandbox = 'allow-scripts'
  const top = `<!doctype html><html><head><meta charset="utf-8"></head><body><script>
window.__caps = { variant: ${JSON.stringify(v)}, top: typeof RTCPeerConnection, done: false };
window.addEventListener('message', function (e) {
  if (e.data && e.data.k2caps) { window.__caps.frame = e.data.k2caps; window.__caps.topAfter = typeof RTCPeerConnection; window.__caps.done = true; }
});
var f = document.createElement('iframe');
f.setAttribute('sandbox', ${JSON.stringify(sandbox)});
f.srcdoc = ${JSON.stringify(srcFor(v)).replace(/<\//g, "<\\/")};
document.body.appendChild(f);
</script></body></html>`
  writeFileSync(join(out, `${v}.html`), top)
}
writeFileSync(join(out, 'guard.js'), SEALED_FRAME_GUARD)
writeFileSync(join(out, 'variants.txt'), VARIANTS.join('\n') + '\n')
console.log(`wrote ${VARIANTS.length} variants to ${out}`)
