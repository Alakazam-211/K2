import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { ServerScope } from '@/kessel/server-scope'

const h = vi.hoisted(() => ({
  calls: [] as Array<{ method: string; route: string; arg?: unknown }>,
  answer: null as unknown,
}))

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (_scope: unknown, route: string, params?: unknown) => {
    h.calls.push({ method: 'GET', route, arg: params })
    return h.answer
  }),
  daemonCliPost: vi.fn(async (_scope: unknown, route: string, body?: unknown) => {
    h.calls.push({ method: 'POST', route, arg: body })
    return h.answer
  }),
}))

import {
  CONTACT_ADDRESS,
  PRODUCT_FEEDBACK_SCREENSHOT_FEATURE,
  feedbackAsText,
  fetchScreenshotDraft,
  fetchSentScreenshot,
  forgetScreenshot,
  onlyDeletesLines,
  parseItem,
  parseSettings,
  refusalMessage,
  sendFeedback,
  stateChip,
  uploadScreenshot,
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

const scope = {} as ServerScope
const SHA = 'ab'.repeat(32)

describe('product-feedback screenshot client (prd-feedback-screenshot-v1)', () => {
  beforeEach(() => {
    h.calls = []
    h.answer = null
  })

  it('names the feature key the daemon lists', () => {
    expect(PRODUCT_FEEDBACK_SCREENSHOT_FEATURE).toBe('product-feedback-screenshot-v1')
  })

  it('uploads a window picture with how it was taken, a file without', async () => {
    h.answer = { ok: true, draftId: 'pfs_1', sha256: SHA, bytes: 1200, width: 320, height: 200, source: 'window', capture: 'page', expiresAt: 99 }
    const d = await uploadScreenshot(scope, { image: 'iVBOR', source: 'window', capture: 'page' })
    expect(d).toEqual({ draftId: 'pfs_1', sha256: SHA, bytes: 1200, width: 320, height: 200, source: 'window', capture: 'page', expiresAt: 99 })
    expect(h.calls).toEqual([{ method: 'POST', route: 'product-feedback/screenshot', arg: { image: 'iVBOR', source: 'window', capture: 'page' } }])
    h.calls = []
    h.answer = { draftId: 'pfs_2', sha256: SHA, source: 'file', capture: null }
    const f = await uploadScreenshot(scope, { image: 'iVBOR', source: 'file' })
    expect(f.capture).toBeNull()
    expect(h.calls[0].arg).toEqual({ image: 'iVBOR', source: 'file' })
  })

  it('refuses an answer without a draft id or a real hash', async () => {
    h.answer = { draftId: 'pfs_1', sha256: 'nope' }
    await expect(uploadScreenshot(scope, { image: 'x', source: 'file' })).rejects.toThrow('without the screenshot')
    h.answer = { sha256: SHA }
    await expect(uploadScreenshot(scope, { image: 'x', source: 'file' })).rejects.toThrow('without the screenshot')
  })

  it('reads back the stored bytes of a draft or a row', async () => {
    h.answer = { dataUrl: 'data:image/png;base64,iVBOR', sha256: SHA, bytes: 5, width: 2, height: 1, source: 'window', capture: 'webview' }
    const d = await fetchScreenshotDraft(scope, 'pfs_1')
    expect(d).toEqual({ dataUrl: 'data:image/png;base64,iVBOR', sha256: SHA, bytes: 5, width: 2, height: 1, source: 'window', capture: 'webview' })
    await fetchSentScreenshot(scope, 'pf_1')
    expect(h.calls).toEqual([
      { method: 'GET', route: 'product-feedback/screenshot', arg: { draftId: 'pfs_1' } },
      { method: 'GET', route: 'product-feedback/screenshot', arg: { id: 'pf_1' } },
    ])
    h.answer = { dataUrl: 'data:image/jpeg;base64,xx', sha256: SHA }
    await expect(fetchScreenshotDraft(scope, 'pfs_1')).rejects.toThrow('without the picture')
  })

  it('forgets a draft (Remove)', async () => {
    h.answer = { ok: true, draftId: 'pfs_1', forgotten: true }
    expect(await forgetScreenshot(scope, 'pfs_1')).toBe(true)
    expect(h.calls).toEqual([{ method: 'POST', route: 'product-feedback/screenshot/forget', arg: { draftId: 'pfs_1' } }])
    h.answer = { ok: true, forgotten: false }
    expect(await forgetScreenshot(scope, 'pfs_1')).toBe(false)
  })

  it('send carries the confirmed screenshot only when given', async () => {
    h.answer = { id: 'pf_1', state: 'queued' }
    const base = { category: 'bug' as const, text: 't', contactEmail: null, client: 'desktop' as const, k2Version: null, locale: null, diagnostics: null }
    await sendFeedback(scope, base)
    await sendFeedback(scope, { ...base, screenshot: null })
    await sendFeedback(scope, { ...base, screenshot: { draftId: 'pfs_1', sha256: SHA } })
    expect(h.calls.map((c) => (c.arg as Record<string, unknown>).screenshot)).toEqual([undefined, undefined, { draftId: 'pfs_1', sha256: SHA }])
  })

  it('a row says whether it has a screenshot and whether the server keeps it', () => {
    const plain = parseItem({ id: 'pf_1' })
    expect(plain?.hasScreenshot).toBe(false)
    expect(plain?.screenshot).toBeNull()
    const it = parseItem({ id: 'pf_2', hasScreenshot: true, screenshot: { bytes: 10, width: 3, height: 2, source: 'window', capture: 'page', onBox: false } })
    expect(it?.hasScreenshot).toBe(true)
    expect(it?.screenshot).toEqual({ bytes: 10, width: 3, height: 2, source: 'window', capture: 'page', onBox: false })
    expect(parseItem({ id: 'pf_3', screenshot: { source: 'weird', capture: 'native' } })?.screenshot).toEqual({ bytes: 0, width: 0, height: 0, source: 'file', capture: null, onBox: false })
  })
})
