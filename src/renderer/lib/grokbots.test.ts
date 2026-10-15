// Local Grok Bots (0.45.7): wire parsing, Home addressing and row status.
import { describe, expect, it } from 'vitest'
import {
  grokAgentState,
  grokBotHomeHandle,
  grokBotIdFromAddress,
  grokK2Sender,
  grokRowStatus,
  isGrokBotAddress,
  mergeGrokMessages,
  parseGrokList,
  parseGrokThread,
  type GrokBotAgent,
  type GrokMessage,
  type GrokHostSnapshot,
} from './grokbots'
import { homeAddress } from './home-address'

const ID = '0e5f5de8-7619-4ba4-9753-32c5470b2346'

const agent = (over: Partial<GrokBotAgent> = {}): GrokBotAgent => ({
  id: ID,
  name: 'Cortana',
  title: null,
  avatar: null,
  color: null,
  source: 'gateway',
  working: false,
  needsYou: false,
  unreadCount: 0,
  ...over,
})

const snap = (agents: GrokBotAgent[], over: Partial<GrokHostSnapshot['list']> = {}): GrokHostSnapshot => ({
  list: {
    enabled: true,
    sendEnabled: false,
    installed: true,
    status: { reachable: true, busy: false, activeAgentId: null },
    agents,
    ...over,
  },
  error: null,
  loading: false,
})

describe('parsing', () => {
  it('reads the list defensively and keeps only data: avatars', () => {
    const l = parseGrokList({
      enabled: true,
      sendEnabled: true,
      installed: true,
      status: { reachable: true, busy: true, activeAgentId: ID },
      agents: [
        { id: ID, kind: 'grokbot', name: 'Cortana', avatar: 'data:image/png;base64,AA', source: 'gateway', working: true },
        { id: 'x', name: 'Bad', avatar: 'file:///etc/passwd', source: 'disk' },
        { name: 'no id' },
      ],
    })
    expect(l.enabled && l.sendEnabled && l.status.busy).toBe(true)
    expect(l.agents.map((a) => a.id)).toEqual([ID, 'x'])
    expect(l.agents[0].avatar).toBe('data:image/png;base64,AA')
    expect(l.agents[1].avatar).toBeNull()
    expect(l.agents[1].source).toBe('disk')
  })

  it('reads thread pages, cards and never a secret value', () => {
    const p = parseGrokThread({
      supported: true,
      nextSince: 9,
      messages: [
        { seq: 1, author: 'human', text: 'hi', atMs: 1, viaK2: true },
        {
          seq: 2,
          author: 'agent',
          text: '',
          openInApp: 'secret request',
          card: { type: 'secret-request', label: 'Bot token', platform: 'x', provided: true, value: 'LEAK' },
        },
        { seq: 3, author: 'agent', text: '', card: { type: 'template-share', name: 'Demo', shareUrl: 'javascript:alert(1)' } },
        { author: 'agent', text: 'no seq' },
      ],
    })
    expect(p.messages.map((m) => m.seq)).toEqual([1, 2, 3])
    expect(p.messages[0].viaK2).toBe(true)
    expect(JSON.stringify(p.messages[1].card)).not.toContain('LEAK')
    expect(p.messages[2].card).toEqual({ type: 'template-share', name: 'Demo', body: null, shareUrl: null })
    expect(parseGrokThread({ supported: false, reason: 'odd' }).supported).toBe(false)
  })

  it('merges pages by seq, a later read replaces the line', () => {
    const a = parseGrokThread({ messages: [{ seq: 1, author: 'agent', text: 'draft' }, { seq: 2, author: 'human', text: 'x' }] })
    const b = parseGrokThread({ messages: [{ seq: 1, author: 'agent', text: 'final' }, { seq: 3, author: 'agent', text: 'y' }] })
    expect(mergeGrokMessages(a.messages, b.messages).map((m) => `${m.seq}:${m.text}`)).toEqual(['1:final', '2:x', '3:y'])
  })
})

describe('Home addressing', () => {
  it('round-trips grokbot-<id>::host and never matches a workspace handle', () => {
    const addr = homeAddress(grokBotHomeHandle(ID.toUpperCase()), 'z3.k2.dev')
    expect(addr).toBe(`grokbot-${ID}::z3.k2.dev`)
    expect(grokBotIdFromAddress(addr)).toBe(ID)
    expect(isGrokBotAddress('atlas::local')).toBe(false)
    expect(isGrokBotAddress('grokbot-../x::local')).toBe(false)
  })
})

describe('status', () => {
  it('maps per-agent and box-wide busy to row words', () => {
    const st = { reachable: true, busy: false, activeAgentId: null }
    expect(grokAgentState(agent(), st)).toBe('idle')
    expect(grokAgentState(agent({ working: true }), st)).toBe('working')
    expect(grokAgentState(agent(), { ...st, busy: true, activeAgentId: ID })).toBe('working')
    expect(grokAgentState(agent({ needsYou: true, working: true }), st)).toBe('needs-you')
    expect(grokAgentState(agent({ source: 'disk' }), st)).toBe('not-running')
  })

  it('gives Home rows a status; the server state wins for other servers', () => {
    expect(grokRowStatus(undefined, ID, null).kind).toBe('checking')
    expect(grokRowStatus(snap([agent({ working: true })]), ID, null).kind).toBe('working')
    expect(grokRowStatus(snap([agent({ needsYou: true })]), ID, null).label).toBe('Needs you')
    expect(grokRowStatus(snap([]), ID, null).kind).toBe('not-found')
    expect(grokRowStatus(snap([agent()], { enabled: false }), ID, null).label).toBe('Off')
    expect(grokRowStatus({ list: null, error: '404', loading: false }, ID, null).kind).toBe('not-found')
    const offline = { kind: 'offline' as const, label: 'Offline', people: [] }
    expect(grokRowStatus(snap([agent()]), ID, offline)).toBe(offline)
  })
})

describe('grokK2Sender', () => {
  const msg = (over: Partial<GrokMessage>): GrokMessage => ({
    seq: 1,
    author: 'human',
    text: '',
    atMs: null,
    toAgent: null,
    fromAgent: null,
    viaK2: true,
    openInApp: null,
    card: null,
    ...over,
  })

  it('reads the sender from a K2 send and strips the stamp', () => {
    expect(grokK2Sender(msg({ text: '[from Rosson Test] ship it' }))).toEqual({ from: 'Rosson Test', text: 'ship it' })
    // Only K2's leading stamp comes off; a typed one stays as text.
    expect(grokK2Sender(msg({ text: '[from pat] [from Rosson] do it' }))).toEqual({
      from: 'pat',
      text: '[from Rosson] do it',
    })
    expect(grokK2Sender(msg({ text: '[from pat] line one\nline two' }))).toEqual({
      from: 'pat',
      text: 'line one\nline two',
    })
  })

  it('leaves lines K2 did not send, agent lines and unstamped lines alone', () => {
    expect(grokK2Sender(msg({ viaK2: false, text: '[from pat] hi' }))).toEqual({ from: null, text: '[from pat] hi' })
    expect(grokK2Sender(msg({ author: 'agent', text: '[from pat] hi' }))).toEqual({ from: null, text: '[from pat] hi' })
    expect(grokK2Sender(msg({ text: 'hello' }))).toEqual({ from: null, text: 'hello' })
    expect(grokK2Sender(msg({ text: '[from pat]hi' }))).toEqual({ from: null, text: '[from pat]hi' })
    expect(grokK2Sender(msg({ text: `[from ${'x'.repeat(65)}] hi` })).from).toBeNull()
  })
})
