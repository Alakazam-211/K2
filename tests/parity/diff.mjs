#!/usr/bin/env node
// Pixel-diff the two parity captures — Style System P1 / Checkpoint A.
//
// Usage:  node tests/parity/diff.mjs [beforeLabel] [afterLabel]
//         (defaults: before, after — i.e. tests/parity/shots/{before,after})
//
// Policy: STRICT. Exit non-zero on:
//   - a missing/empty shots dir,
//   - a screen present in one capture but not the other,
//   - a dimension mismatch,
//   - ANY pixel difference above 0 (threshold 0, includeAA — anti-aliased
//     pixels count too; same engine + same platform must be byte-stable).
//
// Output: per-screen report to stdout, machine-readable report at
// shots/_diff/report.json, and a diff PNG at shots/_diff/<screen>.diff.png
// for every mismatching screen.
import * as fs from 'node:fs'
import * as path from 'node:path'
import { fileURLToPath } from 'node:url'
import { PNG } from 'pngjs'
import pixelmatch from 'pixelmatch'

const here = path.dirname(fileURLToPath(import.meta.url))
const beforeLabel = process.argv[2] ?? 'before'
const afterLabel = process.argv[3] ?? 'after'
const beforeDir = path.join(here, 'shots', beforeLabel)
const afterDir = path.join(here, 'shots', afterLabel)
const diffDir = path.join(here, 'shots', '_diff')

function fail(msg) {
  console.error(`\nPARITY DIFF: FAIL — ${msg}`)
  process.exit(1)
}

function listShots(dir, label) {
  if (!fs.existsSync(dir)) {
    fail(`no capture for label '${label}' (${dir} does not exist). Run: PARITY_LABEL=${label} bun run parity:capture`)
  }
  const shots = fs.readdirSync(dir).filter((f) => f.endsWith('.png')).sort()
  if (shots.length === 0) fail(`capture '${label}' is empty (${dir})`)
  return shots
}

const beforeShots = listShots(beforeDir, beforeLabel)
const afterShots = listShots(afterDir, afterLabel)

// The screen SETS must match exactly — a screen that silently vanished
// from one capture is a failure, not a skip.
const onlyBefore = beforeShots.filter((s) => !afterShots.includes(s))
const onlyAfter = afterShots.filter((s) => !beforeShots.includes(s))
if (onlyBefore.length > 0 || onlyAfter.length > 0) {
  fail(
    `screen sets differ.` +
      (onlyBefore.length > 0 ? ` Only in '${beforeLabel}': ${onlyBefore.join(', ')}.` : '') +
      (onlyAfter.length > 0 ? ` Only in '${afterLabel}': ${onlyAfter.join(', ')}.` : ''),
  )
}

fs.rmSync(diffDir, { recursive: true, force: true })
fs.mkdirSync(diffDir, { recursive: true })

const report = []
let failures = 0

console.log(`\nParity diff: '${beforeLabel}' vs '${afterLabel}' — ${beforeShots.length} screens\n`)

for (const shot of beforeShots) {
  const aBuf = fs.readFileSync(path.join(beforeDir, shot))
  const bBuf = fs.readFileSync(path.join(afterDir, shot))

  // Byte-identical file → 0 diff, skip the decode.
  if (aBuf.equals(bBuf)) {
    const { width, height } = PNG.sync.read(aBuf)
    report.push({ screen: shot, width, height, diffPixels: 0, diffPct: 0, identical: true })
    console.log(`  PASS  ${shot}  (byte-identical, ${width}x${height})`)
    continue
  }

  const a = PNG.sync.read(aBuf)
  const b = PNG.sync.read(bBuf)
  if (a.width !== b.width || a.height !== b.height) {
    failures++
    report.push({
      screen: shot,
      error: `dimension mismatch: ${a.width}x${a.height} vs ${b.width}x${b.height}`,
    })
    console.log(`  FAIL  ${shot}  dimension mismatch ${a.width}x${a.height} vs ${b.width}x${b.height}`)
    continue
  }

  const diff = new PNG({ width: a.width, height: a.height })
  const diffPixels = pixelmatch(a.data, b.data, diff.data, a.width, a.height, {
    threshold: 0,
    includeAA: true, // count anti-aliased pixels too — strict parity
  })
  const total = a.width * a.height
  const diffPct = (diffPixels / total) * 100
  const entry = { screen: shot, width: a.width, height: a.height, diffPixels, diffPct, identical: false }
  report.push(entry)

  if (diffPixels > 0) {
    failures++
    const diffPath = path.join(diffDir, shot.replace(/\.png$/, '.diff.png'))
    fs.writeFileSync(diffPath, PNG.sync.write(diff))
    console.log(`  FAIL  ${shot}  ${diffPixels} px differ (${diffPct.toFixed(4)}%) → ${path.relative(process.cwd(), diffPath)}`)
  } else {
    console.log(`  PASS  ${shot}  (0 px differ, ${a.width}x${a.height})`)
  }
}

fs.writeFileSync(
  path.join(diffDir, 'report.json'),
  JSON.stringify({ before: beforeLabel, after: afterLabel, generatedBy: 'tests/parity/diff.mjs', screens: report }, null, 2),
)
console.log(`\nReport: ${path.relative(process.cwd(), path.join(diffDir, 'report.json'))}`)

if (failures > 0) {
  fail(`${failures}/${beforeShots.length} screens differ`)
}
console.log(`\nPARITY DIFF: PASS — all ${beforeShots.length} screens pixel-identical\n`)
