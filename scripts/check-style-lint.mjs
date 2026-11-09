#!/usr/bin/env node
// Style-contract lint ratchet.
//
//   node scripts/check-style-lint.mjs                    # check against baseline
//   node scripts/check-style-lint.mjs --update-baseline  # rewrite the baseline after cleanup
//
// Scans renderer components for values that bypass the Style contract, in two
// categories counted separately:
//   color: hex literals (#abc / #aabbcc) and Tailwind NAMED color utilities
//          (text-red-400, bg-white/5, ...)
//   shape: Tailwind rounded/shadow/backdrop-blur utilities that skip the
//          --radius-* and shadow tokens, and inline borderRadius, boxShadow,
//          backdropFilter and clip-path literals
//
// Comments are removed first, so issue references (// task #617) aren't colors.
//
// Per-file counts are compared to styles/lint-baseline.json ({ color, shape }).
// A count above baseline fails (new debt); below baseline prints a hint to
// re-run with --update-baseline so the ratchet only ever tightens. Files with
// known palette-independent color data (identity/brand maps, CM themes) are
// listed in COLOR_EXEMPT rather than the baseline. Gardens (components/Zen/)
// own their look, so neither category applies there.

import { readFileSync, writeFileSync, readdirSync, statSync } from 'node:fs'
import { join, relative, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
import ts from 'typescript'

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..')
const SCAN_DIRS = ['src/renderer/components', 'src/renderer/App.tsx', 'src/renderer/contexts', 'src/renderer/hooks']
const BASELINE_FILE = join(ROOT, 'styles', 'lint-baseline.json')

// Gardens handle their own theming — neither category applies under here:
const EXEMPT_PREFIX = 'src/renderer/components/Zen/'

// Palette-independent by design — colors here are never counted:
const COLOR_EXEMPT = [
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

// Tailwind utility boundaries, shared with NAMED_UTIL_RE: a variant or class
// separator before, and a separator (or the v4 trailing !) after.
const PRE = '(?:^|[\\s"\'`:!])'
const POST = '(?=[\\s"\'`}!]|$)'
const CORNER = '(?:-(?:t|r|b|l|s|e|ss|se|ee|es|tl|tr|br|bl))?'
const ROUNDED_RE = new RegExp(`${PRE}rounded${CORNER}(?:-(?:sm|md|lg|xl|2xl|3xl|full)|-\\[(?![^\\]]*var\\(--radius-)[^\\]]*\\])?${POST}`, 'g')
const SHADOW_RE = new RegExp(`${PRE}shadow-(?:sm|md|lg|xl|2xl)${POST}`, 'g')
const BLUR_RE = new RegExp(`${PRE}backdrop-blur(?:-[\\w.%/()\\[\\]-]+)?${POST}`, 'g')
const INLINE_SHAPE_RE = /\b(?:borderRadius|boxShadow|backdropFilter|WebkitBackdropFilter|clipPath)\s*[:=]\s*(\d[\w.%]*|'[^'\n]*'|"[^"\n]*"|`[^`]*`)/g
const CLIP_PATH_RE = /\bclip-path\s*:\s*([^;"'`}\]]+)/g

// A literal has a digit outside var(): '8px' and '0 1px 2px var(--c)' count;
// 'var(--radius-box)', 'blur(var(--material-blur))' and 'none' don't. A zero
// radius is the inline form of rounded-none, and url(#id) is an SVG reference.
function isShapeLiteral(raw) {
  const value = raw.replace(/^['"`]|['"`]$/g, '').trim()
  if (/^0(?:px)?$/.test(value) || value.startsWith('url(')) return false
  return /\d/.test(value.replace(/var\((?:[^()]|\([^()]*\))*\)/g, ''))
}

// Blank every comment. TS sources go through the real parser, so http:// in a
// string, // in JSX text and a quote inside a regex can't confuse it.
function stripComments(src, path) {
  if (path.endsWith('.css')) return src.replace(/\/\*[\s\S]*?\*\//g, ' ')
  if (!src.includes('//') && !src.includes('/*')) return src
  const kind = path.endsWith('.tsx') ? ts.ScriptKind.TSX : ts.ScriptKind.TS
  const file = ts.createSourceFile(path, src, ts.ScriptTarget.Latest, false, kind)
  const trivia = []
  const visit = (node) => {
    if (ts.isJSDoc(node)) return
    const kids = node.getChildren(file)
    if (kids.length > 0) {
      kids.forEach(visit)
      return
    }
    // A leaf token: everything between its pos and start is whitespace/comments.
    const start = node.getStart(file)
    if (start > node.pos) trivia.push([node.pos, start])
  }
  visit(file)
  let out = ''
  let at = 0
  for (const [from, to] of trivia) {
    out += src.slice(at, from) + ' '
    at = to
  }
  return out + src.slice(at)
}

function countColor(code) {
  return (code.match(HEX_RE)?.length ?? 0) + (code.match(NAMED_UTIL_RE)?.length ?? 0)
}

function countShape(code) {
  const utilities = [ROUNDED_RE, SHADOW_RE, BLUR_RE].reduce((n, re) => n + (code.match(re)?.length ?? 0), 0)
  const literals = [INLINE_SHAPE_RE, CLIP_PATH_RE].reduce((n, re) => n + [...code.matchAll(re)].filter((m) => isShapeLiteral(m[1])).length, 0)
  return utilities + literals
}

const counts = { color: {}, shape: {} }
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
    if (rel.startsWith(EXEMPT_PREFIX)) continue
    const code = stripComments(readFileSync(f, 'utf8'), f)
    const color = COLOR_EXEMPT.includes(rel) ? 0 : countColor(code)
    const shape = countShape(code)
    if (color > 0) counts.color[rel] = color
    if (shape > 0) counts.shape[rel] = shape
  }
}

const summarize = (map) => `${Object.keys(map).length} files, ${Object.values(map).reduce((a, b) => a + b, 0)} occurrences`

if (process.argv.includes('--update-baseline')) {
  writeFileSync(BASELINE_FILE, JSON.stringify(counts, null, 2) + '\n')
  console.log(`style-lint: baseline updated — color: ${summarize(counts.color)}; shape: ${summarize(counts.shape)}`)
  process.exit(0)
}

let baseline
try {
  baseline = JSON.parse(readFileSync(BASELINE_FILE, 'utf8'))
} catch {
  console.error('style-lint: ERROR: styles/lint-baseline.json missing — run with --update-baseline first')
  process.exit(1)
}
if (!baseline.color || !baseline.shape) {
  console.error('style-lint: ERROR: styles/lint-baseline.json needs "color" and "shape" maps — run with --update-baseline')
  process.exit(1)
}

const HINTS = {
  color: 'use contract slots (var(--color-*)) instead',
  shape: 'use contract slots (var(--radius-*), var(--shadow-*), var(--material-*)) instead',
}

let shrunk = false
const failedCategories = new Set()
for (const category of ['color', 'shape']) {
  for (const [file, n] of Object.entries(counts[category])) {
    const allowed = baseline[category][file] ?? 0
    if (n > allowed) {
      console.error(`style-lint: FAIL [${category}] ${file}: ${n} raw ${category} usages (baseline ${allowed}) — ${HINTS[category]}`)
      failedCategories.add(category)
    } else if (n < allowed) {
      shrunk = true
    }
  }
  for (const file of Object.keys(baseline[category])) {
    if (!(file in counts[category])) shrunk = true
  }
}

if (failedCategories.size > 0) {
  console.error(`style-lint: FAILED in: ${[...failedCategories].join(', ')}`)
  process.exit(1)
}
if (shrunk) console.log('style-lint: PASS (some files improved — run --update-baseline to ratchet)')
else console.log('style-lint: PASS')
