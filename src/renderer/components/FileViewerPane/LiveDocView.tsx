// The Live view: a file whose multiplayer core is on (Settings → Cores →
// Multiplayer) opens in K2's Live editor (`mountLiveEditor`,
// components/LiveEditor; behaviour as vendor/k2-mpc-base/client/EDITOR.md
// describes) over K2's socket, `WS /cli/docs/live`,
// dialled through the server's queue (MS70, `docs-live-daemon.ts`).
//
// Everyone in the file sees each other's carets and edits as they happen;
// the server owns undo (Cmd/Ctrl+Z undoes only this person's own changes).
// The daemon writes the file; there is no Save here.
//
// What K2 adds around the editor:
//   - a status line: live with who else is here, connecting, reconnecting;
//   - the removed-text notes (the kit's `overwritten`: an outside save may
//     have removed this person's text): shown with Copy and Dismiss, then
//     delivered to the daemon (`take_notes`) so no later snapshot repeats them;
//   - a closed state: the core turned off hands the tab back to today's view
//     (`onEnded`); any other close (idle, removed, rights lost, renamed...)
//     says why and offers Reopen.

import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import type { Extension } from '@codemirror/state'
import {
  mountLiveEditor,
  type ConnectionState,
  type LiveEditorHandle,
  type LiveTransport as KitLiveTransport,
  type RemovedTextNote,
} from '@/components/LiveEditor'
import type { ServerScope } from '@/kessel/server-scope'
import { createDaemonLiveTransport } from '@/lib/docs-live-daemon'
import type { LiveTransport } from '@/lib/docs-live-transport'

/** How a Live view ended for good (the editor closed and won't reconnect). */
export interface LiveDocClosed {
  reason: string
  code?: string
  path?: string
}

export interface LiveDocViewProps {
  scope: ServerScope
  /** The file, absolute (the daemon finds its workspace). */
  filePath: string
  /** Markdown mode (highlighting, list keys); otherwise plain text. */
  markdown: boolean
  /** The host's CodeMirror extensions: a language and the theme (never a history). */
  extensions?: Extension
  /** The file's core was turned off: the tab goes back to today's view. */
  onCoreOff?: (closed: LiveDocClosed) => void
  /** Tests: the transport (default: the daemon's, through the dial queue). */
  createTransport?: (scope: ServerScope, doc: { path: string }) => LiveTransport
}

/** What the status line says. */
export function liveStatusText(state: ConnectionState, others: number): string {
  switch (state) {
    case 'connecting':
      return 'Connecting to the live doc…'
    case 'reconnecting':
      return 'Reconnecting… your edits are kept and sent when it’s back.'
    case 'closed':
      return 'Live editing stopped.'
    case 'live':
      return others === 0 ? 'Live · only you here' : `Live · ${others} other${others === 1 ? '' : 's'} here`
  }
}

/** Why a closed Live view stopped, in words. */
export function closedText(c: LiveDocClosed): string {
  switch (c.code === 'core_off' ? 'core_off' : c.reason) {
    case 'core_off':
      return 'Live editing was turned off for this kind of file (Settings → Cores → Multiplayer).'
    case 'idle':
      return 'Live editing paused: this tab was idle.'
    case 'kicked':
      return 'You were removed from this live doc.'
    case 'rights_lost':
      return 'You can no longer read this file.'
    case 'deleted':
      return 'This file’s live doc was deleted.'
    case 'moved':
      return c.path ? `This file was renamed to ${c.path}. Open it there.` : 'This file was renamed. Open it at its new name.'
    case 'declined':
      return 'This file can’t be edited live (too large, or not text K2 can edit live).'
    case 'not_found':
      return 'This file is gone.'
    case 'auth':
      return 'Sign in again to edit live.'
    case 'released':
      return 'The server is restarting. Reopen in a moment.'
    default:
      return 'Live editing stopped.'
  }
}

/** The text a note gives back: this person's own removed text, every place. */
export function noteText(n: RemovedTextNote): string {
  return n.cells.map((c) => c.yours).join('')
}

