// The Desktop tab's pane (prd-remote-desktop-tab-v1 §8), the D7a shell:
// the toolbar, the state cards, and the stage where D7b draws the picture.
//
// Daemon-first: the daemon owns the switch, who may open, and the machine's
// state. This pane reads `GET /cli/desktop/status` on the ROOM's server (the
// server that owns the workspace whose strip holds this tab, vs-live R22)
// and draws the one card that status means for this viewer (`tabState`).
// A server without `remote-desktop-v1` is never dialled: the card says to
// update K2 there. No video yet: a `ready` desktop shows the toolbar and a
// placeholder.

import { useCallback, useEffect, useState } from 'react'
import { useToastStore } from '@/stores/toast'
import { useRoom } from '@/components/Room/RoomContext'
import { useIsTabVisible } from '@/contexts/TabVisibilityContext'
import { isMacPlatform } from '@/lib/desktop-chrome'
import type { DesktopItemData } from '@/stores/tabs'
import { DesktopStateCard } from './DesktopStateCard'
import { DesktopToolbar } from './DesktopToolbar'
import { enableDesktop, fetchDesktopDisplays } from './desktop-api'
import type { DesktopFit } from './desktop-serialize'
import { tabState, type CardAction } from './desktop-state'
import type { Display } from './desktop-wire'
import { useDesktopStatus } from './useDesktopStatus'

export const DESKTOP_READY_PLACEHOLDER = 'Connected. The live picture arrives in a later K2 build.'

export interface DesktopPaneProps {
  tabId: string
  paneGroupId: string
  itemId: string
  data: DesktopItemData
}

export function DesktopPane(props: DesktopPaneProps): React.JSX.Element {
  const room = useRoom()
  const scope = room.scope
  const visible = useIsTabVisible()
  const { supported, status, error, refresh } = useDesktopStatus(scope, visible)
  const [busy, setBusy] = useState(false)
  const [enableCard, setEnableCard] = useState<string | undefined>(undefined)
  const [displays, setDisplays] = useState<Display[]>([])
  const [capture, setCapture] = useState(false)
  const [flip, setFlip] = useState(false)

  const state = status ? tabState(status) : null
  const ready = state === 'ready'

  useEffect(() => {
    if (!ready || !visible) return
    let cancelled = false
    fetchDesktopDisplays(scope).then(
      (d) => {
        if (!cancelled) setDisplays(d)
      },
      (err: unknown) => console.warn('[desktop] desktop/displays:', err instanceof Error ? err.message : String(err)),
    )
    return () => {
      cancelled = true
    }
  }, [scope, ready, visible])

  const stamp = useCallback(
    (patch: Partial<Omit<DesktopItemData, 'desktopId'>>) => {
      room.tabs.getState().setDesktopItemState(props.tabId, props.paneGroupId, props.itemId, patch)
    },
    [room, props.tabId, props.paneGroupId, props.itemId],
  )

  const onAction = useCallback(
    (action: CardAction) => {
      if (action === 'turn-on' || action === 'ask-at-mac') {
        // Ask at the Mac re-arms K2 Screen, which asks for its permission.
        setBusy(true)
        enableDesktop(scope)
          .then(
            (r) => {
              if (r.card) setEnableCard(r.card)
            },
            (err: unknown) => {
              const msg = err instanceof Error ? err.message : String(err)
              useToastStore.getState().addToast(`Couldn’t turn on Remote desktop: ${msg}`, 'error')
            },
          )
          .finally(() => {
            setBusy(false)
            refresh()
          })
        return
      }
      // Retry, Try again, Cancel: read the machine again (D7b also closes
      // or reopens the stream here).
      refresh()
    },
    [scope, refresh],
  )

  if (!supported) {
    return <DesktopStateCard state="update_k2" mayManage={false} onAction={onAction} />
  }
  if (!status || !state) {
    return (
      <div className="flex h-full w-full items-center justify-center text-xs text-[var(--color-text-muted)]" data-desktop-loading="">
        {error ? `Couldn’t read this computer’s Remote desktop: ${error}` : 'Checking this computer…'}
      </div>
    )
  }
  const fit: DesktopFit = props.data.fit ?? 'fit'
  const viewOnly = !!props.data.viewOnly || !status.you.mayControl
  return (
    <div className="flex h-full w-full flex-col" data-desktop-pane={state}>
      <DesktopToolbar
        machine={status.machine}
        displays={displays}
        displayId={props.data.displayId ?? displays.find((d) => d.main)?.id ?? null}
        fit={fit}
        viewOnly={viewOnly}
        mayControl={status.you.mayControl}
        capture={capture}
        flipModifiers={flip}
        live={false}
        viewers={status.viewers ? status.viewers.length : null}
        latencyMs={null}
        onDisplay={(id) => stamp({ displayId: id })}
        onFit={(f) => stamp({ fit: f })}
        onViewOnly={(on) => stamp({ viewOnly: on })}
        onCapture={setCapture}
        onFlipModifiers={setFlip}
      />
      <div className="relative min-h-0 flex-1" data-desktop-stage="" data-desktop-viewer-mac={String(isMacPlatform())}>
        {ready ? (
          <div className="flex h-full w-full items-center justify-center text-xs text-[var(--color-text-muted)]" data-desktop-placeholder="">
            {DESKTOP_READY_PLACEHOLDER}
          </div>
        ) : (
          <DesktopStateCard
            state={state}
            mayManage={status.you.mayManage}
            detail={state === status.state ? status.message : undefined}
            enableCard={enableCard}
            busy={busy}
            onAction={onAction}
          />
        )}
      </div>
    </div>
  )
}
