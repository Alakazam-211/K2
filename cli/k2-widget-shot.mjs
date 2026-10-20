#!/usr/bin/env node
// k2 zen widget preview --dir <folder> --shot <png> (0.46.3): the headless
// runner. `k2` asks this computer's daemon for the shot page
// (POST /cli/zen/widget/shot: the staged folder checked and bundled, the
// sealed frame with K2's prelude, and a stand-in bridge with sample data)
// and runs this script with that answer. It fills the library slots from
// K2's bundled `zen-lib` folder (each file's sha256 checked), renders the
// page in Playwright's WebKit (the desktop webview's engine) and writes the
// PNG. Nothing here talks to the daemon or writes outside --out.
//
//   node k2-widget-shot.mjs --page <answer.json> --out <png> [--libs <dir>]
//                           [--wait-ms N]
//
// Prints one JSON line: {ok, png, ready, errors, refusals, calls, ms}.
// Exit: 0 shot and the widget said ready · 1 it never said ready or threw
// (the PNG is still written, to see why) · 2 usage · 3 no Playwright
// WebKit, or a library this script can't find.

import { createHash } from 'node:crypto'
import { existsSync, readFileSync, writeFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { dirname, join, resolve } from 'node:path'
import { execFileSync } from 'node:child_process'
import { fileURLToPath } from 'node:url'

const here = dirname(fileURLToPath(import.meta.url))
const INSTALL = 'npm install -g playwright && npx playwright install webkit'

function die(code, message) {
  process.stderr.write(`Error: ${message}\n`)
  process.exit(code)
}

function arg(name) {
  const i = process.argv.indexOf(name)
  if (i < 0) return null
  const v = process.argv[i + 1]
  if (!v || v.startsWith('--')) die(2, `${name} needs a value`)
  return v
}

const pagePath = arg('--page')
const out = arg('--out')
if (!pagePath || !out) die(2, 'usage: node k2-widget-shot.mjs --page <answer.json> --out <png> [--libs <dir>] [--wait-ms N]')
const waitMs = Number(arg('--wait-ms') ?? 400)
if (!Number.isFinite(waitMs) || waitMs < 0 || waitMs > 30000) die(2, '--wait-ms is 0 to 30000')

let page
try {
  page = JSON.parse(readFileSync(pagePath, 'utf8'))
} catch (e) {
  die(2, `${pagePath} is not the daemon's shot answer: ${e.message}`)
}
if (page.ok !== true || typeof page.frame !== 'string' || typeof page.host !== 'string') die(2, `${pagePath} is not a clean shot answer`)

// ── Playwright: the agent's workspace first, then NODE_PATH, then global ──
function requireFrom(r) {
  try {
    const req = createRequire(join(r, 'noop.js'))
    return req(req.resolve('playwright', { paths: [r] }))
  } catch {
    return null
  }
}

function loadPlaywright() {
  const roots = [process.cwd(), here, join(here, '..'), ...(process.env.NODE_PATH || '').split(':').filter(Boolean)]
  for (const r of roots) {
    const pw = requireFrom(r)
    if (pw) return pw
  }
  try {
    return requireFrom(execFileSync('npm', ['root', '-g'], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'] }).trim())
  } catch {
    return null // no npm
  }
}

// ── Libraries: K2's bundled zen-lib folder (a K2 checkout next to this
// script, or --libs) ────────────────────────────────────────────────────────
const libDirs = [arg('--libs'), process.env.ZEN_LIB_DIR, join(here, '..', 'src', 'renderer', 'zen-lib')].filter(Boolean)
const SEP = '\n;\n'
let frame = page.frame
for (const lib of page.libs || []) {
  const dir = libDirs.map((d) => join(d, lib.key)).find((d) => existsSync(d))
  if (!dir) {
    die(3, `this widget needs ${lib.key}: --shot reads K2's bundled libraries from a K2 checkout (src/renderer/zen-lib) or --libs <dir>; none has ${lib.key}`)
  }
  const texts = []
  for (const f of lib.files) {
    const buf = readFileSync(join(dir, f.name))
    const sum = createHash('sha256').update(buf).digest('hex')
    if (buf.length !== f.bytes || sum !== f.sha256) die(3, `${lib.key}/${f.name} doesn't match K2's pinned copy (size or sha256)`)
    texts.push(buf.toString('utf8'))
  }
  // zenScriptText: `</script` can't close the element early.
  const text = texts.join(SEP).replace(/<\/(script)/gi, '<\\/$1')
  if (!frame.includes(lib.slot)) die(1, `the frame has no slot for ${lib.key}`)
  frame = frame.split(lib.slot).join(text)
}
if (/__K2_SHOT_LIB_\d+__/.test(frame)) die(1, 'a library slot was left unfilled')

const slot = '__K2_SHOT_FRAME__'
const at = page.host.lastIndexOf(slot)
if (at < 0) die(1, 'the shot page has no frame slot')
const html = page.host.slice(0, at) + JSON.stringify(frame).replace(/<\//g, '<\\/') + page.host.slice(at + slot.length)

const pw = loadPlaywright()
if (!pw || !pw.webkit) die(3, `--shot needs Playwright's WebKit. Install it: ${INSTALL}`)

const started = Date.now()
let browser
try {
  browser = await pw.webkit.launch({ headless: true })
} catch (e) {
  die(3, `Playwright's WebKit didn't start (${String(e.message || e).split('\n')[0]}). Install it: ${INSTALL}`)
}
let result
try {
  const ctx = await browser.newContext({
    viewport: { width: page.width, height: page.height },
    colorScheme: page.scheme === 'dark' ? 'dark' : 'light',
    deviceScaleFactor: 2,
    reducedMotion: 'reduce',
  })
  const p = await ctx.newPage()
  const pageErrors = []
  p.on('pageerror', (e) => pageErrors.push(String(e.message || e).slice(0, 300)))
  await p.setContent(html)
  let ready = true
  try {
    await p.waitForFunction(() => window.__k2shot && window.__k2shot.ready === true, null, { timeout: 10000 })
  } catch {
    ready = false
  }
  await p.waitForTimeout(waitMs)
  const shot = await p.evaluate(() => window.__k2shot)
  const png = await p.screenshot({ clip: { x: 0, y: 0, width: page.width, height: page.height } })
  writeFileSync(resolve(out), png)
  const errors = [...shot.errors, ...pageErrors]
  result = {
    ok: ready && errors.length === 0,
    png: resolve(out),
    ready,
    errors,
    refusals: shot.refusals,
    calls: [...new Set(shot.calls)],
    width: page.width,
    height: page.height,
    scheme: page.scheme,
    ms: Date.now() - started,
  }
} finally {
  await browser.close()
}
process.stdout.write(JSON.stringify(result) + '\n')
process.exit(result.ok ? 0 : 1)
