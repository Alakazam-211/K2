#!/usr/bin/env node
// Every Zen widget library file the daemon serves starts with a licence
// banner (the Kessel Steward's ruling). The files are the `bundled` rows of
// src/shared/zen-lib.json, which crates/k2-daemon/src/zen_lib_bundled.rs
// embeds; this also checks that table names exactly those files, at their
// paths under src/renderer/zen-lib/. What a banner is: scripts/zen-lib/banners.mjs.
//
//   node scripts/zen-lib/check-banners.mjs             check; exit 1 on any failure
//   node scripts/zen-lib/check-banners.mjs --print     also print each file's first line
//   node scripts/zen-lib/check-banners.mjs --self-test the rule against scripts/zen-lib/banner-cases.json
//                                                      (shared with the partner build's port of the rule)
//
// No network, no packages: CI runs it (checks.yml, scripts/ci/checks.sh).

import { readFileSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

import { BANNER_WINDOW, bannerOf, bannerProblem, copyrightLines, glueBanner, hasBanner, libraryBanner } from './banners.mjs'

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '../..')
const MANIFEST = 'src/shared/zen-lib.json'
const TABLE = 'crates/k2-daemon/src/zen_lib_bundled.rs'
const CASES = 'scripts/zen-lib/banner-cases.json'
const args = new Set(process.argv.slice(2))

function selfTest() {
  // The rule's cases, shared with every port of it (banner-cases.json).
  const doc = JSON.parse(readFileSync(join(ROOT, CASES), 'utf8'))
  let bad = 0
  if (doc.window_bytes !== BANNER_WINDOW) {
    console.error(`FAIL self-test: ${CASES} says the window is ${doc.window_bytes} bytes, banners.mjs ${BANNER_WINDOW}`)
    bad++
  }
  if (!Array.isArray(doc.cases) || !doc.cases.length) throw new Error(`${CASES} has no cases`)
  const names = new Set()
  for (const c of doc.cases) {
    if (names.has(c.name)) throw new Error(`${CASES}: two cases are named "${c.name}"`)
    names.add(c.name)
    if (typeof c.pass !== 'boolean' || typeof c.text !== 'string' || !c.id || !c.licence) throw new Error(`${CASES}: "${c.name}" is incomplete`)
    if (Boolean(c.pad) !== c.text.includes('{{pad}}')) throw new Error(`${CASES}: "${c.name}": pad and {{pad}} go together`)
    const text = c.pad ? c.text.replace('{{pad}}', c.pad.unit.repeat(c.pad.times)) : c.text
    const got = hasBanner(text, c.id, c.licence)
    if (got !== c.pass) {
      console.error(`FAIL self-test: "${c.name}" (${c.id}, ${c.licence}): got ${got ? 'pass' : 'fail'}, want ${c.pass ? 'pass' : 'fail'}`)
      bad++
    }
  }
  const passing = doc.cases.filter((c) => c.pass).length
  const lines = copyrightLines(
    'MIT License\n\nCopyright (c) 2020 Acme\n * Copyright 2019-2021 Globex\n   Copyright [yyyy] [name of copyright owner]\nthe above copyright notice\nCopyright Holder\n(c) 2018 Initech\n© 2017 Umbrella\n      (c) You must retain, in the Source form\n/*! @license Hooli 1.0 | (c) Hooli and other contributors | Released under MIT */\nCopyright (c) 2020 Acme\n',
  )
  const want = ['Copyright (c) 2020 Acme', 'Copyright 2019-2021 Globex', '(c) 2018 Initech', '© 2017 Umbrella', '(c) Hooli and other contributors']
  if (JSON.stringify(lines) !== JSON.stringify(want)) {
    console.error(`FAIL self-test: copyrightLines gave ${JSON.stringify(lines)}, want ${JSON.stringify(want)}`)
    bad++
  }
  const lb = libraryBanner({ id: 'acme', version: '1.0.0', license: 'MIT', copyright: 'Copyright (c) 2020 Acme' })
  const gb = glueBanner({ file: 'acme-k2.js', id: 'acme', version: '1.0.0', title: 'Acme (a tool)', license: 'BSD-2-Clause', copyright: 'Copyright (c) 2020 Acme' })
  for (const [b, lic] of [[lb, 'MIT'], [gb, 'BSD-2-Clause']]) {
    if (!hasBanner(`${b}\ncode()`, 'acme', lic)) {
      console.error(`FAIL self-test: the banner written is not a banner: ${b}`)
      bad++
    }
  }
  let threw = false
  try {
    libraryBanner({ id: 'acme', version: '1', license: 'MIT', copyright: 'a */ b' })
  } catch {
    threw = true
  }
  if (!threw) {
    console.error('FAIL self-test: a copyright with "*/" was written into a banner')
    bad++
  }
  if (bad) {
    console.error(`zen-lib banners: self-test: ${bad} failure(s)`)
    process.exit(1)
  }
  console.log(`OK: zen-lib banners self-test (${doc.cases.length} cases from ${CASES}: ${passing} pass, ${doc.cases.length - passing} fail as they should; copyright lines; written banners)`)
}

