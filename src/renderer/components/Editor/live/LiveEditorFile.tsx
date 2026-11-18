// A live (multiplayer) file in the new editors (build-editors.md E4a): the
// same EditorTab (markdown Write | Read | Source, or code) over the kit's
// Live editor instead of a plain buffer. The daemon writes the file, so there
// is no save model: the status line says "All changes saved" while live,
// "Saved on this computer" while disconnected, "Merged · …" after a save
// outside K2 merged in.
//
// What the live shell adds through the frame (FrameLiveContext): the Live chip
// (only when someone else is here, an agent counts), the sentence ("Atlas is
// editing Setup"), the avatar stack (you first, then who's here), and the
// Live panel. Under the header, one notice at a time (pick-notice.ts): the
// connection, the merged save with Compare, Undo their save (with its
// confirm) and Use their version, the file missing or paused, a removed-text
// note, read only. A refusal is a Changes row with "Ask <holder>".
//
// Replaces LiveDocView's status line and notes for the new editors; today's
// view keeps LiveDocView until the old path retires. Presence on the text
// (E4b: name flags, holds, the off-screen band, Follow, Say something) is
// usePresence: its extensions go in the text's slot, its header pieces in
// the frame.

import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState } from 'react'
import type { ShellConfig } from '@/components/LiveView/shell/shell-config'
import { useLiveShell, useTabWidth } from '@/components/LiveView/shell/useLiveShell'
import type { ServerScope } from '@/kessel/server-scope'
import { daemonCliPost } from '@/lib/daemon-cli'
import type { LiveTransport } from '@/lib/docs-live-transport'
import type { LiveDocClose, LiveDocSnapshot } from '@/lib/editor/live/live-doc-source'
import { liveNoticeView, readOnlyReason, type LiveActionId } from '@/lib/editor/live/live-notices'
import { LIVE_UI, liveReadOnly, liveSaveStatus, pickLiveNotice, showsMerged, type LiveUi, type SaveFlow } from '@/lib/editor/live/pick-notice'
import { replaceText } from '@/lib/editor/live/replace'
import { readLocalFlag } from '@/lib/editor/local-flag'
import type { EditorKind } from '@/lib/editor/preview-switch'
import { failureReason } from '@/lib/editor/save-model/write'
import type { LiveChange } from '@/lib/live-view/shared/source'
import { watching } from '@/lib/live-view/shared/people'
import { useSettingsStore } from '@/stores/settings'
import { EDITOR_NARROW_BELOW, fileNameOf } from '../EditorFrame'
import { EditorTab } from '../EditorTab'
import { FrameLiveContext, type FrameLive } from '../frameLive'
import { FrameSaveContext, type FrameSave } from '../frameSave'
import { SaveNoticeBar } from '../SaveNoticeBar'
import { SAVE_CSS } from '../saveCss'
import { AskBar, type Ask } from './AskBar'
import { LIVE_CSS } from './liveCss'
import { LivePanel, READ_ALOUD_KEY } from './LivePanel'
import { LiveSlot } from './LiveSlot'
import { PRESENCE_CSS } from './presenceCss'
import { useLiveDoc } from './useLiveDoc'
import { useBeforeText } from './useBeforeText'
import { usePresence } from './usePresence'

const CompareView = lazy(() => import('../CompareView').then((m) => ({ default: m.CompareView })))

/** The shell's config for the editor: its own layout, so only what the hook reads. */
const EDITOR_SHELL: ShellConfig = {
  kind: 'editor',
  kindLabel: '',
  fileGlyph: null,
  bannerLine: '',
  panel: [],
  narrowBelow: EDITOR_NARROW_BELOW,
  sentenceRowWhenNarrow: false,
  backInHeader: true,
  sayAlways: false,
  kept: 'who’s here and the changes',
  // A message points at the section its person is in: the chip says the heading's words ("Setup"),
  // the wire carries the kit's address (`@h:setup`, or `L12` outside any heading).
  sayAddress: (snap, to) => {
    const p = snap.people.find((x) => x.id === to)
    const place = p?.place
    const address = (snap as LiveDocSnapshot).addrs?.get(to) ?? null
    if (!place || !address) return null
    return { label: place, title: `${place}, where ${p.name} is`, address }
  },
  Hero: () => <></>,
}

/** Your edits out this long read "Saving" in the status line. */
const SLOW_SAVE_MS = 1500

