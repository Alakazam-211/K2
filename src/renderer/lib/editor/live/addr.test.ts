// The kit's address grammar for a text doc (K2 Cores: kit addr.rs is the one
// grammar). Pinned here:
//   - heading slugs are GitHub-style: lowercased, spaces to '-', punctuation
//     dropped, unicode letters kept, inline code's backticks dropped, repeats
//     in document order get -1, -2; an empty heading has no slug;
//   - addresses: a heading is `@h:<slug>`, a line `L12` (from 1);
//   - parsing takes both, and (for one release, to delete) the old
//     `heading:<name>` and `line:<n>`;
//   - where a person is, as an address: their section's heading, else the line.

import { describe, expect, it } from 'vitest'
import { headingAddr, lineAddr, parseAddr, placeAddr, resolveAddr } from './addr'
import { headingSlugs, slugify } from './heading-slug'

describe('slugify (GitHub-style)', () => {
  it('lowercases, turns spaces to dashes and drops punctuation', () => {
    expect(slugify('Getting Started')).toBe('getting-started')
    expect(slugify('What’s new? (v2.1)')).toBe('whats-new-v21')
    expect(slugify('A -- B')).toBe('a----b')
    expect(slugify('snake_case stays')).toBe('snake_case-stays')
  })
  it('keeps unicode letters and numbers', () => {
    expect(slugify('Café Über 2')).toBe('café-über-2')
    expect(slugify('日本語 の 見出し')).toBe('日本語-の-見出し')
  })
  it('drops inline code’s backticks and emphasis marks, keeping the words', () => {
    expect(slugify('Use `k2 doc say` here')).toBe('use-k2-doc-say-here')
    expect(slugify('**Bold** and [a link](http://x.y)')).toBe('bold-and-a-link')
  })
  it('an empty heading has an empty slug', () => {
    expect(slugify('')).toBe('')
    expect(slugify('  !!  ')).toBe('')
  })
})

describe('headingSlugs', () => {
  it('numbers repeats in document order, outside code fences', () => {
    const text = ['# Guide', '', '## Getting started', 'x', '```', '## not a heading', '```', '## Getting Started', '## Getting started!', '##'].join('\n')
    expect(headingSlugs(text)).toEqual([
      { line: 1, text: 'Guide', slug: 'guide' },
      { line: 3, text: 'Getting started', slug: 'getting-started' },
      { line: 8, text: 'Getting Started', slug: 'getting-started-1' },
      { line: 9, text: 'Getting started!', slug: 'getting-started-2' },
    ])
  })
})

describe('addresses', () => {
  const text = ['# Harbor', '', 'Intro.', '', '## Setup', '', '1. Clone it.', '', '## Setup', 'Again.'].join('\n')
  it('emits @h:<slug> for a heading and L<n> for a line', () => {
    expect(headingAddr('getting-started')).toBe('@h:getting-started')
    expect(lineAddr(12)).toBe('L12')
  })
  it('parses both forms, and the old ones for one release', () => {
    expect(parseAddr('@h:getting-started-1')).toEqual({ kind: 'heading', slug: 'getting-started-1' })
    expect(parseAddr('L12')).toEqual({ kind: 'line', line: 12 })
    expect(parseAddr('L0')).toBeNull()
    expect(parseAddr('heading:Getting%20Started')).toEqual({ kind: 'heading', slug: 'getting-started' })
    expect(parseAddr('line:7')).toEqual({ kind: 'line', line: 7 })
    expect(parseAddr('@scene:main')).toBeNull()
  })
  it('resolves an address to its line and its words in this text', () => {
    expect(resolveAddr(text, '@h:setup-1')).toEqual({ line: 9, label: 'Setup' })
    expect(resolveAddr(text, 'L3')).toEqual({ line: 3, label: 'line 3' })
    expect(resolveAddr(text, '@h:nope')).toBeNull()
  })
  it('a person’s place: their section’s heading (repeats counted), else the line', () => {
    expect(placeAddr(text, 7, true)).toBe('@h:setup')
    expect(placeAddr(text, 10, true)).toBe('@h:setup-1')
    expect(placeAddr(text, 3, true)).toBe('L3')
    expect(placeAddr(text, 4, false)).toBe('L4')
  })
})
