// A voice room's recordings, from the headset's list (until rooms show on
// Home as channels): each room of this server gets a quiet "Recordings"
// entry once the server says this login may list them. It opens a drawer
// over the page, at the right edge in the drawers' width: the room's
// recordings newest first, each as the DM's recording card (when, how long,
// who started it, the transcript's state), and Show transcript opens the
// DM's own transcript panel in the same drawer (speaker turns, search,
// Copy transcript). Data: `lib/voice/room-recordings`.
//
// Safe on older servers: the entry asks the server once per open of the
// list (`probeRoomRecordings`) and stays hidden on any refusal, any error and
// an unknown route. Nothing here polls or retries.

import { useEffect, useMemo, useRef, useState, type JSX } from 'react'
import { createPortal } from 'react-dom'
import { create } from 'zustand'
import { scopeForHost } from '@/kessel/server-scope'
import { DialogScrim } from '@/components/ui/Dialog'
import { DialogCloseButton } from '@/components/ui/dialog-cancel'
import { HomeChatRecordingCard, HomeChatTranscriptPanel, useTranscriptPanelWidth, type TranscriptPerson } from '@/components/HomeChat/HomeChatRecording'
import type { TranscriptLine } from '@/lib/home-chat/recordings'
import type { VoiceOccupant } from '@/lib/voice/voice-wire'
import { daemonErrorText } from '@/lib/daemon-error'
import {
  fetchRoomRecordings,
  fetchRoomTranscript,
  probeRoomRecordings,
  recordingWhen,
  roomRecordingCardView,
  roomTranscriptHeading,
  transcriptReadable,
  transcriptRefusalLine,
  type RoomRecording,
  type RoomRecordingsPage,
} from '@/lib/voice/room-recordings'

/** The drawer's layer: the members sheet's, under the confirm dialog's. */
export const ROOM_RECORDINGS_Z = 99990

export const NO_RECORDINGS_LINE = 'No recordings yet in this room.'
export const SIGN_IN_TO_READ_LINE = 'To read a transcript, sign in to this server with your own owner or Admin login.'

// ── which room's drawer is open ───────────────────────────────────────

export interface RoomRecordingsTarget {
  hostKey: string
  /** The room's slug: what the daemon's `room=` takes. */
  roomId: string
  roomName: string
  /** The room's people now: who pressed Record, by Sew's member id. */
  occupants: VoiceOccupant[]
}

interface SheetState {
  open: RoomRecordingsTarget | null
  show: (t: RoomRecordingsTarget) => void
  close: () => void
}

export const useRoomRecordingsSheet = create<SheetState>((set) => ({
  open: null,
  show: (t) => set({ open: t }),
  close: () => set({ open: null }),
}))

// ── the gate ──────────────────────────────────────────────────────────

/** Only an owner's or Admin's login lists the rooms' recordings (the daemon
 *  refuses a Member and a Voice & Chat only login), so only they ask. No
 *  role known (this computer's own server, the owner token) asks too. */
export function mayListRecordings(role: string | null): boolean {
  return role === null || role === 'owner' || role === 'admin'
}

/** True once `hostKey`'s server lists recordings for this login. Asked when
 *  the list mounts (it mounts each time it opens) and when `enabled` turns
 *  on, never on a re-render. */
export function useRoomRecordingsGate(hostKey: string, enabled: boolean): boolean {
  const [shown, setShown] = useState<{ key: string; on: boolean } | null>(null)
  useEffect(() => {
    if (!enabled || !hostKey) return
    let live = true
    let scope: ReturnType<typeof scopeForHost>
    try {
      scope = scopeForHost(hostKey)
    } catch {
      return
    }
    void probeRoomRecordings(scope).then((on) => {
      if (live) setShown({ key: hostKey, on })
    })
    return () => {
      live = false
    }
  }, [hostKey, enabled])
  return enabled && shown?.key === hostKey && shown.on
}

/** The room row's entry. */
export function RoomRecordingsButton({ onOpen }: { onOpen: () => void }): JSX.Element {
  return (
    <button
      type="button"
      data-testid="voice-room-recordings"
      className="type-hint text-[var(--color-text-muted)] underline-offset-2 hover:text-[var(--color-text-primary)] hover:underline"
      onClick={onOpen}
    >
      Recordings
    </button>
  )
}

// ── the drawer ────────────────────────────────────────────────────────

type ListState = { kind: 'loading' } | { kind: 'error'; line: string } | { kind: 'ok'; page: RoomRecordingsPage }

interface OpenTranscript {
  rec: RoomRecording
  lines: TranscriptLine[] | null
  names: Map<string, string>
  state: string | null
}

/** Mounted once beside the headset; draws nothing until a room's drawer opens. */
export function VoiceRoomRecordingsSheet(): JSX.Element | null {
  const open = useRoomRecordingsSheet((s) => s.open)
  const close = useRoomRecordingsSheet((s) => s.close)
  if (!open) return null
  return createPortal(<RecordingsDrawer key={`${open.hostKey}/${open.roomId}`} target={open} onClose={close} />, document.body)
}

