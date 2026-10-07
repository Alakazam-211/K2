// Home M3 — the room boundary ratchet (prd-home-multi-server-client MS2,
// MS14, MS15, MS68).
//
// 1. `useTabsStore` IS the primary room's tabs store. Only the files listed
//    in PRIMARY_TABS_USERS may name it, each a deliberate primary-only
//    owner (the projects store, the Active bar, App's quit save, Settings
//    about the window's server). A new file that names it fails here: room
//    code takes its room's store (`useRoom` / `useRoomTabs`), window-level
//    code the focused room (`focusedRoom`). A file that stops using it must
//    leave the list, so the list never goes stale.
// 2. Room components never reach the window's globals for what their room
//    owns: no `primaryScope()`, and activity is read from `room.activityView`
//    and reported through `room.activity` (MS68), never the window
//    server's activity store directly (prd-daemon-activity-and-thread-
//    working-v1 S5).

import { describe, it, expect } from 'vitest'
import { readFileSync, readdirSync } from 'node:fs'
import { dirname, join, relative, sep } from 'node:path'
import { fileURLToPath } from 'node:url'

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

/** Source with comments removed (same crude rule as scope-boundary). */
function read(f: string): string {
  return readFileSync(join(RENDERER, f), 'utf8')
    .replace(/(^|\s)\/\*[\s\S]*?\*\//g, '$1')
    .replace(/(^|\s)\/\/.*$/gm, '$1')
}

/** file → why it may name the primary room's store directly. */
const PRIMARY_TABS_USERS: Record<string, string> = {
  'stores/tabs.ts': 'defines it: `useTabsStore = createTabsStore(primary binding)`',
  'stores/room.ts': 'the primary room wraps it',
  'stores/projects.ts': "the window's workspace switch drives the primary room",
  'stores/active-agents.ts': "primary-only (MS14): the window's agents map, toasts' View and this computer's daemon events",
  'App.tsx': "quit / unload save and the dirty-dot of the window's own room",
  'components/Sidebar/ActiveBar.tsx': 'window sidebar: the primary room',
  'components/Sidebar/WorktreeDialog.tsx': 'window sidebar: the primary room',
  'components/GitInitDialog/GitInitDialog.tsx': "adds a workspace to the window's server",
  'components/RunningAgentsPanel/RunningAgentsPanel.tsx': "primary-only (MS14): the window's running agents",
  'components/AgentsPanel/AgentsPanel.tsx': 'unmounted legacy panel (primary)',
  'components/Settings/sections/ProjectsSection.tsx': "Settings is about the window's server",
  'components/Settings/sections/HeartbeatsSection.tsx': "Settings is about the window's server",
  'components/Settings/sections/WakeSchedulerSection.tsx': "Settings is about the window's server",
}

/** Room components: everything server-bound comes from their room. */
const ROOM_FILES = [
  'components/AgentPane/AgentChatPane.tsx',
  'components/AgentPane/AgentInboxPane.tsx',
  'components/AgentPane/AgentPane.tsx',
  'components/BrowserPane/BrowserPane.tsx',
  'components/ChangesPanel/ChangesPanel.tsx',
  'components/ChatHistory/ChatHistory.tsx',
  'components/ChatHistory/ContinueNewChatDialog.tsx',
  'components/FileTree/FileTree.tsx',
  'components/FileViewerPane/FileViewerPane.tsx',
  'components/HeartbeatsPanel/HeartbeatEntry.tsx',
  'components/HeartbeatsPanel/HeartbeatsPanel.tsx',
  'components/PaneLayout/PaneGroupView.tsx',
  'components/PaneLayout/PaneLayout.tsx',
  'components/PaneLayout/PaneTabBar.tsx',
  'components/PaneLayout/PinDimensionsModal.tsx',
  'components/PresetsBar/PresetsBar.tsx',
  'components/SessionView/AgentSessionChrome.tsx',
  'components/TabBar/TabBar.tsx',
  'components/Terminal/AlacrittyTerminalView.tsx',
  'components/Terminal/TerminalArea.tsx',
  'components/WorkspacePanel/HideApiSessionsToggle.tsx',
  'components/WorkspacePanel/WorkspacePanel.tsx',
  'hooks/useTerminalShortcuts.ts',
  'kessel-term/TerminalPane.tsx',
]

describe('Home M3 room boundary', () => {
  it('only the listed primary-only owners name useTabsStore', () => {
    const found = SOURCE_FILES.filter((f) => /\buseTabsStore\b/.test(read(f)))
    const unexpected = found.filter((f) => !(f in PRIMARY_TABS_USERS))
    const stale = Object.keys(PRIMARY_TABS_USERS).filter((f) => !found.includes(f))
    expect(unexpected).toEqual([])
    expect(stale).toEqual([])
  })

  it('room components take their scope, tabs and activity sink from the room', () => {
    const offenders: string[] = []
    for (const f of ROOM_FILES) {
      const src = read(f)
      if (/\bprimaryScope\(\)/.test(src)) offenders.push(`${f}: primaryScope()`)
      if (/\buseTabsStore\b/.test(src)) offenders.push(`${f}: useTabsStore`)
      if (/(^|[^.\w])(activityStore|useActivity|setViewing)\(/m.test(src)) {
        offenders.push(`${f}: the window server's activity, not room.activityView / room.activity`)
      }
      if (!/\buseRoom(Tabs|Projects|Supports)?\(|\bisFocusedRoom\(room\)/.test(src)) {
        offenders.push(`${f}: never reads its room`)
      }
    }
    expect(offenders).toEqual([])
  })
})

// Heartbeat S4 (prd-heartbeat-firing-v1 HB28, T-S4c): the drawer reads its
// rows and feature checks through the room's scope. Every file in the
// folder: no `primaryScope()` and no bare (window-global) `serverSupports(`
// — only `room.scope.serverSupports(…)` / `useRoomSupports(…)`.
describe('Heartbeat S4: the drawer folder stays room-scoped', () => {
  it('components/HeartbeatsPanel/ has no primaryScope() and no bare serverSupports(', () => {
    const folder = SOURCE_FILES.filter((f) => f.startsWith('components/HeartbeatsPanel/'))
    expect(folder).toEqual([
      'components/HeartbeatsPanel/HeartbeatEntry.tsx',
      'components/HeartbeatsPanel/HeartbeatsPanel.tsx',
    ])
    const offenders: string[] = []
    for (const f of folder) {
      const src = read(f)
      if (/\bprimaryScope\(\)/.test(src)) offenders.push(`${f}: primaryScope()`)
      if (/(^|[^.\w])serverSupports\(/m.test(src)) offenders.push(`${f}: bare serverSupports(`)
    }
    expect(offenders).toEqual([])
  })
})
