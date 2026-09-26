import { describe, expect, it } from 'vitest'
import { applyChatTranscriptFrame, chatTranscriptWsUrl, shouldOpenChatTranscriptSocket } from './chatTranscript'
import type { ChatTurn } from './chatTranscript'

const turn = (id: string, text: string): ChatTurn => ({
  id,
  role: 'user',
  blocks: [{ type: 'text', text }],
})

describe('chat transcript socket', () => {
  it('opens only for a visible chat view with a provider conversation id', () => {
    const base = {
      view: 'chat' as const,
      visible: true,
      provider: 'claude',
      conversationId: 'conv-1',
    }
    expect(shouldOpenChatTranscriptSocket(base)).toBe(true)
    expect(shouldOpenChatTranscriptSocket({ ...base, view: 'thread' })).toBe(false)
    expect(shouldOpenChatTranscriptSocket({ ...base, view: 'terminal' })).toBe(false)
    expect(shouldOpenChatTranscriptSocket({ ...base, visible: false })).toBe(false)
    expect(shouldOpenChatTranscriptSocket({ ...base, conversationId: null })).toBe(false)
    expect(shouldOpenChatTranscriptSocket({ ...base, conversationId: '  ' })).toBe(false)
    expect(shouldOpenChatTranscriptSocket({ ...base, provider: 'pi' })).toBe(false)
    expect(shouldOpenChatTranscriptSocket({ ...base, provider: null })).toBe(false)
  })

  it('does not put the transcript on grid, session-events, or overlay events', () => {
    const url = chatTranscriptWsUrl('ws://127.0.0.1:9', 'tok', {
      provider: 'codex',
      conversationId: 'sid',
      agentName: 'tab-1',
    })
    expect(url.startsWith('ws://127.0.0.1:9/cli/chat/transcript?')).toBe(true)
    expect(url.includes('/cli/sessions/grid')).toBe(false)
    expect(url.includes('/cli/sessions/events')).toBe(false)
    expect(url.includes('/cli/overlay/events')).toBe(false)
    expect(url.includes('.jsonl')).toBe(false)
    expect(url.includes('file=')).toBe(false)
    const params = new URL(url.replace('ws://', 'http://')).searchParams
    expect(params.get('provider')).toBe('codex')
    expect(params.get('conversation')).toBe('sid')
    expect(params.get('agent')).toBe('tab-1')
    expect(params.get('token')).toBe('tok')
  })

  it('replaces a turn when the same id arrives again', () => {
    const first = applyChatTranscriptFrame([], { kind: 'turn', turn: turn('msg1', 'Hi') })
    const second = applyChatTranscriptFrame(first, {
      kind: 'turn',
      turn: { id: 'msg1', role: 'assistant', blocks: [{ type: 'text', text: 'Hi' }, { type: 'tool_call', id: 't1', name: 'Bash', input: 'ls' }] },
    })
    expect(second).toHaveLength(1)
    expect(second[0].blocks).toHaveLength(2)
    const reset = applyChatTranscriptFrame(second, { kind: 'reset' })
    expect(reset).toEqual([])
  })
})
