import { useCallback, useEffect, useRef, useState } from 'react'
import { daemonCliGet } from '@/lib/daemon-cli'
import { getDaemonWs, daemonWsBase } from '@/kessel/daemon-ws'
import {
  applyChatterFrame,
  chatterItemsFromSnapshot,
  mergeOlderOverlayItems,
  OVERLAY_PAGE_SIZE,
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
  hasMore: boolean
  loadOlder: () => Promise<void>
  loadingOlder: boolean
} {
  const { addr, conversationId, enabled } = opts
  const [items, setItems] = useState<OverlayThreadItem[]>([])
  const [resolvedConv, setResolvedConv] = useState(conversationId ?? '')
  const [error, setError] = useState<string | null>(null)
  const [hasMore, setHasMore] = useState(false)
  const [loadingOlder, setLoadingOlder] = useState(false)
  const snapshotSeqRef = useRef(0)
  const itemsRef = useRef(items)
  const hasMoreRef = useRef(false)
  const loadingOlderRef = useRef(false)
  const addrRef = useRef(addr)
  itemsRef.current = items
  hasMoreRef.current = hasMore
  loadingOlderRef.current = loadingOlder
  addrRef.current = addr

  useEffect(() => {
    if (!enabled || !addr.trim()) {
      setItems([])
      setError(null)
      setHasMore(false)
      setLoadingOlder(false)
      return
    }
    let cancelled = false
    let ws: WebSocket | null = null

    async function boot(): Promise<void> {
      try {
        const raw = await daemonCliGet<unknown>('chatter', { addr, limit: OVERLAY_PAGE_SIZE })
        if (cancelled) return
        const snap = chatterItemsFromSnapshot(raw)
        const conv = snap.conversation_id || conversationId || ''
        const lastSeq = snap.items.reduce((m, it) => (it.seq > m ? it.seq : m), 0)
        snapshotSeqRef.current = lastSeq
        setItems(snap.items)
        setHasMore(snap.has_more)
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
          if (frame.collection === 'chatter' && typeof frame.seq === 'number' && Number.isFinite(frame.seq)) {
            snapshotSeqRef.current = Math.max(snapshotSeqRef.current, frame.seq)
          }
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

  const loadOlder = useCallback(async () => {
    if (!addr.trim() || !hasMoreRef.current || loadingOlderRef.current) return
    const current = itemsRef.current
    if (current.length === 0) return
    const minSeq = current.reduce((m, it) => (it.seq < m ? it.seq : m), Number.POSITIVE_INFINITY)
    if (!Number.isFinite(minSeq)) return
    const requestedAddr = addr
    loadingOlderRef.current = true
    setLoadingOlder(true)
    try {
      const raw = await daemonCliGet<unknown>('chatter', {
        addr,
        limit: OVERLAY_PAGE_SIZE,
        before_seq: minSeq,
      })
      if (addrRef.current !== requestedAddr) return
      const snap = chatterItemsFromSnapshot(raw)
      setHasMore(snap.has_more)
      setItems((prev) => mergeOlderOverlayItems(prev, snap.items))
    } catch (e) {
      if (addrRef.current === requestedAddr) {
        setError(e instanceof Error ? e.message : String(e))
      }
    } finally {
      loadingOlderRef.current = false
      setLoadingOlder(false)
    }
  }, [addr])

  return { items, conversationId: resolvedConv, error, hasMore, loadOlder, loadingOlder }
}
