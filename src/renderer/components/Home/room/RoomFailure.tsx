// Home M4 — a remote room's failure states (prd-home-multi-server-client
// MS27, MS30, MS45, MS83). The copy and the one action per state come from
// `lib/room-failure.ts`; this file renders them and wires the actions to
// THAT server only:
//
//   - Retry: `hostPool.check(hostKey)` (the pinned server, never the
//     window's).
//   - Sign in: the non-switching sign-in for that saved server
//     (`signInForManagement`), or its password-rotation step. Never the
//     full-screen overlay on its own, never a keychain delete (MS36).
//   - Open B's server: the explicit switch of this window to B, the same
//     path Home uses with the preview off (`pickHost`).
//
// The pinned-room shell (`HomeRemoteRooms`) wraps its content in
// `<RoomFailureGate hostKey={room.scope.hostKey}>`, so a failing server
// shows the banner over its last frame (dimmed, input off) and never falls
// back to the window's server.

import React from 'react'
import { useStore } from 'zustand'
import { hostPool } from '@/lib/host-pool-instance'
import { nextCheckDelayMs } from '@/lib/host-pool'
import { LOCAL_HOME_HOST, savedHostForKey } from '@/lib/host-key'
import { roomFailure, type RoomFailure } from '@/lib/room-failure'
import { useConnectHostStore } from '@/stores/connect-host'

export interface RoomFailureActions {
  retry(): void
  signIn(): void
  openServer(): void
}

/** The actions for one server. Each one targets `hostKey` only. */
export function roomFailureActions(hostKey: string): RoomFailureActions {
  return {
    retry() {
      void hostPool.check(hostKey)
    },
    signIn() {
      if (hostKey === LOCAL_HOME_HOST) {
        void hostPool.check(hostKey)
        return
      }
      const state = useConnectHostStore.getState()
      const saved = savedHostForKey(state.hosts, hostKey)
      if (!saved) return
      if (hostPool.entry(hostKey)?.auth === 'rotate-required') {
        state.requestPasswordRotation(saved, { activate: false })
      } else {
        state.signInForManagement(saved)
      }
    },
    openServer() {
      const state = useConnectHostStore.getState()
      // The same target Home's row click uses with the preview off
      // (`switchTargetForHost` in lib/home-open), without pulling the
      // projects store into this component.
      if (hostKey === LOCAL_HOME_HOST) {
        state.pickHost('local')
        return
      }
      const saved = savedHostForKey(state.hosts, hostKey)
      if (!saved) return
      state.pickHost(saved)
    },
  }
}

function serverLabelOf(hostKey: string, hosts: ReturnType<typeof useConnectHostStore.getState>['hosts']): string {
  if (hostKey === LOCAL_HOME_HOST) return 'This computer'
  const saved = savedHostForKey(hosts, hostKey)
  return saved ? saved.label || saved.hostname : hostKey
}

/** The failure a room on `hostKey` shows right now, or null. */
export function useRoomFailure(hostKey: string): RoomFailure | null {
  const hosts = useConnectHostStore((s) => s.hosts)
  const entry = useStore(hostPool.store, (s) => s.entries[hostKey])
  const saved = hostKey === LOCAL_HOME_HOST || savedHostForKey(hosts, hostKey) !== null
  const now = Date.now()
  return roomFailure({
    serverLabel: serverLabelOf(hostKey, hosts),
    entry,
    saved,
    now,
    nextCheckAt: entry?.checkedAt != null ? entry.checkedAt + nextCheckDelayMs(entry) : null,
  })
}

/** The banner. Pure presentation: the caller passes the state and actions. */
export function RoomFailureView({
  failure,
  actions,
}: {
  failure: RoomFailure
  actions: RoomFailureActions
}): React.JSX.Element {
  const onClick = (): void => {
    if (failure.action === 'retry') actions.retry()
    else if (failure.action === 'sign-in') actions.signIn()
    else if (failure.action === 'open-server') actions.openServer()
  }
  return (
    <div
      role="alert"
      data-room-failure={failure.kind}
      className="flex items-center justify-between gap-3 px-4 py-3 border-b border-[var(--color-border)] bg-[var(--color-bg-surface)]"
    >
      <div className="min-w-0">
        <p className="text-xs text-[var(--color-text-primary)]">{failure.title}</p>
        {failure.detail && (
          <p className="text-[10px] text-[var(--color-text-muted)] mt-0.5">{failure.detail}</p>
        )}
      </div>
      {failure.action && failure.actionLabel && (
        <button
          type="button"
          data-room-failure-action={failure.action}
          onClick={onClick}
          className="flex-shrink-0 px-3 py-1 text-xs text-[var(--color-accent)] border border-[var(--color-accent)]/30 hover:bg-[var(--color-accent)]/10 no-drag cursor-pointer"
        >
          {failure.actionLabel}
        </button>
      )}
    </div>
  )
}

/**
 * A room on `hostKey`: its children as they are when the server is usable;
 * otherwise the banner above the children's last frame, dimmed, with input
 * off (MS45). Never renders anything from another server.
 */
export function RoomFailureGate({
  hostKey,
  actions,
  children,
}: {
  hostKey: string
  /** Defaults to `roomFailureActions(hostKey)`; tests inject their own. */
  actions?: RoomFailureActions
  children?: React.ReactNode
}): React.JSX.Element {
  const failure = useRoomFailure(hostKey)
  if (!failure) return <>{children}</>
  return (
    <div className="flex flex-col h-full min-h-0" data-room-failing={failure.kind}>
      <RoomFailureView failure={failure} actions={actions ?? roomFailureActions(hostKey)} />
      <div className="flex-1 min-h-0 opacity-50 pointer-events-none" aria-disabled="true" inert>
        {children}
      </div>
    </div>
  )
}
