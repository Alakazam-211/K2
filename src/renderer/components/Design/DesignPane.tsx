// The Design tab's pane (prd-browser-design-mode-v1 §00, D-A6–D-A12).
//
// It draws its OWN two-pane view inside the tab item: the agent's
// conversation in Thread view on the left and the page on the right. It never calls
// `splitTerminalArea` and never touches the room's shared `splitCount` /
// `extraGroups` (DD5): selecting a Design tab is this window's view only.
//
//  - Thread column (Rosson 2026-10-16): its own Thread instance for the
//    workspace's agent (`DesignThreadColumn`), never the pinned Chat's
//    retained AgentChatPane. Opening or closing Design never moves that
//    instance or changes the Chat tab's view. Notes sent "via Design"
//    show up here.
//  - The split width is this window's, per room (`design-split-sizes.ts`,
//    DD6). Dragging it claims the window (`beginBrowserMotion('drag')`) so
//    the native page freezes instead of eating the mouse.
//  - Page: the Browser pane's native child and dock code, created as a
//    design view (`design: true`) so the picker commands accept it. Drawer
//    slides freeze it the same way they freeze a Browser tab (the dock is
//    the Browser pane's, and `AnimatedDrawer` calls `beginBrowserMotion`).
//  - Select is on when the tab opens; the palette in the chrome bar (after
//    Back / Forward) toggles it, accent-coloured while on. No S hotkey.
//    Pick → note pop-up beside the element (its overlay claim freezes the
//    page) → Add to the tray, or ⌘↩ to send now → one Send.
//  - Notes are per viewer; they leave when the tab is closed.

import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useStore } from 'zustand'
import { useRoom } from '@/components/Room/RoomContext'
import { useIsTabVisible } from '@/contexts/TabVisibilityContext'
import { webFeatures } from '@/web/features'
import { BrowserPane, browserNativeItemId, currentParentWindow } from '@/components/BrowserPane/BrowserPane'
import { beginBrowserMotion } from '@/lib/browser-overlay'
import { pickArm, pickDisarm, pickPoll, setDesignBadges } from '@/lib/design-picker-ipc'
import type { AgentItemData, DesignItemData, Tab } from '@/stores/tabs'
import { DesignPaletteButton } from './DesignToolbar'
import { DesignThreadColumn } from './DesignThreadColumn'
import { DesignWebStub } from './DesignWebStub'
import { NotePopover, sendNowChord } from './NotePopover'
import { NoteTray } from './NoteTray'
import { designItemStillOpen } from './design-lifecycle'
import { readDesignSplit, writeDesignSplit } from './design-split-sizes'
import { scaleGuestRect } from './placement'
import { MAX_DESIGN_NOTES, NOTES_FULL_REASON, badgesFor, useDesignNotesStore, useDesignTray } from './useDesignNotes'
import { useDesignPicker } from './useDesignPicker'
import type { DesignPickerIpc, NoteRect, PickResult } from './types'

/** The Tauri picker commands in the shape `useDesignPicker` takes. */
export const tauriDesignPickerIpc: DesignPickerIpc = {
  pickArm: (itemId, parentWindow) => pickArm({ itemId, parentWindow }),
  pickDisarm: (itemId, parentWindow) => pickDisarm({ itemId, parentWindow }),
  pickPoll: (itemId, parentWindow) => pickPoll({ itemId, parentWindow }),
  setDesignBadges: (itemId, parentWindow, badges) => setDesignBadges({ itemId, parentWindow }, badges),
}

/** Smallest share either side of the split keeps (flex units of 100). */
const MIN_SPLIT_SHARE = 15

export interface DesignPaneProps {
  /** Layout item id (the tray key; the native view gets the room-safe id). */
  itemId: string
  tabId: string
  paneGroupId: string
  data: DesignItemData
  /** Test seam: the picker IPC (default: the Tauri commands). */
  ipc?: DesignPickerIpc
}

/** The workspace's pinned Chat (the Chat system tab's agent item). */
function pinnedChatItem(tabs: Tab[]): AgentItemData | null {
  for (const tab of tabs) {
    if (!tab.isSystemAgent) continue
    const item = Array.from(tab.paneGroups.values())[0]?.items[0]
    if (item?.type !== 'agent') continue
    const d = item.data as AgentItemData
    if (d.section === 'chat') return d
  }
  return null
}

