// prd-tickets-badge-orphans — the Tickets fetches against a mocked daemon.
//   T7: `fetchWaitingCount` rejects when `waiting-count` rejects and fires
//       no per-workspace `feedback/list` GET (the old fallback turned a dead
//       host into a count of 0).
//   TB4/TB14: a server reporting `tickets-list-all` answers the page in ONE
//       `list-all?all=1` GET (unlinked rows kept); an older one keeps the
//       per-workspace fan-out.

import { beforeEach, describe, expect, it, vi } from 'vitest'

const cli = vi.hoisted(() => ({
  get: vi.fn(),
  supports: { 'tickets-list-all': true } as Record<string, boolean>,
}))

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: cli.get,
  daemonCliPost: vi.fn(),
}))

vi.mock('@/kessel/server-scope', () => {
  const scope = {
    serverSupports: (f: string) => {
      if (!(f in cli.supports)) throw new Error(`unexpected feature check: ${f}`)
      return cli.supports[f]
    },
  }
  return { primaryScope: () => scope }
})

import { fetchAllFeedback, fetchFeedbackBrief, fetchWaitingCount, parseFeedbackBrief } from './feedback-api'

function item(id: string, projectId: string, createdAt: number): Record<string, unknown> {
  return {
    id,
    projectId,
    sessionId: null,
    sessionKind: null,
    agentName: 'scout',
    kind: 'question',
    title: `t-${id}`,
    body: null,
    options: null,
    priority: 3,
    status: 'waiting',
    answer: null,
    createdAt,
    updatedAt: createdAt,
    answeredAt: null,
    commentCount: 1,
    assignees: [],
  }
}

const projects = [{ id: 'p1', name: 'Alpha', path: '/ws/alpha' }]

beforeEach(() => {
  cli.get.mockReset()
  cli.supports['tickets-list-all'] = true
})

describe('fetchWaitingCount (T7)', () => {
  it('resolves the host-wide count', async () => {
    cli.get.mockResolvedValueOnce({ ok: true, count: 2 })
    await expect(fetchWaitingCount()).resolves.toBe(2)
    expect(cli.get).toHaveBeenCalledTimes(1)
    expect(cli.get.mock.calls[0][1]).toBe('feedback/waiting-count')
  })

  it('rejects when waiting-count rejects, and fires no feedback/list GET', async () => {
    cli.get.mockRejectedValueOnce(new Error('host unreachable'))
    await expect(fetchWaitingCount()).rejects.toThrow('host unreachable')
    expect(cli.get).toHaveBeenCalledTimes(1)
    const paths = cli.get.mock.calls.map((c) => c[1])
    expect(paths).toEqual(['feedback/waiting-count'])
  })

  it('rejects a body with no count instead of reading it as 0', async () => {
    cli.get.mockResolvedValueOnce({ ok: true })
    await expect(fetchWaitingCount()).rejects.toThrow('no count')
  })
})

describe('fetchAllFeedback (TB4/TB14)', () => {
  it('a server with tickets-list-all: ONE list-all?all=1 GET, unlinked rows kept with a null workspace', async () => {
    cli.get.mockResolvedValueOnce({
      ok: true,
      items: [
        { ...item('a', 'p1', 10), projectName: 'Alpha', projectPath: '/ws/alpha', linked: true },
        { ...item('b', 'gone', 20), projectName: null, projectPath: null, linked: false },
      ],
    })
    const rows = await fetchAllFeedback(projects)
    expect(cli.get).toHaveBeenCalledTimes(1)
    expect(cli.get.mock.calls[0][1]).toBe('feedback/list-all')
    expect(cli.get.mock.calls[0][2]).toEqual({ all: 1 })
    expect(rows.map((r) => [r.id, r.linked, r.projectName, r.projectPath])).toEqual([
      ['b', false, null, null],
      ['a', true, 'Alpha', '/ws/alpha'],
    ])
  })

  it('list-all failing throws (the page shows the error, not a fake empty)', async () => {
    cli.get.mockRejectedValueOnce(new Error('boom'))
    await expect(fetchAllFeedback(projects)).rejects.toThrow('boom')
  })

  it('a server without the key keeps the per-workspace fan-out, every row linked', async () => {
    cli.supports['tickets-list-all'] = false
    cli.get.mockResolvedValueOnce({ ok: true, items: [item('a', 'p1', 10)] })
    const rows = await fetchAllFeedback(projects)
    expect(cli.get).toHaveBeenCalledTimes(1)
    expect(cli.get.mock.calls[0][1]).toBe('feedback/list')
    expect(cli.get.mock.calls[0][2]).toEqual({ project: '/ws/alpha', all: 1 })
    expect(rows.map((r) => [r.id, r.linked, r.projectName])).toEqual([['a', true, 'Alpha']])
  })
})

// prd-ticket-html-brief-v1 H17/H39: the brief rides `show?brief=1`, and a
// malformed brief throws instead of reaching state.
describe('fetchFeedbackBrief', () => {
  const good = {
    html: '<p>hi</p>',
    text: 'hi',
    bytes: 9,
    sha256: 'abc',
    sanitizer: 'k2-brief-v1',
    createdAt: 1,
  }

  it('GETs feedback/show with brief=1 and returns the parsed brief', async () => {
    cli.get.mockResolvedValue({ ok: true, id: 'x', brief: good })
    await expect(fetchFeedbackBrief('x')).resolves.toEqual(good)
    expect(cli.get).toHaveBeenCalledTimes(1)
    expect(cli.get.mock.calls[0][1]).toBe('feedback/show')
    expect(cli.get.mock.calls[0][2]).toEqual({ id: 'x', brief: 1 })
  })

  it('null or missing brief → null (no brief, or an older daemon)', async () => {
    cli.get.mockResolvedValue({ ok: true, id: 'x', brief: null })
    await expect(fetchFeedbackBrief('x')).resolves.toBeNull()
    cli.get.mockResolvedValue({ ok: true, id: 'x' })
    await expect(fetchFeedbackBrief('x')).resolves.toBeNull()
  })

  it('a malformed brief throws', () => {
    expect(() => parseFeedbackBrief('<p>hi</p>')).toThrow(/not an object/)
    expect(() => parseFeedbackBrief([good])).toThrow(/not an object/)
    expect(() => parseFeedbackBrief({ ...good, html: 5 })).toThrow(/brief.html/)
    expect(() => parseFeedbackBrief({ ...good, text: null })).toThrow(/brief.text/)
    expect(() => parseFeedbackBrief({ ...good, sha256: '' })).toThrow(/sha256/)
    expect(() => parseFeedbackBrief({ ...good, bytes: '9' })).toThrow(/bytes/)
  })
})
