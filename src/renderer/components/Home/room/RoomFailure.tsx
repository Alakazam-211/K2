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
import { Button } from '@/components/ui'

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

/**
 * The banner. Pure presentation: the caller passes the state and actions.
 *
 * One markup, two looks, through the Style contract only:
 *   - Terminal: a square strip on the surface with a bottom rule; the action
 *     is the shared quiet Button (cut corners).
 *   - Glass: `data-material="tile"` makes it a tile (rounded box radius, the
 *     tile edge, sheen and shadow); the Gate's `--gap-pane` gap floats it off
 *     the room below. The action is the same quiet Button (a pill in Glass).
 * The title wraps to two lines at most (a long server name breaks anywhere);
 * the button label truncates past 45% of the row. Under 32rem (a narrow
 * window or split) the button drops under the text and may use the full
 * width. Both keep their full text in `title`.
 */
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
    // Not <Surface bordered={false}>: that zeroes --k2-ch-bw, which inherits
    // and would drop the Retry button's diagonal edge in Terminal.
    <div
      role="alert"
      data-room-failure={failure.kind}
      data-surface="surface"
      data-material="tile"
      className="@container flex flex-shrink-0 flex-wrap items-center gap-x-3 gap-y-2 px-4 py-2.5 bg-[var(--color-bg-surface)] rounded-[var(--radius-box)] border-b border-[var(--color-border)]"
    >
      <div className="min-w-0 flex-1 @max-[32rem]:basis-full">
        <p
          className="type-name text-[var(--color-text-primary)] line-clamp-2 [overflow-wrap:anywhere]"
          title={failure.title}
          data-room-failure-title=""
        >
          {failure.title}
        </p>
        {failure.detail && (
          <p className="type-hint text-[var(--color-text-muted)] mt-0.5 [overflow-wrap:anywhere]" data-room-failure-detail="">
            {failure.detail}
          </p>
        )}
      </div>
      {failure.action && failure.actionLabel && (
        <Button
          variant="quiet"
          data-room-failure-action={failure.action}
          onClick={onClick}
          title={failure.actionLabel}
          className="flex-shrink-0 max-w-[45%] @max-[32rem]:max-w-full"
        >
          {/* The label truncates, not the button: overflow on the button
              would clip Terminal's chamfer edge. */}
          <span className="block truncate">{failure.actionLabel}</span>
        </Button>
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
    // --gap-pane: 0 in Terminal (the strip sits on the room), a tile gap in
    // Glass (the banner floats as its own tile above the room's tiles).
    <div className="flex flex-col h-full min-h-0 gap-[var(--gap-pane)]" data-room-failing={failure.kind}>
      <RoomFailureView failure={failure} actions={actions ?? roomFailureActions(hostKey)} />
      <div className="flex-1 min-h-0 opacity-50 pointer-events-none" aria-disabled="true" inert>
        {children}
      </div>
    </div>
  )
}