export function LiveDocView({ scope, filePath, markdown, extensions, onCoreOff, createTransport }: LiveDocViewProps): React.JSX.Element {
  const host = useRef<HTMLDivElement | null>(null)
  const transport = useRef<LiveTransport | null>(null)
  const [mountKey, setMountKey] = useState(0)
  const [state, setState] = useState<ConnectionState>('connecting')
  const [others, setOthers] = useState(0)
  const [closed, setClosed] = useState<LiveDocClosed | null>(null)
  const [notes, setNotes] = useState<RemovedTextNote[]>([])
  const [copied, setCopied] = useState<string | null>(null)
  const onCoreOffRef = useRef(onCoreOff)
  onCoreOffRef.current = onCoreOff

  useEffect(() => {
    const el = host.current
    if (!el) return
    setState('connecting')
    setClosed(null)
    setNotes([])
    const t = (createTransport ?? createDaemonLiveTransport)(scope, { path: filePath })
    transport.current = t
    let ed: LiveEditorHandle
    try {
      ed = mountLiveEditor(el, {
        // K2's transport has the kit's shape (wire.ts `LiveTransport`): one
        // attempt per connect, a connect while open replaces the socket,
        // `prev` retires the previous connection, frames dropped while down.
        transport: t as unknown as KitLiveTransport,
        mode: markdown ? 'markdown' : 'plain',
        ...(extensions ? { extensions } : {}),
      })
    } catch (e) {
      setClosed({ reason: 'declined', code: e instanceof Error ? e.message : String(e) })
      return
    }
    const offs = [
      ed.on('state', (s) => setState(s)),
      ed.on('roster', (entries) => setOthers(Math.max(0, entries.length - 1))),
      ed.on('note', (n) => setNotes((prev) => [...prev, n])),
      ed.on('closed', (c) => {
        const info: LiveDocClosed = { reason: c.reason, ...(c.code ? { code: c.code } : {}), ...(c.path ? { path: c.path } : {}) }
        setClosed(info)
        if (info.code === 'core_off') onCoreOffRef.current?.(info)
      }),
    ]
    return () => {
      for (const off of offs) off()
      transport.current = null
      ed.destroy()
    }
    // A new file, server or mode is a new editor; `mountKey` is Reopen.
  }, [scope, filePath, markdown, extensions, createTransport, mountKey])

  // The notes on screen are delivered: no later snapshot or `k2 doc` verb
  // repeats them (the editor already shows each (change, row) once).
  const shownNotes = notes.length
  useEffect(() => {
    if (shownNotes > 0) transport.current?.send({ type: 'take_notes' })
  }, [shownNotes])

  const copy = useCallback(async (n: RemovedTextNote) => {
    const text = noteText(n)
    try {
      await navigator.clipboard.writeText(text)
      setCopied(`${n.change}:${n.row ?? ''}`)
    } catch {
      setCopied(null)
    }
  }, [])

  const status = useMemo(() => (closed ? closedText(closed) : liveStatusText(state, others)), [closed, state, others])

  return (
    <div className="flex h-full min-h-0 flex-col" data-testid="live-doc-view" data-state={closed ? 'closed' : state}>
      <div
        className="flex shrink-0 items-center gap-2 border-b border-[var(--color-border)] px-3 py-1 text-xs text-[var(--color-text-muted)]"
        role="status"
        aria-live="polite"
      >
        <span
          className={`inline-block h-2 w-2 rounded-full ${
            closed ? 'bg-[var(--color-text-muted)]' : state === 'live' ? 'bg-green-500' : 'bg-yellow-500'
          }`}
          aria-hidden
        />
        <span data-testid="live-doc-status">{status}</span>
        {closed && closed.code !== 'core_off' && (
          <button
            type="button"
            className="ml-auto rounded border border-[var(--color-border)] px-2 py-0.5 text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)]"
            onClick={() => setMountKey((k) => k + 1)}
          >
            Reopen
          </button>
        )}
      </div>
      {notes.length > 0 && (
        <div className="shrink-0 space-y-2 border-b border-[var(--color-border)] px-3 py-2" data-testid="live-doc-notes">
          {notes.map((n) => {
            const key = `${n.change}:${n.row ?? ''}`
            return (
              <div key={key} className="rounded border border-yellow-500/40 bg-yellow-500/10 p-2 text-xs text-[var(--color-text-primary)]" role="alert">
                <div className="whitespace-pre-wrap break-words">{n.message}</div>
                <div className="mt-1 flex gap-2">
                  {noteText(n) && (
                    <button
                      type="button"
                      className="rounded border border-[var(--color-border)] px-2 py-0.5 hover:bg-[var(--color-bg-hover)]"
                      onClick={() => void copy(n)}
                    >
                      {copied === key ? 'Copied' : 'Copy my text'}
                    </button>
                  )}
                  <button
                    type="button"
                    className="rounded border border-[var(--color-border)] px-2 py-0.5 hover:bg-[var(--color-bg-hover)]"
                    onClick={() => setNotes((prev) => prev.filter((x) => `${x.change}:${x.row ?? ''}` !== key))}
                  >
                    Dismiss
                  </button>
                </div>
              </div>
            )
          })}
        </div>
      )}
      <div ref={host} className="min-h-0 flex-1 overflow-auto" data-testid="live-doc-editor" key={mountKey} />
    </div>
  )
}
