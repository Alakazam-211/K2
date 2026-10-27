// @vitest-environment jsdom
// Chat view deleted (0.46.5). The agent-pane Chat view (the agent's JSONL
// transcript drawn over its PTY, LIVE 0.41.0) lost its UI in 0.46.4 and its
// code in 0.46.5 (Rosson 2026-10-23: "we're sticking with threads";
// 2026-10-27: "Green light on Chat View removal and code deletion. Anyone
// using it as a default point them at threads."). See wiki
// `Plan - Retirement ledger`.
//
// This ratchet proves the code is gone and stays gone:
// 1. The Chat view's files are deleted (and their tests).
// 2. No renderer source names a Chat view module, helper or flag, or the
//    daemon's deleted `/cli/chat/transcript` socket.
// 3. No source picks the Chat view: no menu/option row, setter call or label.
// 4. `chat` is not a member of `SessionViewTab` / `SplitPaneView`; the only
//    `'chat'` view literal left is the one saved-state migration constant.
// 5. No "0.46.5: delete" retention marker is left behind.
// 6. The view menus offer no Chat row (behaviour, not just source).
// 7. `chatHarness.ts` stays: the Token picker uses it.
// The migration to Threads itself is tested in sessionViewTab.test.ts and
// useSessionViewTab.test.tsx.

import { describe, expect, it, vi } from 'vitest'
import { existsSync, readFileSync, readdirSync } from 'node:fs'
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

const ALL_FILES = walk(RENDERER, [])
  .map((f) => relative(RENDERER, f).split(sep).join('/'))
  .sort()

const SOURCE_FILES = ALL_FILES.filter(
  (f) => !/\.(ms)?test\.tsx?$/.test(f) && !f.startsWith('test-utils/'),
)

/** This file names the gone identifiers on purpose. */
const SELF = 'components/SessionView/chatViewGone.test.ts'

function raw(f: string): string {
  return readFileSync(join(RENDERER, f), 'utf8')
}

