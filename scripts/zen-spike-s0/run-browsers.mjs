// Zen v2 S0 spike: drive Playwright WebKit and Chromium over serve.ts and
// print the results (prd-zen-user-widgets-v2 §15). A first check only; the
// decisive runs are the signed app on z3mbpZ and z13flow.
//
//   bun scripts/zen-spike-s0/serve.ts --port 5199 &
//   node scripts/zen-spike-s0/run-browsers.mjs [http://127.0.0.1:5199] [out.json]
//
// Exits 1 if any `proposed` check under the `proposed` app CSP fails.

import { writeFileSync } from 'node:fs'
import { chromium, webkit } from 'playwright'

const base = process.argv[2] ?? 'http://127.0.0.1:5199'
const outFile = process.argv[3]
const all = []
let failed = 0

// Chromium runs as the installed Google Chrome (`channel: 'chrome'`), so a
// Playwright browser download is never needed for it.
for (const [name, engine, opts] of [
  ['webkit', webkit, {}],
  ['chrome', chromium, { channel: 'chrome' }],
]) {
  const browser = await engine.launch({ headless: true, ...opts })
  for (const app of ['v045', 'r3', 'proposed']) {
    const page = await browser.newPage()
    page.on('dialog', (d) => {
      console.log(`  !! ${name}/${app}: a ${d.type()} dialog opened from the frame`)
      void d.dismiss()
    })
    await page.goto(`${base}/?app=${app}`)
    await page.waitForFunction(() => window.__zenSpikeS0?.done === true, null, { timeout: 180_000 })
    const run = await page.evaluate(() => window.__zenSpikeS0)
    await page.close()
    console.log(`\n== ${name} (${browser.version()}), outer app CSP: ${app}`)
    for (const r of run.results) {
      const verdict =
        r.expect === 'info' ? 'INFO' : r.variant === 'control' ? (r.ok ? 'same' : 'DIFF') : r.ok ? 'PASS' : 'FAIL'
      console.log(`${r.variant.padEnd(8)} ${r.id.padEnd(28)} ${r.expect.padEnd(7)} ${verdict.padEnd(5)} ${r.detail}`)
      if (app === 'proposed' && r.variant === 'proposed' && r.expect !== 'info' && !r.ok) failed++
    }
    all.push({ engine: name, version: browser.version(), app, env: run.env, results: run.results })
    if (outFile) writeFileSync(outFile, JSON.stringify(all, null, 1))
  }
  await browser.close()
}

if (outFile) writeFileSync(outFile, JSON.stringify(all, null, 1))
console.log(`\n${failed === 0 ? 'OK' : `${failed} FAIL`}: proposed frame CSP under the proposed app CSP`)
process.exit(failed === 0 ? 0 : 1)
