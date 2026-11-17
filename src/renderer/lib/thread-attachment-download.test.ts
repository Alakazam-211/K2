// A Thread file's Download: on the hosted web client the Thread door's own
// download link goes to the browser (lib/file-link); the desktop and a
// ticket's door keep the chunked read (fs-transfer's lifecycle).

import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { ServerScope } from '@/kessel/server-scope'

const web = vi.hoisted(() => ({ on: false }))
const calls = vi.hoisted(() => [] as { label: string; opts: Record<string, unknown> }[])
const mintAttachmentLink = vi.fn()

vi.mock('@/lib/is-web', () => ({ isWebClient: () => web.on }))
vi.mock('@/lib/file-link', () => ({ mintAttachmentLink: (...a: unknown[]) => mintAttachmentLink(...a) }))
vi.mock('@/lib/fs-transfer', () => ({
  downloadChunks: async (label: string, _read: unknown, opts: Record<string, unknown>) => {
    calls.push({ label, opts })
    return label
  },
}))
vi.mock('@/lib/thread-image-thumbs', () => ({ readAttachment: vi.fn() }))

import { downloadThreadAttachment } from './thread-attachment-download'

const scope = { id: 'box' } as unknown as ServerScope

beforeEach(() => {
  web.on = false
  calls.length = 0
  mintAttachmentLink.mockReset()
})

describe('downloadThreadAttachment', () => {
  it('hosted web, a Thread file: the link is the Thread door’s token for that attachment', async () => {
    web.on = true
    const link = { url: 'https://h/_k2/transfers/downloads/x?token=k2dn_x.y', name: 'plan.pdf', size: 3, mime: 'application/pdf' }
    mintAttachmentLink.mockResolvedValue(link)
    await downloadThreadAttachment({ scope, addr: 'sales', docId: 'doc-9', n: 1 }, 'plan.pdf', { fromOthers: true })
    expect(calls).toHaveLength(1)
    expect(calls[0].label).toBe('plan.pdf')
    expect(calls[0].opts.quarantine).toBe(true)
    const make = calls[0].opts.link as () => Promise<unknown>
    expect(typeof make).toBe('function')
    await expect(make()).resolves.toBe(link)
    expect(mintAttachmentLink).toHaveBeenCalledWith(scope, { addr: 'sales', docId: 'doc-9', n: 1 }, 'plan.pdf')
  })

  it('the desktop keeps the chunked read (no link)', async () => {
    await downloadThreadAttachment({ scope, addr: 'sales', docId: 'doc-9', n: 1 }, 'plan.pdf', { fromOthers: true })
    expect(calls[0].opts).toEqual({ quarantine: true })
    await downloadThreadAttachment({ scope, addr: 'sales', docId: 'doc-9', n: 1 }, 'plan.pdf')
    expect(calls[1].opts).toEqual({})
  })

  it('hosted web, a ticket’s door: no link (its own reader)', async () => {
    web.on = true
    await downloadThreadAttachment({ scope, read: vi.fn(), docId: 'c-1', n: 0 }, 'shot.png')
    expect(calls[0].opts).toEqual({})
    expect(mintAttachmentLink).not.toHaveBeenCalled()
  })
})