/** Source with comments removed (same crude rule as room-boundary). */
function code(f: string): string {
  return raw(f)
    .replace(/(^|\s)\/\*[\s\S]*?\*\//g, '$1')
    .replace(/(^|\s)\/\/.*$/gm, '$1')
}

const DELETED_FILES = [
  'components/SessionView/ChatOverlayColumn.tsx',
  'components/SessionView/ChatOverlayColumn.test.tsx',
  'components/SessionView/ChatOverlayPane.tsx',
  'components/SessionView/ChatOverlayPane.test.tsx',
  'components/SessionView/useChatTranscript.ts',
  'components/SessionView/useChatTranscript.test.tsx',
  'components/SessionView/chatTranscript.ts',
  'components/SessionView/chatTranscript.test.ts',
  'components/SessionView/chatViewRetired.test.ts',
]

/** Identifiers of the deleted Chat view, in code or tests. */
const GONE_IDENTIFIERS: Array<[string, RegExp]> = [
  ['ChatOverlayColumn', /\bChatOverlayColumn\b/],
  ['ChatOverlayPane', /\bChatOverlayPane\b/],
  ['useChatTranscript', /\buseChatTranscript\b/],
  ['chatTranscript module', /\bchatTranscript\b/],
  ['retireChatTab', /\bretireChatTab\b/],
  ['retireChatSides', /\bretireChatSides\b/],
  ['savedViewNamesChat', /\bsavedViewNamesChat\b/],
  ['chatFallbackSide', /\bchatFallbackSide\b/],
  ['chatFaceOn', /\bchatFaceOn\b/],
  ['chatConversationId', /\bchatConversationId\b/],
  ['the overlayViewer chat flag', /\bviewer\.chat\b|\bchat:\s*showChat\b/],
  ['the deleted /cli/chat/transcript socket', /chat\/transcript/],
  ['the Chat view test ids', /chat-overlay-(column|pane|list-slot|compose-slot)/],
]

/** Ways renderer code could pick the Chat view. */
const ENTRY_POINT_PATTERNS: Array<[string, RegExp]> = [
  ['a menu / option row for chat', /\b(id|value)\s*:\s*['"]chat['"]/],
  ['a view setter called with chat', /\b(setViewTab|onViewTabChange|onChange|setSplitLeft|setSplitRight|onSplitLeft|onSplitRight)\(\s*['"]chat['"]/],
  ['the "Chat (beta)" label', /Chat \(beta\)/],
  ['the chatEligible gate', /\bchatEligible\b/],
  ['a "Show chat view" control', /show chat view/i],
]

describe('Chat view code is gone (0.46.5)', () => {
  it('walks the renderer source', () => {
    expect(SOURCE_FILES.length).toBeGreaterThan(200)
    expect(SOURCE_FILES).toContain('components/SessionView/SessionViewMenu.tsx')
    expect(SOURCE_FILES).toContain('kessel-term/TerminalPane.tsx')
    expect(ALL_FILES).toContain(SELF)
  })

  it('the Chat view files and their tests are deleted', () => {
    const left = DELETED_FILES.filter((f) => existsSync(join(RENDERER, f)))
    expect(left).toEqual([])
  })

  it('no renderer file (source or test) names a Chat view module, helper, flag or the deleted socket', () => {
    const hits: string[] = []
    for (const f of ALL_FILES) {
      if (f === SELF) continue
      const src = code(f)
      for (const [what, re] of GONE_IDENTIFIERS) {
        if (re.test(src)) hits.push(`${f}: ${what}`)
      }
    }
    expect(hits).toEqual([])
    // TerminalPane's Chat mounts and their flag. (DesignPane's own `showChat`
    // is its design-chat column, not the Chat view.)
    expect(code('kessel-term/TerminalPane.tsx')).not.toMatch(/\bshowChat\b/)
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

  it("`chat` is not a view type member; the only 'chat' literal is the saved-state migration constant", () => {
    const src = code('components/SessionView/sessionViewTab.ts')
    const tabType = /export type SessionViewTab\s*=([^\n]+)/.exec(src)?.[1]
    const sideType = /export type SplitPaneView\s*=([^\n]+)/.exec(src)?.[1]
    if (!tabType || !sideType) throw new Error('view type declarations not found in sessionViewTab.ts')
    expect(tabType.trim()).toBe("'terminal' | 'thread' | 'chatter' | 'split'")
    expect(sideType.trim()).toBe("'terminal' | 'thread' | 'chatter'")
    expect(src.match(/['"]chat['"]/g)).toEqual(["'chat'"])
    expect(src).toMatch(/const LEGACY_CHAT = 'chat'/)

    // Nowhere else in renderer source is `'chat'` a session view.
    const viewLiteral = /\b(viewTab|splitLeft|splitRight|view|tab)\s*===?\s*['"]chat['"]/
    const hits = SOURCE_FILES.filter((f) => viewLiteral.test(code(f)))
    expect(hits).toEqual([])
  })

  it('no "0.46.5: delete" retention marker is left in renderer source', () => {
    const marker = /0\.46\.5\s*:\s*(delete|drop)/i
    const hits = SOURCE_FILES.filter((f) => marker.test(raw(f)))
    expect(hits).toEqual([])
  })

  it('the view menu and the Split side menus have no Chat row', () => {
    const view = viewMenuItems({ muted: false, canMute: true }).map((i) => i.id)
    expect(view).toEqual(['terminal', 'thread', 'chatter', 'split', 'separator-mute', 'mute-thread'])
    expect(sideMenuItems().map((i) => i.id)).toEqual(['terminal', 'thread', 'chatter'])
    const labels = [...viewMenuItems(), ...sideMenuItems()].map((i) => i.label.toLowerCase())
    expect(labels.filter((l) => l.startsWith('chat') && l !== 'chatter')).toEqual([])
  })

  it('keeps chatHarness.ts for the Token picker', () => {
    expect(existsSync(join(RENDERER, 'components/SessionView/chatHarness.ts'))).toBe(true)
    expect(code('components/SessionView/AgentSessionChrome.tsx')).toMatch(/from '\.\/chatHarness'/)
  })
})
