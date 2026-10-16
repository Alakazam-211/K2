import { describe, it, expect } from 'vitest'
import catalog from '@shared/cli-catalog.generated.json'
import { K2_EXPLORER_INTRO, K2_NODE_BLURBS, K2_NOUN_GROUPS } from './k2Nouns'

const items = K2_NOUN_GROUPS.flatMap((g) => g.items)

describe('K2 noun families (Help explorer)', () => {
  it('intro talks about K2 nouns, never K2SO', () => {
    expect(K2_EXPLORER_INTRO).toContain('k2 <noun>')
    expect(K2_EXPLORER_INTRO.toLowerCase()).not.toContain('k2so')
  })

  it('tags whoami as sidecar and mail as Linux sidecar', () => {
    expect(items.find((i) => i.noun === 'whoami')?.note).toBe('sidecar')
    expect(items.find((i) => i.noun === 'mail')?.note).toBe('linux-sidecar')
  })

  it('groups the noun families in rail order', () => {
    expect(K2_NOUN_GROUPS.map((g) => g.title)).toEqual([
      'Talk',
      'Inbox & humans',
      'Identity',
      'Agents & groups',
      'Always-on & more',
      'Mail & calendars',
      'Apps, Gardens & data',
      'Network & domains',
      'Server',
      'Help & docs',
    ])
    expect(new Set(K2_NOUN_GROUPS.map((g) => g.id)).size).toBe(K2_NOUN_GROUPS.length)
  })

  it('every top-level noun `k2 --schema` lists has a family (add new nouns to k2Nouns.ts)', () => {
    const mapped = new Set(items.flatMap((i) => [i.noun, ...(i.also ?? []), ...(i.leftover ?? [])]))
    const heads = [...new Set(catalog.k2.commands.map((c) => c.name.split(' ')[0]))]
    expect(heads.length).toBeGreaterThan(40)
    expect(heads.filter((h) => !mapped.has(h))).toEqual([])
  })

  it('no noun is listed twice', () => {
    const all = items.flatMap((i) => [i.noun, ...(i.also ?? []), ...(i.leftover ?? [])])
    expect(all.filter((n, i) => all.indexOf(n) !== i)).toEqual([])
  })

  it('keeps the core nouns humans and agents run', () => {
    const nouns = items.map((i) => i.noun)
    for (const need of ['msg', 'thread', 'read', 'inbox', 'feedback', 'whoami', 'connections', 'agent', 'preset', 'project', 'workspace', 'heartbeat', 'activity', 'mail', 'wiki', 'skills', 'publish', 'dns', 'checkin', 'compute', 'vault', 'study']) {
      expect(nouns).toContain(need)
    }
    expect(items.find((i) => i.noun === 'feedback')?.also).toEqual(['tickets'])
    expect(items.find((i) => i.noun === 'checkin')?.also).toEqual(['done'])
  })

  it('examples use k2, never k2so; blurbs never say k2so', () => {
    const examples = items.map((i) => i.example).filter((e): e is string => Boolean(e))
    expect(examples.length).toBeGreaterThan(30)
    for (const example of examples) {
      expect([example, example.startsWith('k2 ')]).toEqual([example, true])
      expect(example.toLowerCase()).not.toContain('k2so')
    }
    for (const group of K2_NOUN_GROUPS) {
      expect(group.items.length).toBeGreaterThan(0)
      for (const item of group.items) expect(item.blurb.toLowerCase()).not.toContain('k2so')
    }
  })

  it('every k2-node subcommand has a blurb', () => {
    const nodeNouns = [...new Set(catalog.k2Node.commands.map((c) => c.noun))]
    expect(nodeNouns).toContain('enroll')
    expect(nodeNouns.filter((n) => !K2_NODE_BLURBS[n])).toEqual([])
  })
})
