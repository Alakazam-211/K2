// `k2.download` consent (security review #1): "Always" is the daemon's, one
// answer for every client; revoking it brings K2's card back; "Save once"
// keeps nothing. The daemon is faked as one shared list (what
// `/cli/zen/downloads*` serves every window and the web client).
import { describe, expect, it } from 'vitest'
import { zenDownloadConsent, type ZenDownloadAnswer, type ZenDownloadAsk, type ZenDownloadConsentStore } from './zen-download-consent'

class FakeDaemon {
  lists = new Map<string, string[]>()
  store(): ZenDownloadConsentStore {
    return {
      widgets: async (g) => [...(this.lists.get(g) ?? [])],
      allow: async (g, w) => {
        const l = this.lists.get(g) ?? []
        if (!l.includes(w)) l.push(w)
        this.lists.set(g, l)
      },
    }
  }
  forget(g: string, w: string): void {
    this.lists.set(g, (this.lists.get(g) ?? []).filter((x) => x !== w))
  }
}

/** One client: its own memory (none kept) and its own card. */
function client(daemon: FakeDaemon, answer: ZenDownloadAnswer) {
  const shown: ZenDownloadAsk[] = []
  const store = daemon.store()
  return {
    shown,
    save: () =>
      zenDownloadConsent(store, 'g-1', 'board', { widget: 'Board', garden: 'Home', name: 'Board.png', size: 10 }, async (ask) => {
        shown.push(ask)
        return answer
      }),
  }
}

describe('download consent is the daemon’s, for every client', () => {
  it('Always in one client is honoured by another; revoking brings the card back', async () => {
    const daemon = new FakeDaemon()
    const desktop = client(daemon, 'always')
    expect(await desktop.save()).toBe(true)
    expect(desktop.shown).toHaveLength(1)
    // Another window or the web client: no card, the daemon already says yes.
    const web = client(daemon, 'deny')
    expect(await web.save()).toBe(true)
    expect(web.shown).toHaveLength(0)
    // The person revokes it (k2 / Settings, the owner token): the card is back everywhere.
    daemon.forget('g-1', 'board')
    expect(await web.save()).toBe(false)
    expect(web.shown).toHaveLength(1)
    expect(await desktop.save()).toBe(true)
    expect(desktop.shown).toHaveLength(2)
  })

  it('Save once keeps nothing; Don’t save saves nothing', async () => {
    const daemon = new FakeDaemon()
    const once = client(daemon, 'once')
    expect(await once.save()).toBe(true)
    expect(await once.save()).toBe(true)
    expect(once.shown).toHaveLength(2)
    expect(daemon.lists.get('g-1') ?? []).toEqual([])
    const no = client(daemon, 'deny')
    expect(await no.save()).toBe(false)
    expect(daemon.lists.get('g-1') ?? []).toEqual([])
  })

  it('the answer is per Garden and widget', async () => {
    const daemon = new FakeDaemon()
    await client(daemon, 'always').save()
    const store = daemon.store()
    const shown: ZenDownloadAsk[] = []
    const ask = { widget: 'Notes', garden: 'Home', name: 'n.txt', size: 1 }
    expect(await zenDownloadConsent(store, 'g-1', 'notes', ask, async (a) => (shown.push(a), 'deny'))).toBe(false)
    expect(await zenDownloadConsent(store, 'g-2', 'board', ask, async (a) => (shown.push(a), 'deny'))).toBe(false)
    expect(shown).toHaveLength(2)
  })
})