/** The chip shows these even when you're alone. */
const ALWAYS_CHIP = new Set(['reconnecting', 'cant', 'offline', 'paused', 'missing'])

export interface LiveEditorFileProps {
  scope: ServerScope
  filePath: string
  kind: EditorKind
  /** A view-only room: why nothing here can be edited. */
  roomReadOnly: string | null
  /** The core was turned off mid-session: the plain editor carries on. */
  onCoreOff: () => void
  /** A file with its own preview (CSV, HTML…): drawn from the live text, or
   *  from `null` while there is none (before the welcome) or the session
   *  ended (any close, any reason): the host then draws the file as it read
   *  it. An ended or refused live session never blanks the preview. */
  renderPreview?: (text: string | null) => React.ReactNode
  /** The daemon declined this file (`closed {declined}`), before the welcome
   *  (a protected folder, a secret, not text…) or after it, from any server
   *  version: the host opens it the usual way and says why. Without it the
   *  closed notice stays. */
  onRefused?: (closed: LiveDocClose) => void
  width?: number
  createTransport?: (scope: ServerScope, doc: { path: string }) => LiveTransport
}

const linesOf = (c: LiveChange): { first: number; last: number } | null => {
  const m = c.target ? /^lines:(\d+)-(\d+)$/.exec(c.target) : null
  return m ? { first: Number(m[1]), last: Number(m[2]) } : null
}

