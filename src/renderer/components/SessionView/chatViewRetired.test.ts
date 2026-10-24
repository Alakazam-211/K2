// @vitest-environment jsdom
// Chat view retirement ratchet (0.46.4, Rosson 2026-10-23: "we're sticking
// with threads"). The agent-pane Chat view (the agent's JSONL transcript
// drawn over its PTY, LIVE 0.41.0) lost every UI entry point in 0.46.4; its
// code stays, reachable only from tests, until 0.46.5 deletes it. See wiki
// `Plan - Retirement ledger`.
//
// 1. No menu row, setting option or call in renderer source picks the Chat
//    view: no `id: 'chat'` / `value: 'chat'` row, no setter called with
//    `'chat'`, no "Chat (beta)" label, no `chatEligible` gate.
// 2. The view menus offer no Chat row (behaviour, not just source).
// 3. Only the retained chain may import the Chat view modules: TerminalPane
//    (its two mounts, unreachable outside tests) → ChatOverlayColumn →
//    ChatOverlayPane → useChatTranscript → chatTranscript. A new importer
//    fails here. Each retained file names the 0.46.5 removal.
// (The Settings search text is checked in NewAgentViewRow.test.tsx.)
//
// 0.46.5: when the Chat view code is deleted, delete this file too.

import { describe, expect, it, vi } from 'vitest'
import { readFileSync, readdirSync } from 'node:fs'
import { dirname, join, relative, sep } from 'node:path'
import { fileURLToPath } from 'node:url'

// SessionViewMenu's store imports reach Tauri at load; no Tauri here.
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => {}), listen: vi.fn(async () => () => {}) }))

import { sideMenuItems, viewMenuItems } from './SessionViewMenu'

const RENDERER = join(dirname(fileURLToPath(import.meta.url)), '../..')

function walk(dir: string, out: string[]): string[] {
  for (const e of readdirSync(dir, { withFileTypes: true })) {
    const p = join(dir, e.name)
    if (e.isDirectory()) walk(p, out)
    else if (/\.tsx?$/.test(e.name)) out.push(p)
  }
  return out
}

const SOURCE_FILES = walk(RENDERER, [])
  .map((f) => relative(RENDERER, f).split(sep).join('/'))
  .filter((f) => !/\.(ms)?test\.tsx?$/.test(f) && !f.startsWith('test-utils/'))
  .sort()

function raw(f: string): string {
  return readFileSync(join(RENDERER, f), 'utf8')
}

/** Source with comments removed (same crude rule as room-boundary). */
function code(f: string): string {
  return raw(f)
    .replace(/(^|\s)\/\*[\s\S]*?\*\//g, '$1')
    .replace(/(^|\s)\/\/.*$/gm, '$1')
}

/** Ways renderer code could still pick the Chat view. */
const ENTRY_POINT_PATTERNS: Array<[string, RegExp]> = [
  ['a menu / option row for chat', /\b(id|value)\s*:\s*['"]chat['"]/],
  ['a view setter called with chat', /\b(setViewTab|onViewTabChange|onChange|setSplitLeft|setSplitRight|onSplitLeft|onSplitRight)\(\s*['"]chat['"]/],
  ['the "Chat (beta)" label', /Chat \(beta\)/],
  ['the chatEligible gate (Chat row enable)', /\bchatEligible\b/],
  ['a "Show chat view" control', /show chat view/i],
]

/** The retained Chat view chain: file → which Chat view module it may import. */
const CHAT_VIEW_MODULES = ['ChatOverlayColumn', 'ChatOverlayPane', 'useChatTranscript', 'chatTranscript'] as const
const ALLOWED_IMPORTERS: Record<string, string> = {
  'kessel-term/TerminalPane.tsx': 'ChatOverlayColumn',
  'components/SessionView/ChatOverlayColumn.tsx': 'ChatOverlayPane',
  'components/SessionView/ChatOverlayPane.tsx': 'chatTranscript,useChatTranscript',
  'components/SessionView/useChatTranscript.ts': 'chatTranscript',
}

function chatViewImports(f: string): string[] {
  const found = new Set<string>()
  const re = /from\s+['"]([^'"]+)['"]/g
  for (const m of code(f).matchAll(re)) {
    const last = m[1].split('/').pop() ?? ''
    if ((CHAT_VIEW_MODULES as readonly string[]).includes(last)) found.add(last)
  }
  return [...found].sort()
}

describe('Chat view retired from the UI (0.46.4)', () => {
  it('walks the renderer source', () => {
    expect(SOURCE_FILES.length).toBeGreaterThan(200)
    expect(SOURCE_FILES).toContain('components/SessionView/SessionViewMenu.tsx')
    expect(SOURCE_FILES).toContain('components/Settings/sections/GeneralSection.tsx')
  })

  it('no renderer source has a Chat view entry point', () => {
    const hits: string[] = []
    for (const f of SOURCE_FILES) {
      const src = code(f)
      for (const [what, re] of ENTRY_POINT_PATTERNS) {
        if (re.test(src)) hits.push(`${f}: ${what}`)
      }
    }
    expect(hits).toEqual([])
  })

  it('the view menu and the Split side menus have no Chat row', () => {
    const view = viewMenuItems({ muted: false, canMute: true }).map((i) => i.id)
    expect(view).toEqual(['terminal', 'thread', 'chatter', 'split', 'separator-mute', 'mute-thread'])
    expect(sideMenuItems().map((i) => i.id)).toEqual(['terminal', 'thread', 'chatter'])
    const labels = [...viewMenuItems(), ...sideMenuItems()].map((i) => i.label.toLowerCase())
    expect(labels.filter((l) => l.startsWith('chat') && l !== 'chatter')).toEqual([])
  })

  it('only the retained chain imports the Chat view modules, each marked for 0.46.5', () => {
    const importers: Record<string, string> = {}
    for (const f of SOURCE_FILES) {
      const mods = chatViewImports(f)
      if (mods.length > 0) importers[f] = mods.join(',')
    }
    expect(importers).toEqual(ALLOWED_IMPORTERS)
    const retained = [
      ...Object.keys(ALLOWED_IMPORTERS),
      'components/SessionView/chatTranscript.ts',
      'components/SessionView/sessionViewTab.ts',
      'components/SessionView/SessionViewMenu.tsx',
    ]
    const unmarked = retained.filter((f) => !raw(f).includes('0.46.5'))
    expect(unmarked).toEqual([])
  })
})
