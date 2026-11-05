// TEST ONLY: a page on another origin (localhost, not 127.0.0.1) asks for
// the mic. k2_voice.rs must refuse it.
'use strict'

async function main() {
  const r = { origin: location.origin, secureContext: window.isSecureContext }
  if (!navigator.mediaDevices || !navigator.mediaDevices.getUserMedia) {
    r.getUserMedia = { ok: false, error: 'no mediaDevices' }
  } else {
    try {
      const s = await Promise.race([
        navigator.mediaDevices.getUserMedia({ audio: true }),
        new Promise((_, rej) => setTimeout(() => rej(new Error('timeout')), 10000)),
      ])
      s.getTracks().forEach((t) => t.stop())
      r.getUserMedia = { ok: true }
    } catch (e) {
      r.getUserMedia = { ok: false, error: `${e.name}: ${e.message}` }
    }
  }
  await fetch('/result/other', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(r) })
}

main()
