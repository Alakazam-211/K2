import { useEffect, useState } from 'react'
import { daemonWsBase, getDaemonWs } from '@/kessel/daemon-ws'
import {
  applyChatTranscriptFrame,
  chatTranscriptWsUrl,
  parseChatTranscriptFrame,
  shouldOpenChatTranscriptSocket,
  type ChatTurn,
} from './chatTranscript'
import type { SessionViewTab } from './sessionViewTab'

/**
 * Tail socket for the chat face. `enabled` is only true while Chat is the
 * visible view. `retainWhileHidden` must not be passed here — a hidden pane
 * sets `visible` false and the effect cleanup closes the socket.
 */
export function useChatTranscript(opts: {
  view: SessionViewTab
  visible: boolean
  provider: string | null
  conversationId: string | null
  agentName: string | null
}): { turns: ChatTurn[]; error: string | null } {
  const { view, visible, provider, conversationId, agentName } = opts
  const [turns, setTurns] = useState<ChatTurn[]>([])
  const [error, setError] = useState<string | null>(null)
  const open = shouldOpenChatTranscriptSocket({ view, visible, provider, conversationId })

  useEffect(() => {
    if (!open || !provider || !conversationId?.trim()) {
      setTurns([])
      setError(null)
      return
    }
    let cancelled = false
    let ws: WebSocket | null = null
    const conversation = conversationId.trim()

    async function boot(): Promise<void> {
      try {
        const creds = await getDaemonWs()
        if (cancelled) return
        const url = chatTranscriptWsUrl(daemonWsBase(creds), creds.token, {
          provider: provider as string,
          conversationId: conversation,
          agentName,
        })
        ws = new WebSocket(url)
        if (cancelled) {
          ws.close()
          ws = null
          return
        }
        ws.onmessage = (ev) => {
          if (typeof ev.data !== 'string') return
          const frame = parseChatTranscriptFrame(ev.data)
          if (!frame) return
          setTurns((prev) => applyChatTranscriptFrame(prev, frame))
        }
        ws.onerror = () => {
          if (!cancelled) setError('Chat log disconnected')
        }
      } catch (err) {
        if (!cancelled) setError(err instanceof Error ? err.message : String(err))
      }
    }

    void boot()
    return () => {
      cancelled = true
      if (ws) {
        ws.close()
        ws = null
      }
    }
  }, [open, provider, conversationId, agentName])

  return { turns, error }
}