export function DesignPane(props: DesignPaneProps): React.JSX.Element {
  // The hosted web client has no Browser pane (D-A15): say so, never a
  // broken pane.
  if (!webFeatures.browserPane) return <DesignWebStub />
  return <DesignDesk {...props} />
}

function DesignDesk({ itemId, tabId, paneGroupId, data, ipc = tauriDesignPickerIpc }: DesignPaneProps): React.JSX.Element {
  const room = useRoom()
  const tabVisible = useIsTabVisible()
  const parentWindow = useMemo(() => currentParentWindow(), [])
  const nativeItemId = browserNativeItemId(room, itemId)

  // ── Chat column ───────────────────────────────────────────────────────
  const chatAgent = useStore(room.tabs, (s) => pinnedChatItem(s.tabs)?.agentName ?? null)
  const chatPath = useStore(room.tabs, (s) => pinnedChatItem(s.tabs)?.projectPath ?? null)
  const chatSession = useStore(room.tabs, (s) => pinnedChatItem(s.tabs)?.sessionId)
  const showChat = !!chatAgent && !!chatPath

  // ── Split width (this window, this room) ──────────────────────────────
  const roomKey = room.roomId() ?? room.key
  const [split, setSplit] = useState<[number, number]>(() => readDesignSplit(roomKey))
  useEffect(() => {
    setSplit(readDesignSplit(roomKey))
  }, [roomKey])
  const rowRef = useRef<HTMLDivElement | null>(null)
  const startSplitDrag = useCallback(
    (e: React.MouseEvent) => {
      e.preventDefault()
      const row = rowRef.current
      if (!row) return
      const box = row.getBoundingClientRect()
      if (box.width <= 0) return
      const endFreeze = beginBrowserMotion('drag')
      let latest: [number, number] = split
      const onMove = (ev: MouseEvent): void => {
        const chat = Math.round(((ev.clientX - box.left) / box.width) * 100)
        const clamped = Math.max(MIN_SPLIT_SHARE, Math.min(100 - MIN_SPLIT_SHARE, chat))
        latest = [clamped, 100 - clamped]
        setSplit(latest)
      }
      const onUp = (): void => {
        window.removeEventListener('mousemove', onMove)
        window.removeEventListener('mouseup', onUp)
        endFreeze()
        writeDesignSplit(roomKey, latest)
      }
      window.addEventListener('mousemove', onMove)
      window.addEventListener('mouseup', onUp)
    },
    [roomKey, split],
  )

  // ── Notes tray ────────────────────────────────────────────────────────
  const tray = useDesignTray(itemId)
  const workspace = chatPath ?? room.cwd()
  const target = useMemo(() => ({ scope: room.scope, workspace }), [room.scope, workspace])
  const send = useCallback(() => void useDesignNotesStore.getState().send(itemId, target), [itemId, target])
  const retry = useCallback(() => void useDesignNotesStore.getState().retry(itemId, target), [itemId, target])

  // Drop the tray when the tab is CLOSED (not on a workspace switch or a
  // layout re-apply, which unmount and remount the same item).
  useEffect(() => {
    return () => {
      setTimeout(() => {
        if (!designItemStillOpen(room.tabs.getState(), itemId)) useDesignNotesStore.getState().drop(itemId)
      }, 0)
    }
  }, [room, itemId])

  // ── Picker ────────────────────────────────────────────────────────────
  const [pick, setPick] = useState<PickResult | null>(null)
  const [popoverBox, setPopoverBox] = useState<{ anchor: NoteRect; container: NoteRect } | null>(null)
  const dockRef = useRef<HTMLDivElement | null>(null)
  const lastErrorRef = useRef<string | null>(null)
  const onError = useCallback((err: unknown) => {
    // A poll before the page exists fails every tick; log each new reason once.
    const msg = err instanceof Error ? err.message : String(err)
    if (msg === lastErrorRef.current) return
    lastErrorRef.current = msg
    console.warn('[design] picker:', msg)
  }, [])
  const onPick = useCallback((p: PickResult) => {
    const dock = dockRef.current?.getBoundingClientRect()
    const root = rowRef.current?.getBoundingClientRect()
    if (!dock || !root) return
    const dockRect: NoteRect = { x: dock.left, y: dock.top, width: dock.width, height: dock.height }
    const anchor = p.element.rect
      ? scaleGuestRect(p.element.rect, p.viewport, dockRect)
      : { x: dockRect.x + dockRect.width / 2, y: dockRect.y + dockRect.height / 2, width: 0, height: 0 }
    setPopoverBox({ anchor, container: { x: root.left, y: root.top, width: root.width, height: root.height } })
    setPick(p)
  }, [])
  const picker = useDesignPicker({
    itemId: nativeItemId,
    parentWindow,
    ipc,
    visible: tabVisible,
    onPick,
    onError,
  })
  const { setArmed } = picker

  // One pick at a time: Select pauses while the pop-up is open and comes
  // back after Add, Cancel or ⌘↩ (pin many, send once).
  useEffect(() => {
    if (pick) setArmed(false)
  }, [pick, setArmed])
  const closePopover = useCallback(() => {
    setPick(null)
    setPopoverBox(null)
    setArmed(true)
  }, [setArmed])
  const sendNow = useCallback(
    (comment: string, intent: 'change' | 'question') => {
      const typed = pick && comment
        ? {
            comment,
            intent,
            element: pick.element,
            url: pick.url,
            title: pick.title,
            viewport: { width: pick.viewport.width, height: pick.viewport.height },
          }
        : undefined
      closePopover()
      void useDesignNotesStore.getState().sendNow(itemId, target, typed)
    },
    [itemId, pick, target, closePopover],
  )
  const addNote = useCallback(
    (comment: string, intent: 'change' | 'question') => {
      if (!pick) return
      const res = useDesignNotesStore.getState().add(itemId, {
        comment,
        intent,
        element: pick.element,
        url: pick.url,
        title: pick.title,
        viewport: { width: pick.viewport.width, height: pick.viewport.height },
      })
      if (!res.ok) {
        console.warn('[design] note not added:', res.reason)
        return
      }
      closePopover()
    },
    [itemId, pick, closePopover],
  )

  // Numbered badges in the page follow the tray (visual only).
  const badges = useMemo(() => badgesFor(tray), [tray])
  useEffect(() => {
    if (!tabVisible) return
    ipc.setDesignBadges(nativeItemId, parentWindow, badges).catch(onError)
  }, [ipc, nativeItemId, parentWindow, badges, tabVisible, onError])

  const store = useDesignNotesStore.getState
  const toolbar = (
    <>
      <NoteTray
        notes={tray.notes}
        sending={tray.sending}
        error={tray.error}
        canRetry={tray.pendingThreadItemId !== null}
        lockedNoteIds={tray.pendingNoteIds}
        onEdit={(noteId, comment) => store().edit(itemId, noteId, { comment })}
        onRemove={(noteId) => store().remove(itemId, noteId)}
        onClear={() => store().clear(itemId)}
        onSend={send}
        onSendOne={(noteId) => void store().sendOne(itemId, noteId, target)}
        onRetry={retry}
        sendAllHint={sendNowChord()}
      />
    </>
  )

  return (
    <div ref={rowRef} className="flex h-full w-full min-w-0" data-design-pane={itemId}>
      {showChat && chatAgent && chatPath ? (
        <>
          <div
            className="h-full min-w-0 overflow-hidden"
            style={{ flex: `${split[0]} 1 0` }}
            data-design-chat=""
          >
            <DesignThreadColumn
              projectPath={chatPath}
              agentName={chatAgent}
              conversationId={chatSession ?? null}
              visible={tabVisible}
            />
          </div>
          <div
            role="separator"
            aria-orientation="vertical"
            aria-label="Resize Thread and page"
            className="w-1 flex-shrink-0 cursor-col-resize bg-[var(--color-border)] hover:bg-[var(--color-accent)]"
            onMouseDown={startSplitDrag}
            data-design-split=""
          />
        </>
      ) : null}
      <div className="h-full min-w-0" style={{ flex: `${showChat ? split[1] : 100} 1 0` }}>
        <BrowserPane
          itemId={itemId}
          tabId={tabId}
          paneGroupId={paneGroupId}
          url={data.url}
          navSeq={data.navSeq}
          design
          aboveDock={toolbar}
          navExtra={<DesignPaletteButton armed={picker.armed} onToggle={picker.toggle} />}
          dockRef={dockRef}
        />
      </div>
      {pick && popoverBox ? (
        <NotePopover
          key={pick.pickId}
          open
          element={pick.element}
          anchor={popoverBox.anchor}
          container={popoverBox.container}
          onAdd={addNote}
          onSendNow={sendNow}
          trayCount={tray.notes.length}
          addDisabledReason={tray.notes.length >= MAX_DESIGN_NOTES ? NOTES_FULL_REASON : null}
          onCancel={closePopover}
        />
      ) : null}
    </div>
  )
}
