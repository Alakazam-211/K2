// Home M4 — remote rooms in Home's main area (prd-home-v1 R8/R9; MS24,
// MS45, MS55).
//
// Each open remote room mounts the SAME room components the Agents page
// mounts (the tab strip, panes, pinned Chat, Inbox, Thread, Browser, file
// tabs, terminals), under its own `RoomProvider`, wrapped in the failure
// gate for its server. Home M5: a room on a server with the layout revision
// check is usable; an older server's room stays view only, with a note.
// Hot rooms stay mounted (shown or hidden); warm and
// cold rooms are not rendered (their grids close; `stores/home-rooms.ts`
// keeps or drops the tabs store). The window's server never changes.
//
// 0.43.2 (prd-home-seamless-0432 Z8): these rooms render outside the keyed
// App, through `HomeRoomsHost.tsx`'s portal, so a top-switcher change keeps
// them mounted. The Agents shell only holds the slot they appear in.

import { useEffect, useMemo } from 'react'
import { useStore } from 'zustand'
import { RoomProvider } from '@/components/Room/RoomContext'
import { TerminalArea } from '@/components/Terminal/TerminalArea'
import { LeftPanelContent, RightPanelContent } from '@/components/Layout/WorkspaceDrawers'
import { PresenceAvatarCluster } from '@/components/Presence/PresenceWorkspaceAvatars'
import { PageLiveContext } from '@/contexts/TabVisibilityContext'
import { useRoomTier } from '@/lib/room-tiers'
import { usersForWorkspace } from '@/stores/presence'
import { homeRooms, useHomeRoomsStore, type HomeRoomAccess, type HomeRoomEntry } from '@/stores/home-rooms'
import { usePageViewStore } from '@/stores/page-view'
import { RoomFailureGate, roomFailureActions, useRoomFailure } from './RoomFailure'
import { hostPool } from '@/lib/host-pool-instance'
import type { PinnedRoom } from '@/stores/room'

/** What the room bar's chip says (Home M5), and, for an older server, the
 *  label of its "Switch to {server}" button (0.43.2 Z5: with the old
 *  default this row would have switched the window and been usable). */
export function roomAccessCopy(
  access: HomeRoomAccess | null,
  serverLabel: string,
  version: string | null = null,
): { chip: string; title: string; switchLabel: string | null } {
  if (access === 'use') {
    return {
      chip: 'Remote room',
      title: `Everything you do here happens on ${serverLabel}: typing, tabs, files and chats.`,
      switchLabel: null,
    }
  }
  if (access === 'view-older-server') {
    const runs = version === null ? 'an older K2' : `K2 ${version}`
    return {
      chip: 'View only',
      title: `${serverLabel} runs ${runs}, which can’t save this room’s tabs safely.`,
      switchLabel: `Switch to ${serverLabel}`,
    }
  }
  return { chip: 'View only', title: 'Nothing is sent to that server from this room.', switchLabel: null }
}

/** The room's chip (usable, or view only and why), the room's server, and
 *  that server's people on this agent (R7: the room shows B's who's-here). */
export function RoomBar({
  room,
  label,
  access,
}: {
  room: PinnedRoom
  label: string
  access: HomeRoomAccess | null
}): React.JSX.Element {
  const roster = useStore(room.presence, (s) => s.roster)
  const supported = useStore(room.presence, (s) => s.supported)
  const path = room.cwd()
  const people = useMemo(
    () => (supported ? usersForWorkspace(roster, path).map((u) => ({ user: u.user, role: u.role })) : []),
    [roster, supported, path],
  )
  const version = useStore(hostPool.store, (s) => s.entries[room.scope.hostKey]?.boot?.version ?? null)
  const copy = roomAccessCopy(access, room.scope.label, version)
  return (
    <div
      className="flex h-6 flex-shrink-0 items-center gap-2 border-b border-[var(--color-border)] px-3 text-[10px] text-[var(--color-text-muted)]"
      data-room-bar=""
      data-room-access={access ?? ''}
    >
      <span
        className="flex-shrink-0 border border-[var(--color-border)] px-1.5 py-px text-[var(--color-text-secondary)]"
        data-room-chip=""
        title={copy.title}
      >
        {copy.chip}
      </span>
      <span className="truncate" data-room-server="">
        {label} on {room.scope.label}
      </span>
      {access === 'view-older-server' && (
        <span className="truncate" data-room-note="">
          {copy.title}
        </span>
      )}
      {copy.switchLabel !== null && (
        <button
          type="button"
          className="flex-shrink-0 border border-[var(--color-border)] px-1.5 py-px text-[var(--color-text-secondary)] hover:bg-white/[0.05] no-drag cursor-pointer"
          data-room-switch=""
          title={`Switch this window to ${room.scope.label} to type and change tabs`}
          onClick={() => roomFailureActions(room.scope.hostKey).openServer()}
        >
          {copy.switchLabel}
        </button>
      )}
      <span className="ml-auto flex items-center" data-room-people={people.map((p) => p.user).join(',')}>
        <PresenceAvatarCluster users={people} />
      </span>
    </div>
  )
}

