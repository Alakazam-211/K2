// `k2.open(url)` (catalog v21 `links.open`, cap `links:open`; 0.46.4):
// the link check, the custom layer's rules (cap, press, rate, one card at a
// time) and the first-time-per-site card flow. Fixtures are made up
// (g-test0001, example.test).
import { describe, expect, it } from 'vitest'
import type { ZenWidgetBridge } from './zen-bridge'
import type { ZenCustomWidgetPayload, ZenFrameReply } from './zen-custom-types'
import { createZenCustomLayer, type ZenCustomLayerDeps, type ZenCustomLinks } from './zen-custom-bridge'
import { createZenLinks, zenLinkTarget, ZEN_LINK_MAX_CHARS, ZEN_LINKS_OPEN, type ZenLinkAnswer, type ZenLinkAsk } from './zen-links'
import { ZEN_CUSTOM_VERBS } from './zen-verbs.generated'
import { USER_WIDGET_CAPS } from '../k2-caps.generated'

function payload(caps: string[]): ZenCustomWidgetPayload {
  return {
    id: 'board',
    kind: 'custom',
    widget: 'board-lite',
    column: 0,
    props: { config: {} },
    caps: caps as ZenCustomWidgetPayload['caps'],
    requested: caps as ZenCustomWidgetPayload['requested'],
    source: 'user',
    name: 'Board',
    description: null,
    reasons: {},
    libs: [],
    hash: 'h1',
    state: 'ok',
    errors: [],
    warnings: [],
    origin: 'local',
    paused: null,
  }
}

class FakeLinks implements ZenCustomLinks {
  press = true
  opened: Array<[string, string]> = []
  answer = true
  gate: Promise<void> | null = null
  pressed(): boolean {
    return this.press
  }
  async open(url: string, site: string): Promise<boolean> {
    if (this.gate) await this.gate
    this.opened.push([url, site])
    return this.answer
  }
}

function harness(links: ZenCustomLinks | undefined, caps = ['links:open']) {
  const clock = { t: 1_000_000 }
  const inner = { widgetId: 'board', caps: new Set<string>(), call: () => null } as unknown as ZenWidgetBridge
  const deps: ZenCustomLayerDeps = {
    widget: () => payload(caps),
    gardenId: () => 'g-test0001',
    inner,
    boundRows: () => [],
    ensureConversation: async () => undefined,
    push: () => undefined,
    stop: () => undefined,
    paused: () => false,
    pauseForRunaway: () => undefined,
    theme: () => ({}),
    onThemeChange: () => () => undefined,
    now: () => clock.t,
    ...(links ? { links } : {}),
  }
  return { layer: createZenCustomLayer(deps), clock }
}

async function call(layer: ReturnType<typeof createZenCustomLayer>, ...args: unknown[]): Promise<ZenFrameReply> {
  const r = await layer.handle({ id: 1, verb: 'links.open', args })
  if (!r) throw new Error('no reply')
  return r
}

function ok(r: ZenFrameReply): unknown {
  if (!('ok' in r)) throw new Error('not a call reply')
  if (!r.ok) throw new Error(`refused: ${JSON.stringify(r.error)}`)
  return r.value
}

function code(r: ZenFrameReply): string {
  if (!('ok' in r)) throw new Error('not a call reply')
  if (r.ok) throw new Error(`ok: ${JSON.stringify(r.value)}`)
  return r.error.code
}

describe('zenLinkTarget', () => {
  it('takes http and https links and names their site (origin)', () => {
    expect(zenLinkTarget(' https://Example.test/a/b?c=1#d ')).toEqual({ url: 'https://Example.test/a/b?c=1#d', site: 'https://example.test' })
    expect(zenLinkTarget('http://example.test:8080/x')).toEqual({ url: 'http://example.test:8080/x', site: 'http://example.test:8080' })
    expect(zenLinkTarget('https://example.test:443/x')).toEqual({ url: 'https://example.test:443/x', site: 'https://example.test' })
  })

  it('refuses everything else with a sentence', () => {
    for (const bad of [
      42,
      null,
      '',
      'javascript:alert(1)',
      'file:///etc/passwd',
      'mailto:a@b.test',
      'data:text/html,hi',
      'k2://x',
      '/relative',
      'example.test',
      'https://user:pw@example.test/',
      'https://user@example.test/',
      `https://example.test/${'a'.repeat(ZEN_LINK_MAX_CHARS)}`,
    ]) {
      expect(typeof zenLinkTarget(bad), String(bad)).toBe('string')
    }
  })
})

