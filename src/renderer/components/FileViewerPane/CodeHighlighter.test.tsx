// @vitest-environment jsdom
//
// The code viewer highlights only allowlisted grammars (lib/shiki-allowlist.ts).
// A language that isn't on the list, or an alias of one, renders as plain,
// escaped text: never blank, never broken.
import React from 'react'
import { afterEach, describe, expect, it } from 'vitest'
import { cleanup, render, waitFor } from '@testing-library/react'
import { CodeViewer, HighlightedCodeBlock, detectLanguage, highlightToHtml, resolveFenceLang } from './CodeHighlighter'

afterEach(cleanup)

const NGINX = 'server {\n  listen 80; # <b>not bold</b>\n}\n'

/** Give the effect time to try a highlight, so a plain result is final. */
async function settle(): Promise<void> {
  await highlightToHtml('x', 'typescript')
  await new Promise((r) => setTimeout(r, 20))
}

describe('CodeHighlighter: left-out languages render plain', () => {
  it.each(['nginx', 'terraform', 'tf', 'hcl', 'toml', 'glsl', 'racket', 'conf'])('fence %s renders plain escaped text', async (lang) => {
    expect(resolveFenceLang(lang)).toBeUndefined()
    const { container } = render(<HighlightedCodeBlock code={NGINX} language={lang} />)
    await settle()
    expect(container.querySelector('.shiki')).toBeNull()
    const code = container.querySelector('pre > code')
    expect(code).not.toBeNull()
    expect(code!.textContent).toBe(NGINX)
    expect(container.querySelector('b')).toBeNull()
  })

  it.each(['nginx.conf', 'main.tf', 'vars.hcl', 'Cargo.toml', 'site.nginx'])('file %s detects no language and renders plain', async (path) => {
    expect(detectLanguage(path)).toBeUndefined()
    const { container } = render(<CodeViewer code={NGINX} language={detectLanguage(path)} />)
    await settle()
    expect(container.querySelector('.shiki')).toBeNull()
    expect(container.querySelector('pre > code')!.textContent).toBe(NGINX)
  })

  it('highlightToHtml refuses a left-out grammar', async () => {
    expect(await highlightToHtml(NGINX, 'nginx')).toBeNull()
    expect(await highlightToHtml(NGINX, 'terraform')).toBeNull()
  })
})

describe('CodeHighlighter: allowlisted languages still highlight', () => {
  it('a ts fence highlights with the viewer theme', async () => {
    expect(resolveFenceLang('ts')).toBe('typescript')
    const { container } = render(<HighlightedCodeBlock code={'const a: number = 1'} language="ts" />)
    await waitFor(() => expect(container.querySelector('pre.shiki')).not.toBeNull())
    expect(container.querySelector('pre.shiki')!.className).toContain('github-dark-default')
    expect(container.textContent).toContain('const a: number = 1')
  })

  it('grammars that embed a left-out grammar still load (cpp, ruby)', async () => {
    expect(await highlightToHtml('int main() { return 0; }', 'cpp')).toContain('class="shiki')
    expect(await highlightToHtml('puts "hi"', 'ruby')).toContain('class="shiki')
    expect(detectLanguage('src/a.cpp')).toBe('cpp')
    expect(detectLanguage('Gemfile')).toBe('ruby')
  })

  it('yaml is allowed: yaml files and fences highlight', async () => {
    expect(detectLanguage('config.yaml')).toBe('yaml')
    expect(resolveFenceLang('yml')).toBe('yaml')
    expect(await highlightToHtml('a: 1', 'yaml')).toContain('class="shiki')
  })

  it('text fences use the plain renderer', async () => {
    expect(resolveFenceLang('txt')).toBe('plaintext')
    expect(await highlightToHtml('<b>x</b>', 'plaintext')).toContain('&#x3C;b>x')
  })
})