function check() {
  const m = JSON.parse(readFileSync(join(ROOT, MANIFEST), 'utf8'))
  const want = []
  for (const lib of m.libs) {
    if (lib.source !== 'bundled') continue
    for (const f of lib.files) want.push({ lib, name: f.name, key: `${lib.id}@${lib.version}` })
  }
  const problems = []
  // The daemon's table embeds exactly these files, from their paths.
  const rows = [
    ...readFileSync(join(ROOT, TABLE), 'utf8').matchAll(
      /^\s*\("([^"]+)", "([^"]+)", include_bytes!\("\.\.\/\.\.\/\.\.\/src\/renderer\/zen-lib\/([^"]+)"\)\),$/gm,
    ),
  ]
  const have = rows.map((r) => `${r[1]}/${r[2]}`)
  for (const r of rows) {
    if (r[3] !== `${r[1]}/${r[2]}`) problems.push(`${TABLE}: ${r[1]}/${r[2]} is embedded from src/renderer/zen-lib/${r[3]}`)
  }
  const wantKeys = want.map((w) => `${w.key}/${w.name}`)
  if (JSON.stringify(have) !== JSON.stringify(wantKeys)) {
    for (const k of wantKeys.filter((k) => !have.includes(k))) problems.push(`${TABLE} does not embed ${k} (python3 scripts/zen-lib/gen-daemon-libs.py)`)
    for (const k of have.filter((k) => !wantKeys.includes(k))) problems.push(`${TABLE} embeds ${k}, which ${MANIFEST} has no bundled file for`)
    if (!problems.length) problems.push(`${TABLE} lists the files in another order than ${MANIFEST}`)
  }
  // Per served file, never per library: a library's second file is
  // checked like its first. --print groups the files by library.
  let pass = 0
  let lastKey = ''
  for (const w of want) {
    const rel = `src/renderer/zen-lib/${w.key}/${w.name}`
    const text = readFileSync(join(ROOT, rel), 'utf8')
    const problem = bannerProblem(text, w.lib.id, w.lib.license)
    if (args.has('--print')) {
      if (w.key !== lastKey) console.log(`== ${w.key} (${w.lib.license})`)
      lastKey = w.key
      const first = text.split('\n').find((l) => l.trim()) ?? ''
      const banner = bannerOf(text, w.lib.id, w.lib.license)
      console.log(`  ${problem ? 'FAIL' : 'ok  '} ${w.name}: ${first.slice(0, 400)}`)
      if (banner && banner !== first) console.log(`       banner: ${banner.slice(0, 600)}`)
    }
    if (problem) problems.push(`${rel}: ${problem} (${w.lib.id}, ${w.lib.license})`)
    else pass++
  }
  for (const p of problems) console.error(`FAIL ${p}`)
  console.log(`zen-lib banners: ${pass} of ${want.length} served files start with a banner naming their package, licence and copyright`)
  if (problems.length) {
    console.error(`zen-lib banners: ${problems.length} failure(s); run scripts/vendor-zen-lib.sh and commit the output`)
    process.exit(1)
  }
  console.log(`OK: every file ${TABLE} embeds starts with a licence banner`)
}

if (args.has('--self-test')) selfTest()
else check()
