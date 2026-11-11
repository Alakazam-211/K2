// @vitest-environment jsdom
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { describe, expect, it, vi } from 'vitest'
import { render, screen } from '@testing-library/react'
import { Button, Callout, DialogFrame, Input, Surface } from '@/components/ui'

vi.mock('@/lib/browser-overlay', () => ({ useBrowserOverlay: () => {} }))

const renderer = join(__dirname, '..', '..')
const globals = readFileSync(join(renderer, 'globals.css'), 'utf8').replace(/\/\*[\s\S]*?\*\//g, ' ')
const tokens = readFileSync(join(renderer, 'styles.generated.css'), 'utf8')

/** Every selector in globals.css that names the chamfer attributes. */
function chamferSelectors(): string[] {
  const out: string[] = []
  for (const m of globals.matchAll(/([^{}]+)\{/g)) {
    const sel = m[1].trim()
    if (/data-k2-chamfer/.test(sel) && !sel.startsWith('@layer')) out.push(sel)
  }
  return out
}

describe('chamfer opt-in', () => {
  it('controls and cards carry the mark', () => {
    render(
      <>
        <Button data-testid="b">x</Button>
        <Input data-testid="i" />
        <Callout data-testid="c" />
        <Surface chamfer data-testid="s" />
      </>
    )
    expect(screen.getByTestId('b').getAttribute('data-k2-chamfer')).toBe('control')
    expect(screen.getByTestId('i').getAttribute('data-k2-chamfer')).toBe('control')
    expect(screen.getByTestId('c').getAttribute('data-k2-chamfer')).toBe('surface')
    expect(screen.getByTestId('s').getAttribute('data-k2-chamfer')).toBe('surface')
  })

  it('structural Surfaces stay square by default', () => {
    render(<Surface data-testid="pane" />)
    expect(screen.getByTestId('pane').hasAttribute('data-k2-chamfer')).toBe(false)
  })

  it("a borderless Surface drops its own cut's edge without taking it from the controls inside", () => {
    render(
      <Surface bordered={false} chamfer data-testid="card">
        <Button>Save</Button>
      </Surface>,
    )
    const card = screen.getByTestId('card')
    // The band width itself, set on the Surface's own element.
    expect(card.className).toContain('[--k2-ch-w:0px]')
    // Never --k2-ch-bw: it is inherited, and each cut control inside derives
    // its own --k2-ch-w from it, so the controls would lose their diagonals.
    expect(card.className).not.toContain('--k2-ch-bw')
    expect(globals).toContain('--k2-ch-w: var(--k2-ch-bw, 1px)')
    // The shared Button needs no override of its own.
    expect(screen.getByRole('button', { name: 'Save' }).className).not.toContain('--k2-ch-bw')
  })

  it('DialogFrame is a chamfered card inside a shadow wrapper', () => {
    render(<DialogFrame data-testid="d" />)
    const frame = screen.getByTestId('d')
    expect(frame.getAttribute('data-k2-chamfer')).toBe('surface')
    expect(frame.parentElement?.hasAttribute('data-k2-chamfer-float')).toBe(true)
  })
})

describe('chamfer CSS scope', () => {
  const selectors = chamferSelectors()

  it('finds the rules', () => {
    expect(selectors.length).toBeGreaterThan(5)
  })

  it('applies only under the Terminal style, never in Glass or Bezel', () => {
    for (const sel of selectors) expect(sel.startsWith('html[data-style="terminal"]'), sel).toBe(true)
  })

  it('never reaches inside a Garden or the Thread box', () => {
    for (const sel of selectors) {
      expect(sel.includes(':not([data-zen-root] *'), sel).toBe(true)
      expect(sel.includes('[data-k2-thread-zen] *'), sel).toBe(true)
    }
  })

  it('Glass and Bezel chamfer tokens are 0', () => {
    for (const style of ['glass', 'bezel']) {
      const block = tokens.slice(tokens.indexOf(`[data-style="${style}"]`))
      const value = block.match(/--chamfer-surface:\s*([^;]+);/)?.[1]
      expect(value, style).toBe('0px')
    }
  })
})

describe('chamfer sizes stay in step', () => {
  it('the Thread cuts match the Terminal tokens', async () => {
    const { CHAMFER_CUT } = await import('@/components/Thread/chamfer')
    const tokens = JSON.parse(readFileSync(join(process.cwd(), 'styles/terminal/tokens.json'), 'utf8'))
    expect(`${CHAMFER_CUT.surface}px`).toBe(tokens.chamfer.surface.$value)
    expect(`${CHAMFER_CUT.button}px`).toBe(tokens.chamfer.control.$value)
  })
})
