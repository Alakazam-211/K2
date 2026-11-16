// The live text socket as the editor's LiveDocSource reads it (build-editors.md
// E4a). The kit's editor (components/LiveEditor) consumes the transport and
// keeps the frames it knows; this tap hands the source what the editor doesn't
// pass on, without touching the editor:
//   - the welcome's K2 fields: `features` (["view_address"] on a text socket)
//     and `access_owner` (who can grant edit access);
//   - every server frame, so the source sees `outside_save` and `verb_refused`
//     (wire-shapes-k2-2026-11-15, "Text socket").
// One object across reconnects, like the transport it wraps.

import type { LiveTransport, LiveWelcome } from '@/lib/docs-live-transport'

/** `access_owner` in every live welcome: the box owner (shares name workspaces, not people). */
export interface AccessOwner {
  id: string
  name: string
  kind: string
  colour: string | null
}

export type TappedWelcome = LiveWelcome & { features?: string[]; access_owner?: AccessOwner | null }

export interface TransportTap {
  /** Hand this to `mountLiveEditor`. */
  readonly transport: LiveTransport
  onWelcome(cb: (w: TappedWelcome) => void): () => void
  /** Every frame after the welcome, as JSON (the kit's frames and K2's). */
  onFrame(cb: (f: Record<string, unknown>) => void): () => void
}

export function tapTransport(t: LiveTransport): TransportTap {
  const welcomes = new Set<(w: TappedWelcome) => void>()
  const transport: LiveTransport = {
    get open() {
      return t.open
    },
    async connect(o) {
      const w = await t.connect(o)
      for (const cb of [...welcomes]) cb(w as TappedWelcome)
      return w
    },
    send: (f) => t.send(f),
    onFrame: (cb) => t.onFrame(cb),
    onClose: (cb) => t.onClose(cb),
    close: () => t.close(),
  }
  return {
    transport,
    onWelcome(cb) {
      welcomes.add(cb)
      return () => void welcomes.delete(cb)
    },
    onFrame: (cb) => t.onFrame((f) => cb(f as unknown as Record<string, unknown>)),
  }
}
