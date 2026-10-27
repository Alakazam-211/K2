#!/usr/bin/env node
// Build @k2/activity's two browser scripts from sdk/activity/src (0.46.5
// activity SDK). `node scripts/zen-lib/build-k2-activity.mjs` writes them;
// `--check` writes nothing and exits 1 when either is stale. The vitest
// twin (sdk/activity/test/build.test.ts) runs the same build.
//
// - src/renderer/zen-lib/k2-activity@1.0.0/k2-activity.js: the Garden
//   widget library `k2-activity@1` (global K2Activity), from widget.ts.
//   Its zen-lib.json row carries the bytes and sha256 (run
//   scripts/zen-lib/vendor.ts after a rebuild).
// - sdk/activity/browser/k2-activity-bar.js: <k2-activity-bar> for App pages,
//   from element.ts.
//
// esbuild is the repo's own (node_modules/esbuild), pinned by bun.lock, so
// the bytes are reproducible.

import { readFileSync, writeFileSync, mkdirSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import * as esbuild from 'esbuild'

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '../..')

export const K2_ACTIVITY_VERSION = '1.0.0'

export const OUTPUTS = [
  {
    entry: 'sdk/activity/src/widget.ts',
    out: `src/renderer/zen-lib/k2-activity@${K2_ACTIVITY_VERSION}/k2-activity.js`,
    banner: `/*! k2-activity ${K2_ACTIVITY_VERSION} (FSL-1.1-Apache-2.0), K2's widget kit: the activity bar. Copyright 2025-2026 Alakazam Labs. */`,
  },
  {
    entry: 'sdk/activity/src/element.ts',
    out: 'sdk/activity/browser/k2-activity-bar.js',
    banner: `/*! k2-activity-bar ${K2_ACTIVITY_VERSION} (FSL-1.1-Apache-2.0), <k2-activity-bar> for K2 App pages. Copyright 2025-2026 Alakazam Labs. */`,
  },
]

/** Build every output in memory: [{out, text}]. */
export async function buildAll() {
  const results = []
  for (const o of OUTPUTS) {
    const r = await esbuild.build({
      entryPoints: [resolve(ROOT, o.entry)],
      bundle: true,
      format: 'iife',
      minify: true,
      target: ['es2020', 'safari15'],
      legalComments: 'none',
      charset: 'utf8',
      banner: { js: o.banner },
      write: false,
      logLevel: 'silent',
    })
    results.push({ out: o.out, text: r.outputFiles[0].text })
  }
  return results
}

async function main() {
  const check = process.argv.includes('--check')
  const stale = []
  for (const { out, text } of await buildAll()) {
    const path = resolve(ROOT, out)
    let current = null
    try {
      current = readFileSync(path, 'utf8')
    } catch {
      current = null
    }
    if (current === text) continue
    stale.push(out)
    if (!check) {
      mkdirSync(dirname(path), { recursive: true })
      writeFileSync(path, text)
      console.log(`wrote ${out} (${Buffer.byteLength(text)} bytes)`)
    }
  }
  if (check && stale.length) {
    console.error(`build-k2-activity --check: stale ${stale.join(', ')}; run node scripts/zen-lib/build-k2-activity.mjs`)
    process.exit(1)
  }
  if (!stale.length) console.log('build-k2-activity: every output is fresh')
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) await main()