/** One remote room, mounted while hot. Hidden (not focused) unless shown. */
function PinnedRoomShell({ entry, shown }: { entry: HomeRoomEntry; shown: boolean }): React.JSX.Element | null {
  const room = entry.room
  if (!room) return null
  return (
    <div
      className="h-full w-full flex-col"
      style={{ display: shown ? 'flex' : 'none' }}
      aria-hidden={shown ? undefined : true}
      data-home-room={entry.address}
      data-room-area=""
      onPointerDownCapture={() => homeRooms.input(entry.address)}
      onKeyDownCapture={() => homeRooms.input(entry.address)}
      onFocusCapture={() => homeRooms.focused(entry.address)}
    >
      <RoomProvider room={room} shown={shown}>
        <RoomFailureGate hostKey={room.scope.hostKey}>
          <div className="flex h-full min-h-0 w-full flex-col">
            <RoomBar room={room} label={entry.label} access={entry.access} />
            <div className="min-h-0 flex-1">
              {/* `shown` already means "Home is the page and this is the
                  room on screen"; the window's own PageLive is about the
                  window's room, which is hidden now. */}
              <PageLiveContext.Provider value={shown}>
                <TerminalArea cwd={room.cwd()} />
              </PageLiveContext.Provider>
            </div>
          </div>
        </RoomFailureGate>
      </RoomProvider>
    </div>
  )
}

/** Mounted only while the room is hot (MS24): unmounting closes its grids,
 *  overlays and chat socket; its workspace socket lives in the tabs store. */
function HotRoom({ entry, shown }: { entry: HomeRoomEntry; shown: boolean }): React.JSX.Element | null {
  const tier = useRoomTier(entry.room?.key ?? '')
  if (tier !== 'hot') return null
  return <PinnedRoomShell key={`${entry.address}#${entry.generation}`} entry={entry} shown={shown} />
}

/** The shown row while it is not an open room yet (or could not open). */
function PendingRoom({ entry }: { entry: HomeRoomEntry }): React.JSX.Element {
  const failure = useRoomFailure(entry.hostKey)
  // The server came back: try the list again once it is usable.
  useEffect(() => {
    if (entry.phase === 'error' && failure === null) void homeRooms.retry(entry.address)
  }, [entry.phase, entry.address, failure])
  const text =
    entry.phase === 'resolving'
      ? `Opening ${entry.label}…`
      : entry.phase === 'not-found'
        ? `${entry.label} is not on that server any more.`
        : `Couldn’t open ${entry.label}${entry.error ? `: ${entry.error}` : '.'}`
  return (
    <div className="h-full w-full" data-home-room={entry.address} data-home-room-phase={entry.phase}>
      <RoomFailureGate hostKey={entry.hostKey}>
        <div className="flex h-full w-full items-center justify-center" data-toast-host="workspace">
          <div className="text-center">
            <p className="text-xs text-[var(--color-text-muted)]">{text}</p>
            {entry.phase !== 'resolving' && (
              <button
                type="button"
                className="mt-2 border border-[var(--color-border)] px-3 py-1 text-xs text-[var(--color-text-secondary)] hover:bg-white/[0.05] no-drag cursor-pointer"
                onClick={() => void homeRooms.retry(entry.address)}
              >
                Retry
              </button>
            )}
          </div>
        </div>
      </RoomFailureGate>
    </div>
  )
}

/** Every remote room this window holds, for Home's main area. Rendered by
 *  `HomeRoomsPortal` (outside the keyed App), shown in the shell's slot. */
export function HomeRemoteRooms(): React.JSX.Element {
  const entries = useHomeRoomsStore((s) => s.entries)
  const shown = useHomeRoomsStore((s) => s.shown)
  const onHome = usePageViewStore((s) => s.page === 'home')
  // Home's page visibility (the tier clock) is driven by the portal host,
  // which also knows when the rooms are parked.
  const shownEntry = shown ? entries[shown] : undefined
  return (
    <>
      {Object.values(entries).map((entry) =>
        entry.phase === 'open' ? (
          <HotRoom key={entry.address} entry={entry} shown={onHome && shown === entry.address} />
        ) : null,
      )}
      {onHome && shownEntry && shownEntry.phase !== 'open' && <PendingRoom entry={shownEntry} />}
    </>
  )
}

/** The shown remote room (on Home), or null. */
export function useShownRemoteRoom(): { entry: HomeRoomEntry; room: PinnedRoom | null } | null {
  const onHome = usePageViewStore((s) => s.page === 'home')
  const entry = useHomeRoomsStore((s) => (s.shown ? s.entries[s.shown] : undefined))
  if (!onHome || !entry) return null
  return { entry, room: entry.phase === 'open' ? entry.room : null }
}

/** The shown remote room's drawers: the same drawer components, in that
 *  room (its files, chat history, Workspace panel). */
export function RemoteRoomDrawer({ room, side }: { room: PinnedRoom; side: 'left' | 'right' }): React.JSX.Element {
  return (
    <RoomProvider room={room}>
      <div className="contents" data-room-key={room.key}>
        {side === 'left' ? <LeftPanelContent rootPath={room.cwd()} /> : <RightPanelContent rootPath={room.cwd()} />}
      </div>
    </RoomProvider>
  )
}
