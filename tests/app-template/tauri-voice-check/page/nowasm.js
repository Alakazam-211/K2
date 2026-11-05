// TEST ONLY: under a CSP without 'wasm-unsafe-eval', WebAssembly must be
// refused (so the directive is what lets the room's .wasm run).
'use strict'

async function main() {
  const r = {}
  try {
    const { instance } = await WebAssembly.instantiateStreaming(fetch('/add.wasm'))
    r.refused = false
    r.result = instance.exports.add(20, 22)
  } catch (e) {
    r.refused = true
    r.error = `${e.name}: ${e.message}`
  }
  await fetch('/result/nowasm', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(r) })
  // Another origin: same port, another host name.
  location.href = `http://localhost:${location.port}/other`
}

main()
