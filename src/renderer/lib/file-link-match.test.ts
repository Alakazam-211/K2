import { describe, expect, it } from 'vitest'
import { findFilePaths, matchWholeFilePath } from './file-link-match'

/** Just the printed paths (+ :line:col) found in `text`. */
function found(text: string, prefixes?: string[]): string[] {
  return findFilePaths(text, { prefixes }).map((m) => {
    expect(text.slice(m.start, m.end).startsWith(m.path)).toBe(true)
    return m.path + (m.line !== undefined ? `:${m.line}` : '') + (m.col !== undefined ? `:${m.col}` : '')
  })
}

describe('findFilePaths (prose)', () => {
  it('finds workspace-relative paths with an extension', () => {
    expect(found('I changed src/foo.ts and crates/k2-core/src/lib.rs today')).toEqual([
      'src/foo.ts',
      'crates/k2-core/src/lib.rs',
    ])
  })

  it('keeps :line and :line:col and gives exact offsets', () => {
    const text = 'see crates/x/y.rs:42 and src/a.tsx:7:3.'
    const ms = findFilePaths(text)
    expect(ms.map((m) => [m.path, m.line, m.col])).toEqual([
      ['crates/x/y.rs', 42, undefined],
      ['src/a.tsx', 7, 3],
    ])
    expect(text.slice(ms[0].start, ms[0].end)).toBe('crates/x/y.rs:42')
    expect(text.slice(ms[1].start, ms[1].end)).toBe('src/a.tsx:7:3')
  })

  it('finds absolute and home paths', () => {
    expect(found('wrote /Users/me/ws/notes.md and ~/ws/plan.txt')).toEqual(['/Users/me/ws/notes.md', '~/ws/plan.txt'])
  })

  it('finds ./ and ../ paths and a folder with a trailing slash', () => {
    expect(found('run ./build.sh, then look in src/renderer/ or ../other/x.rs')).toEqual([
      './build.sh',
      'src/renderer/',
      '../other/x.rs',
    ])
  })

  it('drops sentence punctuation and brackets around a path', () => {
    expect(found('Done (src/a.ts). Also "src/b.ts", src/c.ts; and src/d.ts!')).toEqual([
      'src/a.ts',
      'src/b.ts',
      'src/c.ts',
      'src/d.ts',
    ])
  })

  it('does not treat a/b prose as a path', () => {
    expect(found('the client/server split, and/or read/write, yes/no, 1/2 of it, K2/SO')).toEqual([])
  })

  it('never matches inside a URL, with or without a scheme', () => {
    expect(found('see https://github.com/Alakazam-211/K2/blob/main/src/a.ts and http://x.dev/a/b.md')).toEqual([])
    expect(found('see github.com/Alakazam-211/K2.git and k2.dev/docs/start.md')).toEqual([])
    expect(found('file:///Users/me/a.ts')).toEqual([])
  })

  it('does not start a path in the middle of a word', () => {
    expect(found('foo/src/a.ts')).toEqual(['foo/src/a.ts'])
    expect(found('abc:src/a.ts')).toEqual([])
    expect(found('user@host:src/a.ts')).toEqual([])
    expect(found('C:src/a.ts')).toEqual([])
  })

  it('ignores versions, single segments and bare names in prose', () => {
    expect(found('shipped 0.46.1/0.46.2, see README.md and /tmp and docs/')).toEqual([])
  })

  it('needs a real segment, not punctuation', () => {
    expect(found('a / b and ../ and ./ and //')).toEqual([])
  })

  it('takes a root prefix with spaces whole', () => {
    const root = '/Users/z3thon/DevProjects/Alakazam Labs/K2'
    expect(found(`wrote ${root}/src/foo.ts:12 today`, [root])).toEqual([`${root}/src/foo.ts:12`])
    // Without the prefix the space splits it (and both halves are junk the
    // daemon refuses, never a wrong link).
    expect(found(`wrote ${root}/src/foo.ts today`)).toEqual(['/Users/z3thon/DevProjects/Alakazam', 'Labs/K2/src/foo.ts'])
    expect(found('see ~/DevProjects/Alakazam Labs/K2/a.md', ['~/DevProjects/Alakazam Labs/K2'])).toEqual([
      '~/DevProjects/Alakazam Labs/K2/a.md',
    ])
  })

  it('a prefix must end at a path boundary', () => {
    expect(found('/srv/wsX/a.ts', ['/srv/ws'])).toEqual(['/srv/wsX/a.ts'])
  })

  it('handles dotfiles and scoped packages', () => {
    expect(found('edit .k2/wiki/_Index.md and node_modules/@scope/pkg/index.js')).toEqual([
      '.k2/wiki/_Index.md',
      'node_modules/@scope/pkg/index.js',
    ])
  })

  it('rejects a bad :line', () => {
    expect(found('src/a.ts:0 is not a line')).toEqual(['src/a.ts'])
  })
})

describe('matchWholeFilePath (inline code)', () => {
  it('takes a relative path, a folder and a bare file name', () => {
    expect(matchWholeFilePath('src/renderer/components')).toEqual({ path: 'src/renderer/components' })
    expect(matchWholeFilePath('Cargo.toml')).toEqual({ path: 'Cargo.toml' })
    expect(matchWholeFilePath(' crates/x/y.rs:42 ')).toEqual({ path: 'crates/x/y.rs', line: 42 })
    expect(matchWholeFilePath('src/a.ts:7:3')).toEqual({ path: 'src/a.ts', line: 7, col: 3 })
    expect(matchWholeFilePath('.env.local')).toEqual({ path: '.env.local' })
  })

  it('takes an absolute path with spaces in it', () => {
    expect(matchWholeFilePath('/Users/z3thon/DevProjects/Alakazam Labs/K2/src/a.ts:9')).toEqual({
      path: '/Users/z3thon/DevProjects/Alakazam Labs/K2/src/a.ts',
      line: 9,
    })
    expect(matchWholeFilePath('~/DevProjects/Alakazam Labs/K2/')).toEqual({ path: '~/DevProjects/Alakazam Labs/K2/' })
  })

  it('refuses code that is not just a path', () => {
    for (const code of [
      'k2 thread k2 "hi"',
      'cargo test -p k2-core',
      '/usr/bin/env -S bash',
      'foo()',
      'a.b(c)',
      'npm',
      '0.46.1',
      'https://k2.dev/docs/a.md',
      'github.com/Alakazam-211/K2',
      'GET /cli/fs/stat?path=x',
      'line1\nline2',
      '~root/a.ts',
      '/',
      '/tmp',
      './',
      'a:b',
      'src/a.ts:0',
      '/a/b  c',
    ]) {
      expect([code, matchWholeFilePath(code)]).toEqual([code, null])
    }
  })
})
