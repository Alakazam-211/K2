// The Desktop tab's socket (prd-remote-desktop-tab-v1 §6; vs-live R20):
//
//   POST /cli/desktop/ticket {display?}           → {ticket: "k2dt_…", expiresIn: 60}
//   GET  /cli/desktop/stream?ticket=…&codecs=…&win=3[&display=][&fps=][&view=1][&stats=1][&pw=]
//
//  - A fresh single-use ticket for every dial (and every reconnect). Only
//    the ticket rides the URL: never a token, on a local or a remote scope.
//  - `codecs=` is what this viewer decodes (`isConfigSupported`, JPEG by
//    feature check): H.264 and VP8 where WebCodecs says yes, else JPEG.
//  - Binary: the engine's K2SF / K2SV (`Stream/stream-wire.ts`; codec byte
//    2 = VP8). Text: `desktop-wire.ts`'s `ServerMsg`.
//  - A refusal before the upgrade can't be read by a browser (the socket
//    just fails); the ticket call already carries the daemon's reason for
//    the common ones (not allowed, same machine, off).

import type { ServerScope } from '@/kessel/server-scope'
import { daemonWsBase } from '@/kessel/daemon-ws'
import { daemonCliPost } from '@/lib/daemon-cli'
import { DaemonHttpError } from '@/lib/daemon-http-error'
import { openQueuedWebSocket } from '@/lib/grid-dial-queue'
import { readBinaryFrame, type DecodeSupport, type StreamFrame } from '@/components/Stream/stream-wire'
import { desktopStreamUrl, closeReasonText, parseServerMsg, serializeViewerMsg, type Codec, type ServerMsg, type ViewerMsg } from './desktop-wire'

export type DesktopConnState = 'connecting' | 'open' | 'closed' | 'error'

export interface DesktopStreamHandlers {
  /** `code` is the close code (`closed`), or the ticket's HTTP status
   *  (`error` before dialing). */
  onConn(state: DesktopConnState, reason?: string, code?: number): void
  onServer(m: ServerMsg): void
  onFrame(f: StreamFrame): void
}

export interface DesktopStreamOptions {
  scope: ServerScope
  /** What this viewer decodes (the player's probe). */
  decode: DecodeSupport
  window: number
  /** The desktop to show (prd-linux-virtual-desktop-v1 §3.1); null: This
   *  server (the daemon's own pick). */
  desktop?: string | null
  display?: string | null
  view?: boolean
  stats?: boolean
  pw?: number
  handlers: DesktopStreamHandlers
}

export interface DesktopStreamConnection {
  send(m: ViewerMsg): void
  ack(seq: number): void
  close(): void
}

export type DesktopStreamConnector = (opts: DesktopStreamOptions) => DesktopStreamConnection

/** The `codecs=` list for a probe: H.264 then VP8 where this viewer decodes
 *  them, JPEG last when it can draw JPEG (R13). Never empty. */
export function desktopOffer(d: DecodeSupport): Codec[] {
  const out: Codec[] = []
  if (d.h264) out.push('h264')
  if (d.vp8) out.push('vp8')
  if (d.jpeg || out.length === 0) out.push('jpeg')
  return out
}

/** The ticket refusal's line for the pane. */
export function ticketRefusalText(err: unknown): string {
  if (err instanceof DaemonHttpError) {
    switch (err.code) {
      case 'not_allowed':
        return "You don't have access to this computer's screen."
      case 'same_machine':
        return "You're on this computer."
      case 'desktop_off':
        return 'Remote desktop is off on this computer.'
      case 'desktop_logins_only':
        return 'Sign in to this computer from another device to watch its screen.'
      case 'audit_unavailable':
        return "K2 couldn't write the access log, so it didn't connect you."
    }
    return err.message
  }
  return err instanceof Error ? err.message : String(err)
}

export const wsDesktopStream: DesktopStreamConnector = ({ scope, decode, window: win, desktop, display, view, stats, pw, handlers }) => {
  let ws: WebSocket | null = null
  let closed = false
  const pending: string[] = []
  handlers.onConn('connecting')
  const sendRaw = (text: string): void => {
    if (ws && ws.readyState === WebSocket.OPEN) ws.send(text)
    else if (!closed) pending.push(text)
  }
  void (async () => {
    let ticket: string
    try {
      const r = await daemonCliPost<{ ticket: string }>(scope, 'desktop/ticket', {
        ...(desktop ? { desktop } : {}),
        ...(display ? { display } : {}),
      })
      ticket = r.ticket
    } catch (err) {
      if (!closed) handlers.onConn('error', ticketRefusalText(err), err instanceof DaemonHttpError ? err.status : undefined)
      return
    }
    try {
      if (closed) return
      const creds = await scope.creds()
      if (closed) return
      const url = desktopStreamUrl(daemonWsBase(creds), { ticket, codecs: desktopOffer(decode), window: win, display, view, stats, pw })
      const sock = await openQueuedWebSocket(scope, url)
      if (closed) {
        sock.close()
        return
      }
      sock.binaryType = 'arraybuffer'
      ws = sock
      const opened = (): void => {
        if (closed) return
        handlers.onConn('open')
        for (const p of pending.splice(0)) sock.send(p)
      }
      if (sock.readyState === WebSocket.OPEN) opened()
      else sock.onopen = opened
      sock.onmessage = (ev: MessageEvent) => {
        if (closed) return
        if (typeof ev.data === 'string') {
          const m = parseServerMsg(ev.data)
          if (m) handlers.onServer(m)
          else console.warn('[desktop] stream: a text message this tab cannot read:', ev.data.slice(0, 120))
          return
        }
        const r = ev.data instanceof ArrayBuffer ? readBinaryFrame(ev.data) : ({ error: 'header' } as const)
        if ('error' in r) {
          console.warn(r.error === 'video' ? '[desktop] stream: a K2SV message this tab cannot read' : '[desktop] stream: a binary message without a frame header')
          return
        }
        handlers.onFrame(r.frame)
      }
      sock.onerror = () => {
        if (!closed) handlers.onConn('error', "Couldn't reach this computer's screen.")
      }
      sock.onclose = (ev: CloseEvent) => {
        if (closed) return
        handlers.onConn('closed', closeReasonText(ev.code, ev.reason), ev.code)
      }
    } catch (err) {
      if (!closed) handlers.onConn('error', err instanceof Error ? err.message : String(err))
    }
  })()
  return {
    send(m) {
      sendRaw(serializeViewerMsg(m))
    },
    ack(seq) {
      if (ws && ws.readyState === WebSocket.OPEN) ws.send(serializeViewerMsg({ t: 'ack', seq }))
    },
    close() {
      closed = true
      pending.length = 0
      ws?.close()
      ws = null
    },
  }
}
