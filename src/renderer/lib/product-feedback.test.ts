import { describe, expect, it } from 'vitest'
import {
  CONTACT_ADDRESS,
  feedbackAsText,
  onlyDeletesLines,
  parseItem,
  parseSettings,
  refusalMessage,
  stateChip,
  utf8Bytes,
} from './product-feedback'

describe('product-feedback client', () => {
  it('parses the settings answer defensively', () => {
    const s = parseSettings({
      enabled: true,
      agents: false,
      agentDiagnostics: false,
      rememberDiagnostics: null,
      canEdit: true,
      canSend: false,
      why: 'needs_connect',
      message: 'pair first',
      endpoint: 'https://feedback.akzm.k2.dev',
      pending: 2,
      service: { state: 'unreachable', message: "K2's feedback service isn't reachable yet." },
    })
    expect(s).toEqual({
      enabled: true,
      agents: false,
      agentDiagnostics: false,
      rememberDiagnostics: null,
      canEdit: true,
      canSend: false,
      why: 'needs_connect',
      message: 'pair first',
      endpoint: 'https://feedback.akzm.k2.dev',
      pending: 2,
      service: { state: 'unreachable', message: "K2's feedback service isn't reachable yet." },
    })
    expect(parseSettings(null).canSend).toBe(false)
    expect(parseSettings({ service: { state: 'weird' } }).service.state).toBe('unknown')
  })

  it('parses a row and drops one without an id', () => {
    expect(parseItem({ text: 'x' })).toBeNull()
    const it = parseItem({ id: 'pf_1', state: 'refused', category: 'nope', senderKind: 'agent', diagnostics: [{ id: 'versions', title: 'Versions', text: 'Daemon: 1' }] })
    expect(it?.state).toBe('refused')
    expect(it?.category).toBe('other')
    expect(it?.senderKind).toBe('agent')
    expect(it?.diagnostics).toEqual([{ id: 'versions', title: 'Versions', text: 'Daemon: 1' }])
  })

  it('reads the refusal message from the body, else the error', () => {
    const err = Object.assign(new Error('needs_connect'), { body: { error: 'needs_connect', message: 'This server is not paired.' } })
    expect(refusalMessage(err)).toBe('This server is not paired.')
    expect(refusalMessage(new Error('boom'))).toBe('boom')
  })

  it('copy as text addresses Alakazam and keeps the text', () => {
    const t = feedbackAsText({ category: 'bug', text: '  the tab froze \n', k2Version: '0.46.11' })
    expect(t).toBe(`To: ${CONTACT_ADDRESS}\nFeedback about K2 (bug)\nK2 0.46.11\n\nthe tab froze`)
  })

  it('an edit may only delete lines', () => {
    expect(onlyDeletesLines('a\nb\nc', 'a\nc')).toBe(true)
    expect(onlyDeletesLines('a\nb\nc', 'a\nb\nc')).toBe(true)
    expect(onlyDeletesLines('a\nb\nc', 'c\na')).toBe(false)
    expect(onlyDeletesLines('a\nb\nc', 'a\nx')).toBe(false)
  })

  it('state chips and byte counts', () => {
    expect(stateChip('failed')).toEqual({ label: 'Failed', tone: 'error' })
    expect(stateChip('sent').tone).toBe('ok')
    expect(utf8Bytes('é')).toBe(2)
  })
})
