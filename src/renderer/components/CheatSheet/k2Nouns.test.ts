import { describe, it, expect } from 'vitest'
import { K2_CHEAT_SHEET_INTRO, K2_CHEAT_SHEET_NOTES, K2_NOUN_GROUPS } from './k2Nouns'

describe('K2 noun cheat sheet catalog', () => {
  it('intro talks about K2 nouns, never K2SO', () => {
    expect(K2_CHEAT_SHEET_INTRO).toContain('k2 <noun>')
    expect(K2_CHEAT_SHEET_INTRO.toLowerCase()).not.toContain('k2so')
  })

  it('marks CLI nouns that need a Linux sidecar loaded', () => {
    expect(K2_CHEAT_SHEET_NOTES).toHaveLength(1)
    const note = K2_CHEAT_SHEET_NOTES[0]
    expect(note.note).toBe('linux-sidecar')
    expect(note.body.toLowerCase()).toMatch(/sidecar/)
    expect(note.body.toLowerCase()).toMatch(/linux/)
    expect(note.body.toLowerCase()).toMatch(/postgres|mail/)
    expect(note.body.toLowerCase()).not.toContain('k2so')
    const mail = K2_NOUN_GROUPS.flatMap((g) => g.items).find((i) => i.noun === 'mail')
    expect(mail?.note).toBe('linux-sidecar')
    const whoami = K2_NOUN_GROUPS.flatMap((g) => g.items).find((i) => i.noun === 'whoami')
    expect(whoami?.note).toBeUndefined()
  })

  it('groups the expected noun families', () => {
    expect(K2_NOUN_GROUPS.map((g) => g.title)).toEqual([
      'Talk',
      'Inbox & humans',
      'Identity',
      'Agents & groups',
      'Always-on & more',
    ])
  })

  it('includes the core nouns humans and agents run', () => {
    const nouns = K2_NOUN_GROUPS.flatMap((g) => g.items.map((i) => i.noun))
    for (const need of [
      'msg',
      'thread',
      'read',
      'inbox',
      'feedback / tickets',
      'whoami',
      'connections',
      'agent',
      'preset',
      'project',
      'workspace',
      'heartbeat',
      'activity',
      'mail',
      'wiki',
      'skills',
      'publish',
      'dns',
      'checkin / done',
    ]) {
      expect(nouns).toContain(need)
    }
  })

  it('examples use k2, never k2so', () => {
    const examples = K2_NOUN_GROUPS.flatMap((g) =>
      g.items.map((i) => i.example).filter((e): e is string => Boolean(e)),
    )
    expect(examples.length).toBeGreaterThan(0)
    for (const example of examples) {
      expect(example.startsWith('k2 ')).toBe(true)
      expect(example.toLowerCase()).not.toContain('k2so')
    }
    for (const group of K2_NOUN_GROUPS) {
      expect(group.items.length).toBeGreaterThan(0)
      for (const item of group.items) {
        expect(item.blurb.toLowerCase()).not.toContain('k2so')
      }
    }
  })
})
