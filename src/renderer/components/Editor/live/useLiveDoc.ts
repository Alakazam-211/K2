// The live doc behind the new editors (build-editors.md E4a): the kit's Live
// editor (components/LiveEditor, the same collab wiring today's LiveDocView
// mounts) over K2's socket through the transport tap, and the LiveDocSource
// that reads both. The editor lives in its own element for as long as the
// file tab does, so a mode or a Preview that unmounts the text's slot never
// drops the session; LiveSlot puts the element in place.
//
// The host's CodeMirror extensions (the look, the language, Write and Read,
// line numbers) sit in one compartment that follows the frame's mode; a
// snapshot reload rebuilds the editor's state, so they go back in after one.

import { useCallback, useEffect, useRef, useState } from 'react'
import { Compartment, type Extension } from '@codemirror/state'
import { mountLiveEditor, type LiveEditorHandle, type LiveTransport as KitLiveTransport } from '@/components/LiveEditor'
import { wireLiveHost } from '@/components/LiveEditor/host-wiring'
import type { ServerScope } from '@/kessel/server-scope'
import { daemonCliPost } from '@/lib/daemon-cli'
import { createDaemonLiveTransport } from '@/lib/docs-live-daemon'
import type { LiveTransport } from '@/lib/docs-live-transport'
import { LiveDocSource } from '@/lib/editor/live/live-doc-source'
import { tapTransport } from '@/lib/editor/live/transport-tap'

export interface UseLiveDocOptions {
  scope: ServerScope
  filePath: string
  markdown: boolean
  /** The core was turned off mid-session: the plain editor carries on. */
  onCoreOff: () => void
  /** Tests: the transport (default: the daemon's, through the dial queue). */
  createTransport?: (scope: ServerScope, doc: { path: string }) => LiveTransport
}

export interface LiveDoc {
  source: LiveDocSource
  /** The element the kit's editor lives in. */
  host: HTMLDivElement
  editor: () => LiveEditorHandle | null
  /** The host's extensions for the current mode. */
  configure: (ext: Extension) => void
  /** The removed-text notes were shown: the daemon delivers them. */
  takeNotes: () => void
  /** Reopen after a close. */
  reopen: () => void
  /** Tell the others what you're looking at (a heading's address), or nothing. */
  sendViewAddress: (address: string | null) => void
}

export function useLiveDoc({ scope, filePath, markdown, onCoreOff, createTransport }: UseLiveDocOptions): LiveDoc {
  // Say something goes through the file's say route (an agent also gets it in its inbox).
  const sayTo = useRef({ scope, filePath })
  sayTo.current = { scope, filePath }
  const [source] = useState(() => new LiveDocSource({ markdown, post: (body) => daemonCliPost(sayTo.current.scope, 'docs/say', { path: sayTo.current.filePath, ...body }) }))
  const [host] = useState(() => {
    const el = document.createElement('div')
    el.className = 'ed-live-host'
    el.dataset.testid = 'ed-live-host'
    return el
  })
  const [compartment] = useState(() => new Compartment())
  const ext = useRef<Extension>([])
  const editor = useRef<LiveEditorHandle | null>(null)
  const transport = useRef<LiveTransport | null>(null)
  const coreOff = useRef(onCoreOff)
  coreOff.current = onCoreOff
  const [mountKey, setMountKey] = useState(0)

  useEffect(() => {
    const t = (createTransport ?? createDaemonLiveTransport)(scope, { path: filePath })
    const tap = tapTransport(t)
    transport.current = t
    source.attachTap(tap)
    let ed: LiveEditorHandle
    try {
      ed = mountLiveEditor(host, {
        // K2's transport has the kit's shape (wire.ts `LiveTransport`), as in LiveDocView.
        transport: tap.transport as unknown as KitLiveTransport,
        // The host brings the language (Write and Read's own markdown, or the file's).
        mode: 'plain',
        extensions: compartment.of(ext.current),
        // The frame draws the connection, who's here and the notices.
        render: { banner: false, roster: false },
      })
    } catch (e) {
      console.error('[live-doc] the live editor could not start:', e)
      return
    }
    editor.current = ed
    source.attach(ed)
    const unwire = wireLiveHost(host, () => editor.current, { blocked: () => source.snapshot().undoOff !== null })
    const offs = [
      ed.on('reload', () => ed.view.dispatch({ effects: compartment.reconfigure(ext.current) })),
      ed.on('closed', (c) => {
        if (c.code === 'core_off') coreOff.current()
      }),
    ]
    return () => {
      for (const off of offs) off()
      unwire()
      transport.current = null
      if (editor.current === ed) editor.current = null
      ed.destroy()
    }
  }, [scope, filePath, source, host, compartment, createTransport, mountKey])

  useEffect(() => () => source.close(), [source])

  useEffect(() => {
    const on = (): void => source.setOnline(true)
    const off = (): void => source.setOnline(false)
    window.addEventListener('online', on)
    window.addEventListener('offline', off)
    return () => {
      window.removeEventListener('online', on)
      window.removeEventListener('offline', off)
    }
  }, [source])

  const configure = useCallback(
    (next: Extension) => {
      ext.current = next
      editor.current?.view.dispatch({ effects: compartment.reconfigure(next) })
    },
    [compartment],
  )
  const takeNotes = useCallback(() => transport.current?.send({ type: 'take_notes' }), [])
  const reopen = useCallback(() => setMountKey((k) => k + 1), [])
  const current = useCallback(() => editor.current, [])
  // The kit's presence frame (WIRE.md `view_address`): no reply unless refused.
  const req = useRef(0)
  const sendViewAddress = useCallback((address: string | null) => transport.current?.send({ type: 'view_address', req: ++req.current, address }), [])
  return { source, host, editor: current, configure, takeNotes, reopen, sendViewAddress }
}
