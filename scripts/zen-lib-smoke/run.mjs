// Drive the Zen library smoke (serve.ts) in Playwright WebKit and the
// installed Chrome, print one line per library, and exit 1 if any library
// fails in WebKit (prd-zen-user-widgets-v2 UWB17). A first check only; the
// decisive runs are the signed app on z3mbpZ and z13flow.
//
//   bun scripts/zen-lib-smoke/serve.ts --port 5198 &
//   node scripts/zen-lib-smoke/run.mjs [http://127.0.0.1:5198] [out.json] [libs=a,b]

import { writeFileSync } from 'node:fs'
import { chromium, webkit } from 'playwright'

const base = process.argv[2] ?? 'http://127.0.0.1:5198'
const outFile = process.argv[3]
const only = (process.argv.find((a) => a.startsWith('libs=')) ?? '').slice(5)
const all = []
let webkitFails = 0

for (const [name, engine, opts] of [
  ['webkit', webkit, {}],
  ['chrome', chromium, { channel: 'chrome' }],
]) {
  const browser = await engine.launch({ headless: true, ...opts })
  const page = await browser.newPage()
  page.on('dialog', (d) => {
    console.log(`  !! ${name}: a ${d.type()} dialog opened from a frame`)
    void d.dismiss()
  })
  await page.goto(`${base}/${only ? `?libs=${only}` : ''}`)
  await page.waitForFunction(() => window.__zenLibSmoke?.done === true, null, { timeout: 900_000 })
  const run = await page.evaluate(() => window.__zenLibSmoke)
  console.log(`\n== ${name} ${browser.version()}`)
  for (const r of run.results) {
    console.log(`${r.ok ? 'PASS' : 'FAIL'} ${r.id.padEnd(20)} ${String(r.ms).padStart(6)} ms  ${r.detail}${r.violations.length ? `  [csp: ${r.violations.join(' | ')}]` : ''}`)
    if (name === 'webkit' && !r.ok) webkitFails++
  }
  all.push({ engine: name, version: browser.version(), results: run.results })
  await browser.close()
}

if (outFile) writeFileSync(outFile, JSON.stringify(all, null, 1))
console.log(`\n${webkitFails === 0 ? 'OK' : `${webkitFails} FAIL`} in WebKit`)
process.exit(webkitFails === 0 ? 0 : 1)
