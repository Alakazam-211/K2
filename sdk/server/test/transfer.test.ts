// uploadToDoor (0.46.9): resumes from what the door holds, sends each
// missing range once with its own sha256, retries a 503, stops on a
// refusal with K2's code, and completes with the file's sha256.

import { createHash } from 'node:crypto'

import { describe, expect, it } from 'vitest'

import { backoffMs, downloadLink, missingParts, TransferError, uploadToDoor } from '../src/index.ts'

const sha = (b: Uint8Array) => createHash('sha256').update(b).digest('hex')

function fakeDoor(size: number, held: Array<[number, number]>, opts: { fail503?: number; refuse?: [number, string] } = {}) {
  const data = new Uint8Array(size)
  let received = held.map(([a, b]) => [a, b] as [number, number])
  const puts: string[] = []
  let fail = opts.fail503 ?? 0
  const status = () => {
    received.sort((x, y) => x[0] - y[0])
    const merged: Array<[number, number]> = []
    for (const [a, b] of received) {
      const last = merged[merged.length - 1]
      if (last && a <= last[1] + 1) last[1] = Math.max(last[1], b)
      else merged.push([a, b])
    }
    received = merged
    const missing: Array<[number, number]> = []
    let at = 0
    for (const [a, b] of merged) {
      if (a > at) missing.push([at, a - 1])
      at = b + 1
    }
    if (at < size) missing.push([at, size - 1])
    const got = merged.reduce((n, [a, b]) => n + b - a + 1, 0)
    return { uploadId: 'u1', url: '/_k2/transfers/uploads/u1', size, received: merged, missing, receivedBytes: got, complete: got === size, done: false, expiresAt: 1, partMax: 16 << 20 }
  }
  const fetchImpl = (async (url: string, init: RequestInit) => {
    const h = new Headers(init.headers)
    expect(h.get('authorization')).toBe('Bearer k2up_x.y')
    if (opts.refuse) {
      return new Response(JSON.stringify({ error: { code: opts.refuse[1], hint: 'no' } }), { status: opts.refuse[0] })
    }
    if (init.method === 'GET') return Response.json(status())
    if (init.method === 'PUT') {
      if (fail > 0) {
        fail--
        return new Response('{"error":{"code":"busy","hint":"later"}}', { status: 503, headers: { 'retry-after': '0' } })
      }
      const range = h.get('content-range') ?? ''
      puts.push(range)
      const m = /^bytes (\d+)-(\d+)\/(\d+)$/.exec(range)
      if (!m) throw new Error(`bad range ${range}`)
      const body = init.body as Uint8Array
      expect(h.get('k2-content-sha256')).toBe(sha(body))
      data.set(body, Number(m[1]))
      received.push([Number(m[1]), Number(m[2])])
      return Response.json(status())
    }
    if (init.method === 'POST' && url.endsWith('/complete')) {
      const want = JSON.parse(String(init.body)) as { sha256: string }
      expect(want.sha256).toBe(sha(data))
      return Response.json({ uploadId: 'u1', done: true, relPath: 'in/a.bin', size, sha256: sha(data) })
    }
    return new Response('{}', { status: 404 })
  }) as unknown as typeof fetch
  return { data, puts, fetchImpl }
}

describe('uploadToDoor', () => {
  it('sends only the missing ranges, retries a 503 and completes', async () => {
    const file = new Uint8Array(30_000).map((_, i) => i % 251)
    const door = fakeDoor(file.length, [[10_000, 19_999]], { fail503: 1 })
    door.data.set(file.subarray(10_000, 20_000), 10_000)
    const seen: number[] = []
    const done = await uploadToDoor({
      base: 'https://app.example/',
      url: '/_k2/transfers/uploads/u1',
      token: 'k2up_x.y',
      data: new Blob([file]),
      partSize: 4096,
      backoffMs: 1,
      fetch: door.fetchImpl,
      onProgress: (d) => seen.push(d),
    })
    expect(done.relPath).toBe('in/a.bin')
    expect(door.data).toEqual(file)
    expect(door.puts[0]).toBe('bytes 0-4095/30000')
    expect(door.puts).not.toContain('bytes 10000-14095/30000')
    expect(door.puts).toHaveLength(6)
    expect(seen[0]).toBe(10_000)
    expect(seen[seen.length - 1]).toBe(30_000)
  })

  it('stops at once on a refusal, with K2s code', async () => {
    const door = fakeDoor(10, [], { refuse: [401, 'transfer_token_expired'] })
    const err = await uploadToDoor({ base: 'https://a', url: '/u', token: 'k2up_x.y', data: new Uint8Array(10), backoffMs: 1, fetch: door.fetchImpl }).catch((e: unknown) => e)
    expect(err).toBeInstanceOf(TransferError)
    expect((err as TransferError).code).toBe('transfer_token_expired')
    expect((err as TransferError).retryable).toBe(false)
  })
})

describe('the pieces', () => {
  it('splits gaps into parts and backs off with a cap', () => {
    expect(missingParts([[0, 9]], 4)).toEqual([[0, 4], [4, 4], [8, 2]])
    expect(missingParts([[5, 5], [8, 9]], 100)).toEqual([[5, 1], [8, 2]])
    expect(backoffMs(1, 100, undefined, () => 0)).toBe(50)
    expect(backoffMs(3, 100, undefined, () => 1)).toBe(400)
    expect(backoffMs(30, 500, undefined, () => 1)).toBe(30_000)
    expect(backoffMs(1, 100, 2, () => 0)).toBe(2000)
    expect(downloadLink('https://a/', '/_k2/transfers/downloads/d', 'k2dn_a.b-c')).toBe('https://a/_k2/transfers/downloads/d?token=k2dn_a.b-c')
  })
})
