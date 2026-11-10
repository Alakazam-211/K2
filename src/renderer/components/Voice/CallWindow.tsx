// The call in its own window (Rosson 2026-11-10, "pop out", as a Focus
// window holds one agent): the call's video, its time, and its controls, and
// nothing else. With a screen share, the share is in the middle and the
// people in a column of smaller tiles on the left (CallStage); with none, the
// grid. Opened by `window_open_call` (src-tauri call_window.rs).
//
// The call doesn't move here: it lives in the app's Rust process, and this
// window adopts it as any K2 window does (voice-core `adoptNativeCall`) and
// draws its own video sinks. Closing the window leaves the call up and puts
// it back in the room's window (the floating tile); leaving the call here
// ends it everywhere, and then this window closes itself. Pop in (title bar)
// closes this window and opens the call view in the window that popped it
// out, with the same share in the middle. Its size and place are kept per
// client; the OS's own full screen (its green button) is the window's.
//
// K2's voice rooms have no in-call chat today, so there's none to bring.

import { useEffect, useRef, useState } from 'react'
import { getCurrentWindow } from '@tauri-apps/api/window'
import DesktopChromeLeft from '@/components/TopBar/DesktopChromeLeft'
import DesktopChromeRight from '@/components/TopBar/DesktopChromeRight'
import { titleBarDragOnMouseDown, titleBarOnContextMenu, titleBarOnDoubleClick } from '@/lib/titlebar-drag'
import { CALL_WINDOW_ADOPT_MS, callStartedAt } from '@/lib/voice/call-view'
import { useSettingsStore } from '@/stores/settings'
import { keepCallWindowFrame, popInCall } from '@/stores/call-popout'
import { loadSharePick } from '@/stores/call-share'
import { retainVideo, video } from '@/stores/video'
import { retainVoice, useVoice } from '@/stores/voice'
import CallControls from './CallControls'
import { callActions } from './CallLayer'
import CallStage from './CallStage'
import { useCallDuration } from './CallTile'
import { PopInButton } from './CallViewButtons'
import { ScreenPicker, VideoNoticeLine } from './VideoControls'
import { useStageTiles } from './VideoStage'
import { TOPBAR_HEIGHT_CSS } from '../../../shared/constants'

const NO_PEOPLE: never[] = []

export interface CallWindowProps {
  /** Close this window (tests pass their own). */
  closeWindow?: () => void
}

function closeThisWindow(): void {
  try {
    void getCurrentWindow()
      .close()
      .catch((err: unknown) => console.warn('[call] could not close the call window:', err))
  } catch (err) {
    console.warn('[call] could not close the call window:', err)
  }
}

export default function CallWindow({ closeWindow = closeThisWindow }: CallWindowProps): React.JSX.Element {
  useEffect(() => retainVoice(), [])
  useEffect(() => retainVideo(), [])
  useEffect(() => {
    let off: (() => void) | null = null
    let gone = false
    void keepCallWindowFrame()
      .then((u) => {
        if (gone) u()
        else off = u
      })
      .catch((err: unknown) => console.warn('[call] window frame not kept:', err))
    return () => {
      gone = true
      off?.()
    }
  }, [])

  const call = useVoice((s) => s.call)
  const native = useVoice((s) => s.engineKind) === 'native'
  const preview = useSettingsStore((s) => s.videoRoomsPreview) === true
  const people = call?.participants ?? NO_PEOPLE
  const tiles = useStageTiles(video, people)
  // The call's own clock: when it connected, as the first window to see it
  // knew (this window adopted it later).
  const startedAt = call ? (call.callId ? callStartedAt(call.callId, call.connectedAt) : call.connectedAt) : null
  const time = useCallDuration(startedAt)
  // The share the room's window had in the middle, here too.
  const callId = call?.callId ?? null
  useEffect(() => loadSharePick(callId), [callId])

  // The call shows up once this window has adopted it. It ending after that
  // (left here or elsewhere, or dropped) closes the window; never showing up
  // says so.
  const had = useRef(false)
  const [waited, setWaited] = useState(false)
  useEffect(() => {
    if (call) had.current = true
    else if (had.current) closeWindow()
  }, [call, closeWindow])
  useEffect(() => {
    const id = setTimeout(() => setWaited(true), CALL_WINDOW_ADOPT_MS)
    return () => clearTimeout(id)
  }, [])
  // The OS title follows the room.
  const roomName = call?.roomName ?? null
  useEffect(() => {
    if (!roomName) return
    try {
      void getCurrentWindow()
        .setTitle(`K2 | ${roomName}`)
        .catch(() => {})
    } catch {
      // Not in the app (tests, the web client): no OS title.
    }
  }, [roomName])

  return (
    <div className="fixed inset-[var(--inset-window)] flex flex-col bg-[var(--color-bg)]" data-testid="call-window">
      <div
        className="flex flex-shrink-0 select-none items-center gap-3 border-b border-[var(--color-border)] px-3"
        style={{ height: TOPBAR_HEIGHT_CSS, minHeight: TOPBAR_HEIGHT_CSS }}
        onMouseDown={titleBarDragOnMouseDown}
        onDoubleClick={titleBarOnDoubleClick}
        onContextMenu={titleBarOnContextMenu}
      >
        <DesktopChromeLeft />
        <span data-testid="call-window-title" className="min-w-0 flex-1 truncate text-[12px] text-[var(--color-text-primary)]">
          {call ? call.roomName : 'Call'}
        </span>
        {time ? (
          <span data-testid="call-window-duration" className="font-mono text-[11px] tabular-nums text-[var(--color-text-secondary)]">
            {time}
          </span>
        ) : null}
        {call ? <PopInButton testId="call-window-popin" onPress={() => popInCall(call.callId, closeWindow)} /> : null}
        <DesktopChromeRight />
      </div>
      {!call ? (
        <div data-testid="call-window-empty" className="flex flex-1 flex-col items-center justify-center gap-2 text-[12px] text-[var(--color-text-muted)]">
          {waited || (native && !preview) ? (
            <>
              <span>There's no call to show here.</span>
              <button
                type="button"
                data-testid="call-window-close"
                className="border border-[var(--color-border)] px-2 py-0.5 text-[11px] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)]"
                onClick={closeWindow}
              >
                Close
              </button>
            </>
          ) : (
            <span>Finding the call…</span>
          )}
        </div>
      ) : (
        <>
          {tiles.length > 0 ? (
            <CallStage ctl={video} people={people} callId={call.callId} className="flex-1" />
          ) : (
            <div data-testid="call-window-no-video" className="flex flex-1 items-center justify-center text-[12px] text-[var(--color-text-muted)]">
              No one has video on.
            </div>
          )}
          <div className="flex flex-shrink-0 flex-col items-center gap-1 border-t border-[var(--color-border)] px-2 py-1.5">
            <CallControls variant="overlay" call={call} actions={callActions} video={video} />
            <ScreenPicker ctl={video} />
            <VideoNoticeLine ctl={video} noticeOnly />
          </div>
        </>
      )}
    </div>
  )
}
