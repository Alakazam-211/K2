#!/usr/bin/env node
// Style-contract lint ratchet.
//
//   node scripts/check-style-lint.mjs                    # check against baseline
//   node scripts/check-style-lint.mjs --update-baseline  # shrink the baseline after cleanup
//
// Scans renderer components for color values that bypass the Style contract:
//   1. hex literals (#abc / #aabbcc) in class names or inline styles
//   2. Tailwind NAMED color utilities (text-red-400, bg-white/5, …)
//
// Per-file counts are compared to styles/lint-baseline.json. A count above
// baseline fails (new debt); below baseline prints a hint to re-run with
// --update-baseline so the ratchet only ever tightens. Files with known
// palette-independent color data (identity/brand maps, CM themes) are listed
// in EXEMPT below rather than the baseline, so their counts never ratchet.

import { readFileSync, writeFileSync, readdirSync, statSync } from 'node:fs'
import { join, relative, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..')
const SCAN_DIRS = ['src/renderer/components', 'src/renderer/App.tsx', 'src/renderer/contexts', 'src/renderer/hooks']
const BASELINE_FILE = join(ROOT, 'styles', 'lint-baseline.json')

// Palette-independent by design — never counted:
const EXEMPT = [
  'src/renderer/components/CodeEditor/CodeEditor.tsx', // CM editor-theme defs (independent system)
  'src/renderer/components/FileTree/FileTree.tsx', // language brand-color map
  'src/renderer/components/Sidebar/SectionItem.tsx', // SECTION_COLORS user picker
  'src/renderer/components/Presence/PresenceAvatar.tsx', // role identity colors
  'src/renderer/components/Projects/ProjectGroupAvatar.tsx', // GROUP_AVATAR_COLORS
  'src/renderer/components/AgentPane/AgentIcon.tsx', // agent brand colors
]

const HEX_RE = /#[0-9a-fA-F]{6}\b|#[0-9a-fA-F]{3}\b/g
const NAMED_UTIL_RE =
  /(?:^|[\s"'`:!])(?:text|bg|border|ring|fill|stroke|divide|outline|shadow|decoration|placeholder|accent|caret|from|via|to)-(?:red|orange|amber|yellow|lime|green|emerald|teal|cyan|sky|blue|indigo|violet|purple|fuchsia|pink|rose|slate|gray|zinc|neutral|stone|white|black)(?:-\d{2,3})?(?:\/\[?[\d.]+\]?)?(?=[\s"'`}]|$)/g

function* walk(dir) {
  for (const entry of readdirSync(dir)) {
    const full = join(dir, entry)
    if (statSync(full).isDirectory()) yield* walk(full)
    else if (/\.(tsx?|css)$/.test(entry) && !/\.test\./.test(entry)) yield full
  }
}

function countFile(path) {
  const src = readFileSync(path, 'utf8')
  const hex = src.match(HEX_RE)?.length ?? 0
  const named = src.match(NAMED_UTIL_RE)?.length ?? 0
  return hex + named
}

const counts = {}
for (const target of SCAN_DIRS) {
  const full = join(ROOT, target)
  let files
  try {
    files = statSync(full).isDirectory() ? [...walk(full)] : [full]
  } catch {
    continue
  }
  for (const f of files) {
    const rel = relative(ROOT, f)
    if (EXEMPT.includes(rel)) continue
    const n = countFile(f)
    if (n > 0) counts[rel] = n
  }
}

if (process.argv.includes('--update-baseline')) {
  writeFileSync(BASELINE_FILE, JSON.stringify(counts, null, 2) + '\n')
  const total = Object.values(counts).reduce((a, b) => a + b, 0)
  console.log(`style-lint: baseline updated — ${Object.keys(counts).length} files, ${total} tolerated occurrences`)
  process.exit(0)
}

let baseline = {}
try {
  baseline = JSON.parse(readFileSync(BASELINE_FILE, 'utf8'))
} catch {
  console.error('style-lint: ERROR: styles/lint-baseline.json missing — run with --update-baseline first')
  process.exit(1)
}

let failed = false
let shrunk = false
for (const [file, n] of Object.entries(counts)) {
  const allowed = baseline[file] ?? 0
  if (n > allowed) {
    console.error(`style-lint: FAIL ${file}: ${n} raw color usages (baseline ${allowed}) — use contract slots (var(--color-*)) instead`)
    failed = true
  } else if (n < allowed) {
    shrunk = true
  }
}
for (const file of Object.keys(baseline)) {
  if (!(file in counts)) shrunk = true
}

if (failed) process.exit(1)
if (shrunk) console.log('style-lint: PASS (some files improved — run --update-baseline to ratchet)')
else console.log('style-lint: PASS')
