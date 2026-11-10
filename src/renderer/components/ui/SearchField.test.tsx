// @vitest-environment jsdom
// SearchField: Settings' one search field. The height comes from a token
// (both themes), a × clears the text and refocuses, Escape clears too unless
// the caller owns Escape, and a controlled field's own onChange sees the
// cleared text.
import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { useState } from 'react'
import { SearchField } from './SearchField'

const repo = join(dirname(fileURLToPath(import.meta.url)), '..', '..', '..', '..')

afterEach(() => cleanup())

function Controlled(props: { onChange?: (v: string) => void; onKeyDown?: (e: React.KeyboardEvent) => void; escapeClears?: boolean }): React.JSX.Element {
  const [q, setQ] = useState('')
  return (
    <SearchField
      aria-label="Search things"
      placeholder="Search things"
      value={q}
      escapeClears={props.escapeClears}
      onKeyDown={props.onKeyDown}
      onChange={(e) => {
        setQ(e.target.value)
        props.onChange?.(e.target.value)
      }}
    />
  )
}

const field = (): HTMLInputElement => screen.getByRole('searchbox', { name: 'Search things' }) as HTMLInputElement
const clearBtn = (): HTMLElement | null => screen.queryByRole('button', { name: 'Clear search' })

describe('SearchField', () => {
  it('is a labelled search box at the search height with row-size text and a 16px magnifier', () => {
    render(<Controlled />)
    const input = field()
    expect(input.getAttribute('type')).toBe('search')
    expect(input.className).toContain('h-[calc(var(--control-search-height)*var(--density-scale,1))]')
    expect(input.className).toContain('type-row')
    expect(input.className).not.toContain('text-[11px]')
    expect(input.className).toContain('k2-focus')
    const icon = input.parentElement!.querySelector('svg')!
    expect(icon.getAttribute('aria-hidden')).toBe('true')
    expect(icon.getAttribute('class')).toContain('w-[var(--control-search-icon)]')
  })

  it('both themes set the search height (36px) and the magnifier (16px)', () => {
    for (const style of ['terminal', 'glass']) {
      const t = JSON.parse(readFileSync(join(repo, 'styles', style, 'tokens.json'), 'utf8'))
      expect([style, t.control['search-height'].$value, t.control['search-icon'].$value]).toEqual([style, '36px', '16px'])
    }
    const css = readFileSync(join(repo, 'src', 'renderer', 'styles.generated.css'), 'utf8')
    expect(css).toMatch(/\[data-style="terminal"\] \{[^}]*--control-search-height: 36px;/)
    expect(css).toMatch(/\[data-style="glass"\] \{[^}]*--control-search-height: 36px;/)
  })

  it('shows a × only with text; it clears, refocuses, and the caller’s onChange sees ""', () => {
    const onChange = vi.fn()
    render(<Controlled onChange={onChange} />)
    expect(clearBtn()).toBeNull()
    fireEvent.change(field(), { target: { value: 'atlas' } })
    expect(onChange).toHaveBeenLastCalledWith('atlas')
    const x = clearBtn()!
    expect(x.getAttribute('type')).toBe('button')
    expect(x.hasAttribute('data-k2-focus')).toBe(true)
    fireEvent.click(x)
    expect(field().value).toBe('')
    expect(onChange).toHaveBeenLastCalledWith('')
    expect(document.activeElement).toBe(field())
    expect(clearBtn()).toBeNull()
  })

  it('Escape clears the text and stops there; with no text it passes through', () => {
    const behind = vi.fn()
    render(
      <div onKeyDown={(e) => behind(e.key)}>
        <Controlled />
      </div>,
    )
    fireEvent.change(field(), { target: { value: 'atlas' } })
    fireEvent.keyDown(field(), { key: 'Escape' })
    expect(field().value).toBe('')
    expect(behind).not.toHaveBeenCalled()
    fireEvent.keyDown(field(), { key: 'Escape' })
    expect(behind).toHaveBeenCalledWith('Escape')
  })

  it('leaves Escape alone when the caller owns it (escapeClears off, or the caller handled it)', () => {
    const { unmount } = render(<Controlled escapeClears={false} />)
    fireEvent.change(field(), { target: { value: 'atlas' } })
    fireEvent.keyDown(field(), { key: 'Escape' })
    expect(field().value).toBe('atlas')
    unmount()
    render(<Controlled onKeyDown={(e) => { if (e.key === 'Escape') e.preventDefault() }} />)
    fireEvent.change(field(), { target: { value: 'atlas' } })
    fireEvent.keyDown(field(), { key: 'Escape' })
    expect(field().value).toBe('atlas')
  })

  it('works uncontrolled too, and a disabled field offers no ×', () => {
    const { unmount } = render(<SearchField aria-label="Search things" defaultValue="" />)
    fireEvent.change(field(), { target: { value: 'x' } })
    fireEvent.click(clearBtn()!)
    expect(field().value).toBe('')
    unmount()
    render(<SearchField aria-label="Search things" value="x" disabled onChange={() => {}} />)
    expect(clearBtn()).toBeNull()
  })

  it('forwards the ref and other props to the input; className sizes the box', () => {
    let el: HTMLInputElement | null = null
    render(<SearchField ref={(n) => { el = n }} aria-label="Search things" className="w-full" data-settings-id="x.search" />)
    expect(el).toBe(field())
    expect(field().getAttribute('data-settings-id')).toBe('x.search')
    expect(field().parentElement!.className).toContain('w-full')
  })

  it('Terminal draws a 2px accent ring around a focused search field (the wrapper, since an input has no ::after)', () => {
    const globals = readFileSync(join(repo, 'src', 'renderer', 'globals.css'), 'utf8')
    expect(globals).toMatch(/html\[data-style="terminal"\] \[data-k2-search-field\]:has\(> input:focus-visible\):not\(\[data-zen-root\] \*, \[data-k2-thread-zen\] \*\)::after,\s*html\[data-style="terminal"\] \[data-k2-focus\]:focus-visible[^{]*\{[^}]*--k2-ch-fr: 2px;[^}]*background: var\(--color-accent-text\);/)
  })
})
