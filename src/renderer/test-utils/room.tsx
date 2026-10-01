// Room test helpers (Home M3). Room components throw without a
// `RoomProvider` (MS2), so component tests hand them a room explicitly:
//
//   - `renderInPrimaryRoom(ui)` (test-utils/primary-room.tsx) — the real
//     primary room (`useTabsStore`, the projects store, the active-agents
//     sink, `primaryScope()`). For suites that run the real stores.
//   - `testRoom({...})` — a room built from whatever the suite mocks: its
//     tabs store (a mocked `useTabsStore` is fine), a project list, an
//     activity sink, a scope. Every field the suite does not pass is a stub
//     that THROWS when used, so a test never silently reaches a store it did
//     not set up.

import type { ReactElement, ReactNode } from 'react'
import { render, type RenderOptions, type RenderResult } from '@testing-library/react'
import { createStore } from 'zustand/vanilla'
import { RoomProvider } from '@/components/Room/RoomContext'
// Type-only: importing `@/stores/room` at runtime would load the real tabs,
// projects and active-agents stores into suites that mock them.
import type { Room, RoomActivitySink, RoomProjectsStore } from '@/stores/room'
import { primaryScope, type ServerScope } from '@/kessel/server-scope'
import type { TabsStore } from '@/stores/tabs'
import type { ProjectWithWorkspaces } from '@/stores/projects'

/** Render `ui` inside `room`. The provider is RTL's `wrapper`, so
 *  `rerender(ui)` keeps the same room. */
export function renderInRoom(room: Room, ui: ReactElement, options?: RenderOptions): RenderResult {
  const Outer = options?.wrapper
  const Wrapper = ({ children }: { children: ReactNode }): React.JSX.Element => (
    <RoomProvider room={room} shown>
      {Outer ? <Outer>{children}</Outer> : children}
    </RoomProvider>
  )
  return render(ui, { ...options, wrapper: Wrapper })
}

/** Wrap `ui` for a `rerender` call. */
export function inRoom(room: Room, ui: ReactElement): ReactElement {
  return (
    <RoomProvider room={room} shown>
      {ui}
    </RoomProvider>
  )
}

/** A read-only project-list store for a test room. */
export function projectsStoreOf(projects: ProjectWithWorkspaces[] | (() => ProjectWithWorkspaces[])): RoomProjectsStore {
  const read = typeof projects === 'function' ? projects : () => projects
  const store = createStore<{ projects: ProjectWithWorkspaces[] }>(() => ({ projects: read() }))
  if (typeof projects === 'function') {
    // Live: re-read on every getState so a suite that mutates its fixture
    // between renders is seen.
    return {
      getState: () => ({ projects: read() }),
      getInitialState: () => ({ projects: read() }),
      subscribe: store.subscribe,
    }
  }
  return store
}

const unset = (what: string) => (): never => {
  throw new Error(`testRoom: ${what} was used but the test did not provide it`)
}

/** An activity sink whose every method records nothing and fails loudly
 *  unless the suite passes its own. */
function throwingActivity(): RoomActivitySink {
  return {
    recordOutput: unset('activity.recordOutput'),
    recordTitleActivity: unset('activity.recordTitleActivity'),
    recordTitlePermission: unset('activity.recordTitlePermission'),
    markSeen: unset('activity.markSeen'),
    bindPaneAgentName: unset('activity.bindPaneAgentName'),
    bindPaneProject: unset('activity.bindPaneProject'),
  }
}

export function testRoom(opts: {
  tabs: unknown
  /** A fixture list, a live getter, or a project store with `getState` /
   *  `subscribe` (e.g. a suite's own zustand store). */
  projects?: ProjectWithWorkspaces[] | (() => ProjectWithWorkspaces[]) | RoomProjectsStore
  activity?: Partial<RoomActivitySink>
  scope?: ServerScope
  isPrimary?: boolean
  localCommands?: boolean
  key?: string
  cwd?: string
  activeProjectId?: string | null
}): Room {
  const scope = opts.scope ?? primaryScope()
  return {
    key: opts.key ?? 'test-room',
    isPrimary: opts.isPrimary ?? true,
    scope,
    tabs: opts.tabs as TabsStore,
    projects: opts.projects
      ? ('getState' in opts.projects
          ? (opts.projects as RoomProjectsStore)
          : projectsStoreOf(opts.projects as ProjectWithWorkspaces[] | (() => ProjectWithWorkspaces[])))
      : ({
      getState: unset('projects'),
      getInitialState: unset('projects'),
      subscribe: unset('projects'),
    } as unknown as RoomProjectsStore),
    activity: { ...throwingActivity(), ...opts.activity },
    localCommands: opts.localCommands ?? true,
    activeProjectId: () => opts.activeProjectId ?? null,
    roomId: () => null,
    cwd: opts.cwd ? () => opts.cwd as string : unset('cwd'),
  }
}