describe('links.open through the custom layer', () => {
  it('is a widget row with its own widget cap', () => {
    expect(ZEN_CUSTOM_VERBS['links.open']).toEqual({ cap: 'links:open', reach: 'local', kind: 'call' })
    expect(USER_WIDGET_CAPS as readonly string[]).toContain('links:open')
  })

  it('needs the cap, then deps.links', async () => {
    expect(code(await call(harness(new FakeLinks(), []).layer, 'https://example.test'))).toBe('cap_not_granted')
    expect(code(await call(harness(undefined).layer, 'https://example.test'))).toBe('verb_unavailable')
  })

  it('opens a checked link just after a press and answers {ok}', async () => {
    const links = new FakeLinks()
    const { layer, clock } = harness(links)
    expect(ok(await call(layer, 'https://example.test/report'))).toEqual({ ok: true })
    expect(links.opened).toEqual([['https://example.test/report', 'https://example.test']])
    links.answer = false
    clock.t += 1_000
    expect(ok(await call(layer, 'https://example.test/other'))).toEqual({ ok: false })
  })

  it('refuses a bad link (bad_args) and a call without a press (failed), opening nothing', async () => {
    const links = new FakeLinks()
    const { layer, clock } = harness(links)
    expect(code(await call(layer, 'javascript:alert(1)'))).toBe('bad_args')
    expect(code(await call(layer))).toBe('bad_args')
    links.press = false
    clock.t += 1_000
    expect(code(await call(layer, 'https://example.test'))).toBe('failed')
    expect(links.opened).toEqual([])
  })

  it('once a second and 10 a minute', async () => {
    const links = new FakeLinks()
    const { layer, clock } = harness(links)
    expect(ok(await call(layer, 'https://example.test/0'))).toEqual({ ok: true })
    expect(code(await call(layer, 'https://example.test/again'))).toBe('rate_limited')
    for (let i = 1; i < ZEN_LINKS_OPEN.perMinute; i++) {
      clock.t += 1_000
      expect(ok(await call(layer, `https://example.test/${i}`))).toEqual({ ok: true })
    }
    clock.t += 1_000
    expect(code(await call(layer, 'https://example.test/11th'))).toBe('rate_limited')
    expect(links.opened).toHaveLength(ZEN_LINKS_OPEN.perMinute)
    clock.t += 60_000
    expect(ok(await call(layer, 'https://example.test/later'))).toEqual({ ok: true })
  })

  it('one card at a time', async () => {
    const links = new FakeLinks()
    let release = (): void => undefined
    links.gate = new Promise<void>((r) => {
      release = r
    })
    const { layer, clock } = harness(links)
    const first = call(layer, 'https://example.test/a')
    clock.t += 2_000
    expect(code(await call(layer, 'https://example.test/b'))).toBe('rate_limited')
    release()
    expect(ok(await first)).toEqual({ ok: true })
    expect(links.opened.map(([u]) => u)).toEqual(['https://example.test/a'])
  })
})

describe('createZenLinks: K2’s card the first time per site in a Garden', () => {
  function flow(known: string[], answer: ZenLinkAnswer, allowFails = false) {
    const asks: ZenLinkAsk[] = []
    const allowed: Array<[string, string]> = []
    const opened: string[] = []
    const links = createZenLinks({
      gardenId: () => 'g-test0001',
      sites: async (g) => {
        expect(g).toBe('g-test0001')
        return known
      },
      allow: async (g, s) => {
        if (allowFails) throw new Error('daemon down')
        allowed.push([g, s])
      },
      ask: async (a) => {
        asks.push(a)
        return answer
      },
      describe: () => ({ widget: 'Board', garden: 'Fleet Ops' }),
      openUrl: async (u) => {
        opened.push(u)
      },
    })
    return { links, asks, allowed, opened }
  }

  it('a site the Garden already allows opens without a card', async () => {
    const f = flow(['https://example.test'], 'deny')
    expect(await f.links.open('https://example.test/x', 'https://example.test')).toBe(true)
    expect(f.asks).toEqual([])
    expect(f.opened).toEqual(['https://example.test/x'])
  })

  it('Don’t open opens nothing and keeps nothing', async () => {
    const f = flow([], 'deny')
    expect(await f.links.open('https://example.test/x', 'https://example.test')).toBe(false)
    expect(f.asks).toEqual([{ url: 'https://example.test/x', site: 'https://example.test', widget: 'Board', garden: 'Fleet Ops' }])
    expect(f.opened).toEqual([])
    expect(f.allowed).toEqual([])
  })

  it('Open once opens and keeps nothing; Always also keeps the site for the Garden', async () => {
    const once = flow([], 'once')
    expect(await once.links.open('https://example.test/x', 'https://example.test')).toBe(true)
    expect(once.allowed).toEqual([])
    expect(once.opened).toEqual(['https://example.test/x'])
    const always = flow([], 'always')
    expect(await always.links.open('https://example.test/x', 'https://example.test')).toBe(true)
    expect(always.allowed).toEqual([['g-test0001', 'https://example.test']])
    expect(always.opened).toEqual(['https://example.test/x'])
  })

  it('Always still opens when keeping the site fails (the person said yes)', async () => {
    const f = flow([], 'always', true)
    expect(await f.links.open('https://example.test/x', 'https://example.test')).toBe(true)
    expect(f.opened).toEqual(['https://example.test/x'])
  })
})