export function RecordingsDrawer({ target, onClose }: { target: RoomRecordingsTarget; onClose: () => void }): JSX.Element {
  const width = useTranscriptPanelWidth()
  const [list, setList] = useState<ListState>({ kind: 'loading' })
  const [transcript, setTranscript] = useState<OpenTranscript | null>(null)
  const closeRef = useRef<HTMLButtonElement>(null)
  const { hostKey, roomId } = target

  // One read per open of the drawer.
  useEffect(() => {
    let live = true
    fetchRoomRecordings(scopeForHost(hostKey), roomId).then(
      (page) => live && setList({ kind: 'ok', page }),
      (e: unknown) => live && setList({ kind: 'error', line: `Couldn’t read this room’s recordings. ${daemonErrorText(e) || String(e)}` }),
    )
    return () => {
      live = false
    }
  }, [hostKey, roomId])

  useEffect(() => {
    if (!transcript) closeRef.current?.focus()
  }, [transcript])

  // Escape with the focus elsewhere (the page, after a press on the list):
  // the transcript goes back to the list, the list closes. A press the
  // drawer took already (the search clearing, the panel closing) is left be.
  const reading = transcript !== null
  useEffect(() => {
    const onKey = (e: KeyboardEvent): void => {
      if (e.key !== 'Escape' || e.defaultPrevented) return
      e.preventDefault()
      if (reading) setTranscript(null)
      else onClose()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [reading, onClose])

  const starterOf = useMemo(() => {
    const byMember = new Map(target.occupants.filter((o) => o.memberId).map((o) => [o.memberId as string, o.name ?? o.user]))
    return (memberId: string | null) => (memberId ? (byMember.get(memberId) ?? null) : null)
  }, [target.occupants])

  const showTranscript = (rec: RoomRecording): void => {
    setTranscript({ rec, lines: null, names: new Map(), state: 'Loading…' })
    fetchRoomTranscript(scopeForHost(hostKey), rec.id).then(
      (t) => setTranscript((cur) => (cur && cur.rec.id === rec.id ? { rec, lines: t.lines, names: t.names, state: null } : cur)),
      (e: unknown) => setTranscript((cur) => (cur && cur.rec.id === rec.id ? { rec, lines: null, names: new Map(), state: transcriptRefusalLine(e) } : cur)),
    )
  }

  const personOf = (key: string): TranscriptPerson => {
    const name = transcript?.names.get(key) ?? key
    return { name, username: name }
  }

  const canRead = list.kind === 'ok' && list.page.canRead
  const anyReadable = list.kind === 'ok' && list.page.recordings.some(transcriptReadable)

  return (
    <>
      <DialogScrim zIndex={ROOM_RECORDINGS_Z} />
      <div
        className="no-drag fixed inset-0 flex justify-end"
        style={{ zIndex: ROOM_RECORDINGS_Z + 1 }}
        data-testid="voice-recordings-drawer"
        onPointerDown={(e) => {
          if (e.target === e.currentTarget) onClose()
        }}
      >
        {transcript ? (
          <HomeChatTranscriptPanel
            heading={roomTranscriptHeading(transcript.rec)}
            lines={transcript.lines}
            state={transcript.state}
            personOf={personOf}
            onClose={() => setTranscript(null)}
          />
        ) : (
          <aside
            aria-label={`Recordings in ${target.roomName}`}
            data-testid="voice-recordings"
            data-state={list.kind}
            className="flex max-w-[45%] shrink-0 flex-col border-l border-[var(--color-border)] bg-[var(--color-bg)]"
            style={{ width }}
            onKeyDown={(e) => {
              if (e.key !== 'Escape' || e.defaultPrevented) return
              e.preventDefault()
              onClose()
            }}
          >
            <div className="flex items-center gap-2 border-b border-[var(--color-border)] px-3 py-2">
              <h2 className="type-name min-w-0 flex-1 truncate text-[var(--color-text-primary)]" data-testid="voice-recordings-heading">
                Recordings · {target.roomName}
              </h2>
              <DialogCloseButton ref={closeRef} className="-mr-1" data-testid="voice-recordings-close" onClick={onClose} />
            </div>
            <div className="min-h-0 flex-1 overflow-y-auto py-2">
              {list.kind === 'loading' ? (
                <p className="type-hint px-3 text-[var(--color-text-muted)]" data-testid="voice-recordings-loading">
                  Loading…
                </p>
              ) : list.kind === 'error' ? (
                <p className="type-hint px-3 text-[var(--color-status-error-soft)]" data-testid="voice-recordings-error">
                  {list.line}
                </p>
              ) : list.page.recordings.length === 0 ? (
                <p className="type-hint px-3 text-[var(--color-text-muted)]" data-testid="voice-recordings-empty">
                  {NO_RECORDINGS_LINE}
                </p>
              ) : (
                <ul className="flex flex-col gap-1">
                  {list.page.recordings.map((r) => {
                    const view = roomRecordingCardView(r, starterOf(r.startedBy))
                    return (
                      <li key={r.id} data-testid="voice-recording" data-recording={r.id}>
                        <HomeChatRecordingCard
                          view={view}
                          label={[view.title, recordingWhen(r.startedAt), view.detail].join(' · ')}
                          timeLabel={recordingWhen(r.startedAt)}
                          onShowTranscript={canRead && transcriptReadable(r) ? () => showTranscript(r) : undefined}
                        />
                      </li>
                    )
                  })}
                </ul>
              )}
              {list.kind === 'ok' && !canRead && anyReadable ? (
                <p className="type-hint mt-2 px-4 text-[var(--color-text-muted)]" data-testid="voice-recordings-sign-in">
                  {SIGN_IN_TO_READ_LINE}
                </p>
              ) : null}
            </div>
          </aside>
        )}
      </div>
    </>
  )
}
