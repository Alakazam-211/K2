// The reviews' plain names: each daemon row and template file said in
// words, with the raw path or id kept as a quiet second line.
import { describe, expect, it } from 'vitest'
import { contextWords, fileWords, hireRowWords, humanize, permissionWords, rowNote, toolWords } from './review-words'

describe('review words', () => {
  it('ids become words', () => {
    expect(humanize('draft-review')).toBe('Draft review')
    expect(humanize('weekly_digest')).toBe('Weekly digest')
    expect(contextWords('wiki:index')).toBe('Wiki index')
    expect(permissionWords('compute.use')).toBe('Use compute')
    expect(permissionWords('vault.manage')).toBe('Manage vault')
    expect(toolWords('claude')).toBe('Claude')
  })

  it('files a template holds or an agent gets', () => {
    expect(fileWords('.k2/agent/ROLE.md')).toEqual({ what: 'Instructions (charter)', raw: '.k2/agent/ROLE.md' })
    expect(fileWords('template.toml').what).toBe('Template record')
    expect(fileWords('.k2/agent/template.lock').what).toBe('Template record')
    expect(fileWords('.k2/wiki/_Index.md').what).toBe('Wiki index')
    expect(fileWords('wiki/Voice and style.md').what).toBe('Voice and style (wiki note)')
    expect(fileWords('skills/draft-review/SKILL.md').what).toBe('Draft review (skill)')
    expect(fileWords('notes.txt')).toEqual({ what: 'notes.txt' })
  })

  it('each hire row: a plain name, the raw target under it', () => {
    expect(hireRowWords({ step: 'catalog', target: 'wiki:index' })).toEqual({ what: 'Wiki index (context)', raw: 'wiki:index' })
    expect(hireRowWords({ step: 'skill', target: 'draft-review' })).toEqual({ what: 'Draft review (skill)', raw: 'draft-review' })
    expect(hireRowWords({ step: 'permission', target: 'compute.use' })).toEqual({ what: 'Use compute (permission)', raw: 'compute.use' })
    expect(hireRowWords({ step: 'permission', target: 'permission-cards' }).raw).toBeUndefined()
    expect(hireRowWords({ step: 'lock', target: '.k2/agent/template.lock' }).what).toBe('Template record')
    expect(hireRowWords({ step: 'hello', target: 'thread' })).toEqual({ what: 'Says hello in its Thread' })
    // Add to K2 doesn't repeat the folder's path.
    expect(hireRowWords({ step: 'register', target: '/r/agents/ada' })).toEqual({ what: 'Adds it to your agents in K2' })
  })

  it('a row says its state once: the reason when there is one', () => {
    expect(rowNote('Will add')).toBe('Will add')
    expect(rowNote('Already there', 'one with this name is already there')).toBe('One with this name is already there')
  })
})
