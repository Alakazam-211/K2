import { useCallback, useEffect, useRef, useState } from 'react'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { getDaemonWs, daemonWsBase } from '@/kessel/daemon-ws'
import {
  applyOverlayFrame,
  threadItemsFromSnapshot,
  type OverlayThreadItem,
  type OverlayWsFrame,
} from './overlayThread'

export function useOverlayThread(opts: {
  addr: string
  conversationId: string | null
  enabled: boolean
}): {
  items: OverlayThreadItem[]
  conversationId: string
  error: string | null
  posting: boolean
  post: (text: string) => Promise<void>
} {
  const { addr, conversationId, enabled } = opts
  const [items, setItems] = useState<OverlayThreadItem[]>([])
  const [resolvedConv, setResolvedConv] = useState(conversationId ?? '')
  const [error, setError] = useState<string | null>(null)
  const [posting, setPosting] = useState(false)
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
        const raw = await daemonCliGet<unknown>('thread', { addr })
        if (cancelled) return
        const snap = threadItemsFromSnapshot(raw)
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
        ws.onmessage = (ev) => {
          const rawFrame = typeof ev.data === 'string' ? ev.data : null
          if (!rawFrame) return
          let frame: OverlayWsFrame
          try {
            frame = JSON.parse(rawFrame) as OverlayWsFrame
          } catch {
            return
          }
          setItems((prev) => applyOverlayFrame(prev, frame, snapshotSeqRef.current))
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
      if (ws) {
        try {
          ws.onmessage = null
          ws.close()
        } catch {
          /* ignore */
        }
      }
    }
  }, [addr, conversationId, enabled])

  const post = useCallback(
    async (text: string) => {
      const trimmed = text.trim()
      if (!trimmed || !addr.trim()) return
      setPosting(true)
      try {
        const res = await daemonCliPost<{
          ok?: boolean
          id?: string
          seq?: number
          from?: string
          body?: string
          kind?: string
          conversation_id?: string
        }>('thread/post', { addr, text: trimmed, via: 'compose' })
        if (res?.id && typeof res.seq === 'number') {
          const item: OverlayThreadItem = {
            collection: 'thread',
            seq: res.seq,
            id: res.id,
            conversation_id: res.conversation_id,
            doc: {
              id: res.id,
              kind: res.kind || 'text',
              from: res.from || '',
              body: res.body ?? trimmed,
              via: 'compose',
            },
          }
          snapshotSeqRef.current = Math.max(snapshotSeqRef.current, res.seq)
          setItems((prev) =>
            prev.some((it) => it.id === item.id) ? prev : [...prev, item].sort((a, b) => a.seq - b.seq),
          )
          if (res.conversation_id) setResolvedConv(res.conversation_id)
        }
      } finally {
        setPosting(false)
      }
    },
    [addr],
  )

  return { items, conversationId: resolvedConv, error, posting, post }
}
