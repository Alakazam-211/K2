import { useEffect, useRef, useState } from 'react'
import { daemonCliGet } from '@/lib/daemon-cli'
import { getDaemonWs, daemonWsBase } from '@/kessel/daemon-ws'
import {
  applyChatterFrame,
  chatterItemsFromSnapshot,
  releaseOverlayWebSocket,
  type OverlayThreadItem,
  type OverlayWsFrame,
} from './overlayThread'

export function useOverlayChatter(opts: {
  addr: string
  conversationId: string | null
  enabled: boolean
}): {
  items: OverlayThreadItem[]
  conversationId: string
  error: string | null
} {
  const { addr, conversationId, enabled } = opts
  const [items, setItems] = useState<OverlayThreadItem[]>([])
  const [resolvedConv, setResolvedConv] = useState(conversationId ?? '')
  const [error, setError] = useState<string | null>(null)
  const snapshotSeqRef = useRef(0)

  useEffect(() => {
    if (!enabled || !addr.trim()) {
      setItems([])
      setError(null)
      return
    }
    let cancelled = false
    let ws: WebSocket | null = null

    async function boot(): Promise<void> {
      try {
        const raw = await daemonCliGet<unknown>('chatter', { addr })
        if (cancelled) return
        const snap = chatterItemsFromSnapshot(raw)
        const conv = snap.conversation_id || conversationId || ''
        const lastSeq = snap.items.reduce((m, it) => (it.seq > m ? it.seq : m), 0)
        snapshotSeqRef.current = lastSeq
        setItems(snap.items)
        setResolvedConv(conv)
        setError(null)
        if (!conv) return

        const creds = await getDaemonWs()
        if (cancelled) return
        const url = `${daemonWsBase(creds)}/cli/overlay/events?conversation=${encodeURIComponent(conv)}&token=${encodeURIComponent(creds.token)}`
        ws = new WebSocket(url)
        if (cancelled) {
          releaseOverlayWebSocket(ws)
          ws = null
          return
        }
        ws.onmessage = (ev) => {
          const rawFrame = typeof ev.data === 'string' ? ev.data : null
          if (!rawFrame) return
          let frame: OverlayWsFrame
          try {
            frame = JSON.parse(rawFrame) as OverlayWsFrame
          } catch {
            return
          }
          setItems((prev) => applyChatterFrame(prev, frame, snapshotSeqRef.current))
        }
      } catch (e) {
        if (!cancelled) {
          setError(e instanceof Error ? e.message : String(e))
        }
      }
    }

    void boot()
    return () => {
      cancelled = true
      if (ws) releaseOverlayWebSocket(ws)
    }
  }, [addr, conversationId, enabled])

  return { items, conversationId: resolvedConv, error }
}
