// Home M4 — local-only actions (MS57, vs-live MS67).
//
// 1. Every `invoke('k2so_*' | 'k2_core*' | 'projects_*' | 'fs_*' |
//    'read_worktree_file' | 'format_file')` in the renderer is listed in
//    LOCAL_ONLY_ACTIONS, and every listed command is still invoked
//    somewhere (no stale entries).
// 2. Every such call — and every listener for this computer's daemon
//    events — inside ROOM code is in ROOM_SITES, either gated (the file
//    carries the named gate) or a TODO(M4-integrate) the file marks. A new
//    room-code site fails here until someone decides it.
// 3. The helpers: a room on another server may not run them; a room on
//    this computer may.

import { describe, it, expect } from 'vitest'
import { readFileSync, readdirSync } from 'node:fs'
import { dirname, join, relative, sep } from 'node:path'
import { fileURLToPath } from 'node:url'
import {
  LOCAL_ONLY_ACTIONS,
  isRemoteScope,
  localOnlyAction,
  mayRunLocalActions,
  roomMayRun,
} from './local-only-actions'

const RENDERER = join(dirname(fileURLToPath(import.meta.url)), '..')

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
  .filter((f) => !/\.(ms)?test\.tsx?$/.test(f) && !f.startsWith('test-utils/') && !f.startsWith('shims/'))
  .sort()

function read(f: string): string {
  return readFileSync(join(RENDERER, f), 'utf8')
}

