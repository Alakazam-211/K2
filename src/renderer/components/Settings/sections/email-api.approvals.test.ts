import { beforeEach, describe, expect, it, vi } from 'vitest'

const daemonCliGet = vi.fn()

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: (...args: unknown[]) => daemonCliGet(...args),
  daemonCliPost: vi.fn(),
}))

import { fetchApprovalFile, fetchApprovalText } from './email-api'

describe('opening a message waiting for approval', () => {
  beforeEach(() => daemonCliGet.mockReset())

  it('asks the daemon for the whole text', async () => {
    daemonCliGet.mockResolvedValue({ ok: true, message: { text: 'the whole message' } })
    await expect(fetchApprovalText('out_1')).resolves.toBe('the whole message')
    expect(daemonCliGet.mock.calls[0].slice(1)).toEqual(['mail/approvals/show', { id: 'out_1' }])
  })

  it('turns a waiting file into exactly its bytes, typed so nothing renders it', async () => {
    daemonCliGet.mockResolvedValue({ ok: true, dataBase64: btoa(String.fromCharCode(0, 255, 7)) })
    const blob = await fetchApprovalFile('out_1', 2)
    expect(daemonCliGet.mock.calls[0].slice(1)).toEqual(['mail/approvals/show', { id: 'out_1', file: '2' }])
    expect(blob.type).toBe('application/octet-stream')
    expect(Array.from(new Uint8Array(await blob.arrayBuffer()))).toEqual([0, 255, 7])
  })
})
