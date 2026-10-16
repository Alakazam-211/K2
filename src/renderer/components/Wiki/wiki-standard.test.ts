// prd-wiki-3d-view-v1 §12: the standard Wiki view's list grouping and
// backlinks (pure).

import { describe, expect, it } from 'vitest'
import type { WikiIndex, WikiLink, WikiNode } from './wiki-api'
import type { WikiGraphOpts } from './wiki-graph-data'
import {
  bareWikiNoteId,
  groupWikiNotes,
  wikiBacklinks,
  wikiListNotes,
  wikiNoteFolder,
  wikiTitleCategory,
} from './wiki-standard'

function note(id: string, title: string, extra: Partial<WikiNode> = {}): WikiNode {
  return { id, title, aliases: [], tags: [], path: id, exists: true, ...extra }
}

const OPTS: WikiGraphOpts = {
  mode: 'global',
  k2Lens: 'projects',
  focusGroupFilter: 'all',
  projectFilter: 'all',
  search: '',
  localCenterId: null,
}

function index(nodes: WikiNode[], links: WikiLink[] = []): WikiIndex {
  return { workspacePath: '/w', generatedAt: '', nodes, links, noteCount: nodes.length, scope: 'workspace' }
}

const shape = (groups: ReturnType<typeof groupWikiNotes>) =>
  groups.map((g) => [g.label, g.notes.map((n) => n.title)])

describe('helpers', () => {
  it('bare id, folder and title category', () => {
    expect(bareWikiNoteId('ws1::sub/A.md')).toBe('sub/A.md')
    expect(bareWikiNoteId('A.md')).toBe('A.md')
    expect(wikiNoteFolder(note('ws1::a/b/C.md', 'C'))).toBe('a/b')
    expect(wikiNoteFolder(note('C.md', 'C'))).toBe('')
    expect(wikiTitleCategory('Feature - Tickets Board')).toBe('Feature')
    expect(wikiTitleCategory('Bug - 0.40.145 main-thread abort')).toBe('Bug')
    expect(wikiTitleCategory('Home')).toBeNull()
    expect(wikiTitleCategory('A-B testing')).toBeNull()
  })
})

describe('groupWikiNotes (one workspace)', () => {
  it('Home on top, then categories, then folders, single-note categories in Other', () => {
    const notes = [
      note('Feature - B.md', 'Feature - B'),
      note('Home.md', 'Home'),
      note('Feature - A.md', 'Feature - A'),
      note('Bug - X.md', 'Bug - X'),
      note('Bug - Y.md', 'Bug - Y'),
      note('Release - Z.md', 'Release - Z'),
      note('Loose.md', 'Loose'),
      note('.archive/Old.md', 'Old'),
    ]
    expect(shape(groupWikiNotes(notes, false))).toEqual([
      [null, ['Home']],
      ['Bug', ['Bug - X', 'Bug - Y']],
      ['Feature', ['Feature - A', 'Feature - B']],
      ['.archive/', ['Old']],
      ['Other', ['Loose', 'Release - Z']],
    ])
  })

  it('a flat wiki with no categories is one unlabelled list', () => {
    const groups = groupWikiNotes([note('b.md', 'b'), note('a.md', 'a')], false)
    expect(shape(groups)).toEqual([[null, ['a', 'b']]])
  })
})

describe('groupWikiNotes (All workspaces)', () => {
  it('one group per workspace, by name, Home first', () => {
    const notes = [
      note('w2::Z.md', 'Zed', { workspaceId: 'w2', workspaceName: 'bravo' }),
      note('w1::B.md', 'Beta', { workspaceId: 'w1', workspaceName: 'alpha' }),
      note('w1::Home.md', 'Home', { workspaceId: 'w1', workspaceName: 'alpha' }),
      note('w2::Home.md', 'Home', { workspaceId: 'w2', workspaceName: 'bravo' }),
    ]
    expect(shape(groupWikiNotes(notes, true))).toEqual([
      ['alpha', ['Home', 'Beta']],
      ['bravo', ['Home', 'Zed']],
    ])
  })
})

describe('wikiListNotes', () => {
  const idx = index(
    [note('Home.md', 'Home'), note('A.md', 'Alpha', { tags: ['infra'] }), note('B.md', 'Beta'), note('C.md', 'Gamma')],
    [
      { source: 'Home.md', target: 'A.md' },
      { source: 'A.md', target: 'B.md' },
      { source: 'B.md', target: 'C.md' },
      { source: 'A.md', target: 'Ghost.md', missing: true },
    ],
  )

  it('lists real articles only (no missing stubs)', () => {
    expect(wikiListNotes(idx, OPTS).map((n) => n.id)).toEqual(['Home.md', 'A.md', 'B.md', 'C.md'])
  })

  it('search lists matches only, not their graph neighbours', () => {
    expect(wikiListNotes(idx, { ...OPTS, search: 'infra' }).map((n) => n.id)).toEqual(['A.md'])
  })

  it('Local follows the neighbourhood of the centre (2 hops)', () => {
    expect(wikiListNotes(idx, { ...OPTS, mode: 'local', localCenterId: 'C.md' }).map((n) => n.id)).toEqual([
      'A.md',
      'B.md',
      'C.md',
    ])
  })

  it('excludes fleet hubs and org nodes', () => {
    const fleet: WikiIndex = {
      ...index([
        note('w1::__workspace__', 'alpha', { kind: 'workspaceHub', workspaceId: 'w1' }),
        note('__project__::p1', 'Proj', { kind: 'project', projectId: 'p1' }),
        note('w1::Home.md', 'Home', { workspaceId: 'w1' }),
      ]),
      scope: 'k2',
    }
    expect(wikiListNotes(fleet, { ...OPTS, mode: 'k2' }).map((n) => n.id)).toEqual(['w1::Home.md'])
  })
})

describe('wikiBacklinks', () => {
  it('wikilink sources only, existing, no self-links, de-duplicated, by title', () => {
    const idx = index(
      [note('Home.md', 'Home'), note('A.md', 'Zulu'), note('B.md', 'Alpha'), note('T.md', 'Target')],
      [
        { source: 'A.md', target: 'T.md' },
        { source: 'A.md', target: 'T.md' },
        { source: 'B.md', target: 'T.md', kind: 'wikilink' },
        { source: 'T.md', target: 'T.md' },
        { source: 'Missing.md', target: 'T.md' },
        { source: 'w1::__workspace__', target: 'T.md', kind: 'workspaceHub' },
        { source: 'Home.md', target: 'A.md' },
      ],
    )
    expect(wikiBacklinks(idx, 'T.md').map((n) => n.title)).toEqual(['Alpha', 'Zulu'])
    expect(wikiBacklinks(idx, 'A.md').map((n) => n.title)).toEqual(['Home'])
    expect(wikiBacklinks(idx, 'Home.md')).toEqual([])
  })
})