const INVOKE =
  /\binvoke\s*(?:<(?:[^<>]|<[^<>]*>)*>)?\s*\(\s*['"]((?:k2so_|k2_core|projects_|fs_|read_worktree_file|format_file)[A-Za-z0-9_]*)['"]/g
const LOCAL_EVENTS = Object.entries(LOCAL_ONLY_ACTIONS)
  .filter(([, a]) => a.kind === 'event')
  .map(([name]) => name)
const LISTEN = /\blisten\s*(?:<(?:[^<>]|<[^<>]*>)*>)?\s*\(\s*['"]([^'"]+)['"]/g

/** file → local command / event names it uses. */
function sites(files: string[], withEvents: boolean): Map<string, string[]> {
  const out = new Map<string, string[]>()
  for (const f of files) {
    const src = read(f)
    const names = new Set<string>()
    for (const m of src.matchAll(INVOKE)) names.add(m[1])
    if (withEvents) {
      for (const m of src.matchAll(LISTEN)) if (LOCAL_EVENTS.includes(m[1])) names.add(m[1])
    }
    if (names.size > 0) out.set(f, [...names].sort())
  }
  return out
}

/** Room code: everything rendered inside a room (MS2), plus the tabs store. */
const ROOM_DIRS = [
  'components/AgentPane/',
  'components/BrowserPane/',
  'components/ChangesPanel/',
  'components/ChatHistory/',
  'components/FileTree/',
  'components/FileViewerPane/',
  'components/HeartbeatsPanel/',
  'components/PaneLayout/',
  'components/PresetsBar/',
  'components/SessionView/',
  'components/TabBar/',
  'components/Terminal/',
  'components/WorkspacePanel/',
  'kessel-term/',
]
const ROOM_EXTRA = ['hooks/useTerminalShortcuts.ts', 'components/common/HeartbeatSessionPicker.tsx', 'stores/tabs.ts']
const ROOM_FILES = SOURCE_FILES.filter((f) => ROOM_DIRS.some((d) => f.startsWith(d)) || ROOM_EXTRA.includes(f))

type Site = { status: 'gated'; gate: string } | { status: 'todo' }

/** `<file> <name>` → how the room site is handled. `gated`: the file
 *  contains `gate`. `todo`: the file carries a TODO(M4-integrate). */
const ROOM_SITES: Record<string, Site> = {
  'components/AgentPane/AgentChatPane.tsx k2so_chat_refresh_broadcast': { status: 'gated', gate: 'room.localCommands' },
  'components/AgentPane/AgentChatPane.tsx k2so_session_lookup_by_agent': { status: 'gated', gate: 'room.localCommands' },
  'components/AgentPane/AgentChatPane.tsx sync:projects': { status: 'gated', gate: 'room.localCommands' },
  'components/AgentPane/AgentInboxPane.tsx sync:projects': { status: 'gated', gate: 'room.localCommands' },
  // Gated off; routing to that server's /cli is the TODO.
  'components/AgentPane/AgentPane.tsx read_worktree_file': { status: 'gated', gate: 'room.localCommands' },
  'components/AgentPane/AgentPane.tsx k2so_agents_review_queue': { status: 'gated', gate: 'room.localCommands' },
  'components/AgentPane/AgentPane.tsx k2so_agents_build_launch': { status: 'gated', gate: 'room.localCommands' },
  'components/FileTree/FileTree.tsx fs_watch_dir': { status: 'gated', gate: 'mayRunLocalActions(room)' },
  'components/FileTree/FileTree.tsx fs_unwatch_dir': { status: 'gated', gate: 'mayRunLocalActions(room)' },
  'components/FileTree/FileTree.tsx fs://change': { status: 'gated', gate: 'mayRunLocalActions(room)' },
  'components/FileViewerPane/CodeEditor.tsx format_file': { status: 'gated', gate: 'localFormatRef.current' },
  'components/TabBar/TabBar.tsx projects_open_in_terminal': { status: 'gated', gate: 'room.localCommands' },
  'components/WorkspacePanel/WorkspacePanel.tsx k2so_inbox_count': { status: 'gated', gate: 'room.localCommands' },
  'components/WorkspacePanel/WorkspacePanel.tsx projects_open_in_finder': { status: 'gated', gate: 'room.localCommands' },
  'components/WorkspacePanel/WorkspacePanel.tsx sync:projects': { status: 'gated', gate: 'room.localCommands' },
  'components/common/HeartbeatSessionPicker.tsx k2so_agents_list': { status: 'gated', gate: 'room.room.localCommands' },
  'hooks/useTerminalShortcuts.ts projects_open_focus_window': { status: 'gated', gate: 'room.localCommands' },
  'stores/tabs.ts k2so_agents_list': { status: 'gated', gate: 'binding.localCommands' },
}

describe('local-only actions catalog (MS67)', () => {
  const all = sites(SOURCE_FILES, false)
  const used = new Set([...all.values()].flat())

  it('every local Tauri command the renderer invokes is listed', () => {
    const missing = [...used].filter((n) => !(n in LOCAL_ONLY_ACTIONS)).sort()
    expect(missing).toEqual([])
  })

  it('every listed command is still invoked somewhere (no stale entries)', () => {
    const stale = Object.entries(LOCAL_ONLY_ACTIONS)
      .filter(([name, a]) => a.kind === 'command' && !used.has(name))
      .map(([name]) => name)
    expect(stale).toEqual([])
  })

  it('every routed entry names its /cli route', () => {
    const bad = Object.entries(LOCAL_ONLY_ACTIONS)
      .filter(([, a]) => (a.inRemoteRoom === 'route') !== (typeof a.route === 'string' && a.route.length > 0))
      .map(([n]) => n)
    expect(bad).toEqual([])
  })

  it('the two must-fix commands are off in a remote room', () => {
    expect(localOnlyAction('k2so_session_lookup_by_agent').inRemoteRoom).toBe('off')
    expect(localOnlyAction('k2so_chat_refresh_broadcast').inRemoteRoom).toBe('off')
  })
})

describe('room-code sites are each decided (MS67)', () => {
  const found = sites(ROOM_FILES, true)
  const keys = [...found.entries()].flatMap(([f, names]) => names.map((n) => `${f} ${n}`)).sort()

  it('every local call or local-event listener in room code is in ROOM_SITES', () => {
    expect(keys.filter((k) => !(k in ROOM_SITES))).toEqual([])
  })

  it('ROOM_SITES has no stale entries', () => {
    expect(Object.keys(ROOM_SITES).filter((k) => !keys.includes(k))).toEqual([])
  })

  it('a gated site’s file carries its gate; a todo site carries TODO(M4-integrate)', () => {
    const bad: string[] = []
    for (const [key, site] of Object.entries(ROOM_SITES)) {
      const file = key.slice(0, key.lastIndexOf(' '))
      const src = read(file)
      if (site.status === 'gated' && !src.includes(site.gate)) bad.push(`${key}: missing gate ${site.gate}`)
      if (site.status === 'todo' && !src.includes('TODO(M4-integrate)')) bad.push(`${key}: missing TODO(M4-integrate)`)
    }
    expect(bad).toEqual([])
  })
})

describe('who may run them', () => {
  const local = { isRemote: false }
  const remote = { isRemote: true }

  it('isRemoteScope is the scope’s own answer', () => {
    expect(isRemoteScope(local)).toBe(false)
    expect(isRemoteScope(remote)).toBe(true)
  })

  it('a pinned room on another server: no; on this computer: yes', () => {
    expect(mayRunLocalActions({ localCommands: false, scope: remote })).toBe(false)
    expect(mayRunLocalActions({ localCommands: true, scope: local })).toBe(true)
  })

  it('both the room flag and the scope must agree', () => {
    // The primary room on a remote window: M3 keeps localCommands true; the
    // scope still says remote, so a mayRunLocalActions site turns it off.
    expect(mayRunLocalActions({ localCommands: true, scope: remote })).toBe(false)
    expect(mayRunLocalActions({ localCommands: false, scope: local })).toBe(false)
  })

  it('roomMayRun refuses an unlisted name loudly', () => {
    expect(roomMayRun({ localCommands: true, scope: local }, 'projects_open_in_finder')).toBe(true)
    expect(roomMayRun({ localCommands: false, scope: remote }, 'format_file')).toBe(false)
    expect(() => roomMayRun({ localCommands: true, scope: local }, 'k2so_made_up')).toThrow(/not in LOCAL_ONLY_ACTIONS/)
  })
})