export function LiveEditorFile({ scope, filePath, kind, roomReadOnly, onCoreOff, renderPreview, onRefused, width, createTransport }: LiveEditorFileProps): React.JSX.Element {
  const markdown = kind === 'markdown'
  const doc = useLiveDoc({ scope, filePath, markdown, onCoreOff, createTransport })
  const [outer, tabWidth] = useTabWidth(width)
  const shell = useLiveShell(doc.source, EDITOR_SHELL, tabWidth)
  const snap = shell.snap as LiveDocSnapshot
  const presence = usePresence({ doc, shell, snap, markdown, root: outer })
  const before = useBeforeText(scope, filePath, snap)
  const name = fileNameOf(filePath)

  const [text, setText] = useState('')
  const [cursor, setCursor] = useState({ line: 1, col: 1 })
  useEffect(() => {
    const t = doc.editor()?.text
    if (t !== undefined) setText(t)
  }, [snap, doc])
  const onUpdate = useCallback((t: string, c: { line: number; col: number }) => {
    setText(t)
    setCursor(c)
  }, [])
  // The live text exists only once the daemon welcomed this tab, and only
  // until the session closes: after that the file as read is the truth.
  const closed = snap.closed
  const liveText = snap.tier === 'live' && !closed ? text : null
  const refusedRef = useRef(onRefused)
  refusedRef.current = onRefused
  useEffect(() => {
    if (closed && closed.reason === 'declined') refusedRef.current?.(closed)
  }, [closed])

  // What you did about the notices (the data stays in the source).
  const [ui, setUi] = useState<LiveUi>(LIVE_UI)
  const patch = (p: Partial<LiveUi>): void => setUi((u) => ({ ...u, ...p }))
  const back = snap.connection === 'back'
  useEffect(() => {
    if (!back) setUi((u) => (u.backDismissed ? { ...u, backDismissed: false } : u))
  }, [back])
  useEffect(() => setUi((u) => ({ ...u, noteDismissed: false, noteCopied: false })), [snap.note])
  const saveAt = snap.outsideSave?.at_ms ?? null
  useEffect(() => setUi((u) => ({ ...u, comparing: false })), [saveAt])
  const flow = (step: SaveFlow['step'], reason?: string): void => {
    if (saveAt !== null) setUi((u) => ({ ...u, flow: { at: saveAt, step, ...(reason ? { reason } : {}) } }))
  }

  const notice = pickLiveNotice(snap, ui)
  // A removed-text note on screen is delivered: no later snapshot repeats it.
  const noteShown = notice?.kind === 'note'
  useEffect(() => {
    if (noteShown) doc.takeNotes()
  }, [noteShown, snap.note, doc])

  // Each finished edit by someone else, once: the live region, and aloud when asked.
  const announced = snap.announce
  useEffect(() => {
    if (!announced || !readLocalFlag(READ_ALOUD_KEY)) return
    const speech = typeof window !== 'undefined' ? window.speechSynthesis : undefined
    if (speech && typeof SpeechSynthesisUtterance !== 'undefined') speech.speak(new SpeechSynthesisUtterance(announced.text))
  }, [announced])

  // The panel opens by itself the first time someone else is here (wide tabs).
  const [panelOpen, setPanelOpen] = useState(false)
  const opened = useRef(false)
  useEffect(() => {
    if (opened.current || snap.othersHere === 0 || shell.narrow) return
    opened.current = true
    setPanelOpen(true)
  }, [snap.othersHere, shell.narrow])

  const [ask, setAsk] = useState<Ask | null>(null)
  const openSettings = useSettingsStore((s) => s.openSettings)

  const takeTheirs = (): void => {
    const os = snap.outsideSave
    daemonCliPost(scope, 'docs/use-theirs', { path: filePath, ...(os?.outcome === 'ingested' ? { rev: os.rev } : {}) }).then(
      () => {
        patch({ comparing: false })
        flow('used')
      },
      (e: unknown) => {
        const gone = (e as { status?: number; code?: string | null }).status === 409 && (e as { code?: string | null }).code === 'theirs_gone'
        if (gone) flow('gone')
        else flow('failed', failureReason(e))
      },
    )
  }

  const saveAgain = (): void => {
    const missing = snap.outsideSave?.outcome === 'missing'
    daemonCliPost(scope, 'fs/write-file', { path: filePath, content: doc.editor()?.text ?? text, ...(missing ? { expect: 'absent' } : {}) }).then(
      () => doc.source.clearOutsideSave(),
      (e: unknown) => flow('failed', failureReason(e)),
    )
  }

  const onAction = (id: LiveActionId | 'dismiss'): void => {
    const ed = doc.editor()
    switch (id) {
      case 'retry':
        return doc.source.retry()
      case 'reopen':
        return doc.reopen()
      case 'compare':
      case 'retry-before':
        // The earlier text: seen on this connection, else read from the save's stored version.
        if (id === 'retry-before') flow('merged')
        patch({ comparing: true })
        void before.get().then((t) => {
          if (t !== null) return
          patch({ comparing: false })
          flow('load-failed')
        })
        return
      case 'close-compare':
        return patch({ comparing: false })
      case 'undo-theirs':
        return flow('confirm')
      case 'cancel':
        return flow('merged')
      case 'confirm-undo-theirs':
        // One edit of yours (replace.ts), so Undo can reverse it; not docs/restore.
        void before.get().then((t) => {
          const view = doc.editor()?.view
          patch({ comparing: false })
          if (t === null || !view) return flow('before-gone')
          replaceText(view, t)
          flow('undid')
        })
        return
      case 'use-theirs':
        return takeTheirs()
      case 'redo':
        ed?.redo()
        return flow('merged')
      case 'undo':
        ed?.undo()
        return flow('merged')
      case 'save-again':
        return saveAgain()
      case 'copy-note': {
        const t = snap.note?.cells.map((c) => c.yours).join('') ?? ''
        void navigator.clipboard?.writeText(t).then(() => patch({ noteCopied: true }), () => undefined)
        return
      }
      case 'open-settings':
        return openSettings('multiplayer')
      case 'dismiss':
        if (notice?.kind === 'connection') return patch({ backDismissed: true })
        if (notice?.kind === 'note') {
          doc.source.dismissNote()
          return patch({ noteDismissed: true })
        }
        if (notice?.kind === 'read-only') return patch({ readOnlyDismissed: true })
        patch({ comparing: false })
        return flow('dismissed')
    }
  }

  const merged = showsMerged(snap, ui)
  // "Saving" only when your edits wait more than a moment (else every keystroke would flicker it).
  const [slow, setSlow] = useState(false)
  const waiting = snap.pending > 0
  useEffect(() => {
    setSlow(false)
    if (!waiting) return
    const t = setTimeout(() => setSlow(true), SLOW_SAVE_MS)
    return () => clearTimeout(t)
  }, [waiting])
  const status = liveSaveStatus(slow ? snap : { ...snap, pending: 0 }, merged)
  const comparing = ui.comparing && (before.text !== null || before.status === 'loading')
  const frameSave = useMemo<FrameSave>(
    () => ({
      status,
      // The daemon writes the file: ⌘S and leaving have nothing to save.
      saveNow: () => undefined,
      onLeave: () => undefined,
      replaceBody: comparing ? (
        before.text === null ? (
          <div data-testid="ed-compare-loading" role="status" className="type-row flex flex-1 items-center justify-center text-[var(--color-text-secondary)]">
            Loading the text from before that save…
          </div>
        ) : (
          <Suspense fallback={null}>
            <CompareView before={before.text} now={text} width={width} />
          </Suspense>
        )
      ) : null,
    }),
    [status, comparing, before.text, text, width],
  )

  const owner = snap.accessOwner
  const readOnly = roomReadOnly ?? liveReadOnly(snap, readOnlyReason)
  const tabState = snap.rights === 'read' ? 'read-only-file' : shell.tabState
  const showChip = snap.tier === 'live' && !snap.closed && (snap.othersHere > 0 || snap.rights === 'read' || ALWAYS_CHIP.has(shell.tabState))
  const frameLive = useMemo<FrameLive>(
    () => ({
      state: showChip ? tabState : undefined,
      sentence: shell.offline ? null : (presence.sentence ?? shell.sentence),
      sentenceEnd: shell.offline ? null : presence.sentenceEnd,
      fitKey: presence.fitKey,
      right: presence.right,
      panel: (
        <LivePanel
          snap={snap}
          people={shell.panelPeople}
          onGoThere={(c) => {
            const l = linesOf(c)
            if (l) doc.editor()?.reveal(l)
          }}
          onUndo={(c) => doc.source.undoChange(c.id)}
          onRedo={(c) => doc.source.redoChange(c.id)}
          onAsk={(c) => c.ask && setAsk({ to: c.ask, what: snap.refusal?.target ?? '' })}
          followingId={shell.followingId}
          onFollow={presence.follow}
        />
      ),
      panelOpen,
      onTogglePanel: () => setPanelOpen((o) => !o),
      onClosePanel: () => setPanelOpen(false),
      panelLabel: 'Live',
      offline: shell.offline,
      here: watching(shell.panelPeople).count,
      hereStale: snap.connection !== null && snap.connection !== 'back',
    }),
    [showChip, tabState, shell.offline, shell.sentence, shell.panelPeople, shell.followingId, presence.sentence, presence.sentenceEnd, presence.fitKey, presence.right, presence.follow, snap, panelOpen, doc],
  )

  const view = notice ? liveNoticeView(notice, name) : null
  // Compare needs the text from before the save (seen on this connection).
  const shown = view && !before.available ? { ...view, actions: view.actions.filter((a) => a.id !== 'compare' && a.id !== 'undo-theirs') } : view

  return (
    <div ref={outer} className="flex h-full min-h-0 flex-col" data-testid="ed-live-file" data-live-state={snap.closed ? 'closed' : (snap.connection ?? snap.tier)} data-owner={owner ?? undefined}>
      <style>{SAVE_CSS + LIVE_CSS + PRESENCE_CSS + presence.css}</style>
      <FrameSaveContext.Provider value={frameSave}>
        <FrameLiveContext.Provider value={frameLive}>
          <EditorTab
            kind={kind}
            filePath={filePath}
            text={text}
            dirty={false}
            saving={false}
            readOnly={readOnly}
            cursor={cursor}
            width={width}
            notice={
              <>
                {ask ? <AskBar scope={scope} filePath={filePath} ask={ask} onClose={() => setAsk(null)} /> : null}
                {shown && notice ? <SaveNoticeBar<LiveActionId> view={shown} kind={`live:${notice.kind}${notice.kind === 'connection' ? `:${notice.state}` : ''}`} onAction={onAction} /> : null}
                <div role="status" aria-live="polite" className="sr-only" data-testid="ed-live-announce">
                  {announced ? <span key={announced.id}>{announced.text}</span> : null}
                </div>
              </>
            }
            renderEditor={(o) => (
              <LiveSlot
                host={doc.host}
                filePath={filePath}
                opts={o}
                configure={doc.configure}
                onUpdate={onUpdate}
                presence={presence.extension}
                onConfigured={presence.syncHolds}
                top={presence.top}
                bottom={presence.bottom}
                foot={presence.say}
                ring={presence.ring}
              />
            )}
            renderPreview={renderPreview ? () => renderPreview(liveText) : undefined}
          />
        </FrameLiveContext.Provider>
      </FrameSaveContext.Provider>
    </div>
  )
}

export default LiveEditorFile
