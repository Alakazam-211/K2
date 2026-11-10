// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { Button, ProgressiveHeaderBlur, Segmented, Slider, Toggle } from '@/components/ui'

afterEach(cleanup)

describe('Slider', () => {
  it('fills the track up to its value', () => {
    render(<Slider data-testid="s" min={10} max={20} value={15} onChange={() => {}} />)
    expect(screen.getByTestId('s').style.getPropertyValue('--k2-slider-pct')).toBe('50%')
  })

  it('keeps the fill inside the track for a value out of range', () => {
    render(<Slider data-testid="s" min={0} max={10} value={40} onChange={() => {}} />)
    expect(screen.getByTestId('s').style.getPropertyValue('--k2-slider-pct')).toBe('100%')
  })
})

describe('Toggle', () => {
  it('reports its state and flips on click', () => {
    const onChange = vi.fn()
    render(<Toggle checked={false} onChange={onChange} aria-label="Sound" />)
    const sw = screen.getByRole('switch', { name: 'Sound' })
    expect(sw.getAttribute('aria-checked')).toBe('false')
    fireEvent.click(sw)
    expect(onChange).toHaveBeenCalledWith(true)
  })
})

describe('Toggle size', () => {
  it('marks the compact switch', () => {
    render(<Toggle checked={false} onChange={() => {}} size="sm" aria-label="Small" />)
    expect(screen.getByRole('switch', { name: 'Small' }).getAttribute('data-size')).toBe('sm')
  })
})

describe('Segmented', () => {
  const options = [
    { value: 'a', label: 'A' },
    { value: 'b', label: 'B', disabled: true },
    { value: 'c', label: 'C' },
  ] as const

  it('is a radio group with the current choice checked and the only tab stop', () => {
    render(<Segmented aria-label="Pick" value="c" onChange={() => {}} options={options} />)
    expect(screen.getByRole('radiogroup', { name: 'Pick' })).toBeTruthy()
    const c = screen.getByRole('radio', { name: 'C' })
    expect(c.getAttribute('aria-checked')).toBe('true')
    expect(c.getAttribute('tabindex')).toBe('0')
    expect(screen.getByRole('radio', { name: 'A' }).getAttribute('tabindex')).toBe('-1')
  })

  it('moves one hidden-from-readers indicator onto the selected option', () => {
    const rect = vi.spyOn(Element.prototype, 'getBoundingClientRect').mockImplementation(function (this: Element) {
      return { left: this.getAttribute('role') === 'radio' ? 40 : 10, width: 30 } as DOMRect
    })
    const { container } = render(<Segmented aria-label="Pick" value="c" onChange={() => {}} options={options} />)
    const thumb = container.querySelector<HTMLElement>('.k2-segment-thumb')!
    expect(thumb.getAttribute('aria-hidden')).toBe('true')
    expect(thumb.style.transform).toBe('translateX(30px)')
    expect(thumb.style.width).toBe('30px')
    expect(screen.getAllByRole('radio')).toHaveLength(3)
    rect.mockRestore()
  })

  it('places the thumb from the computed border, so a hairline border lands exactly', () => {
    const rect = vi.spyOn(Element.prototype, 'getBoundingClientRect').mockImplementation(function (this: Element) {
      return { left: this.getAttribute('role') === 'radio' ? 40 : 10, width: 30 } as DOMRect
    })
    const real = window.getComputedStyle
    const style = vi.spyOn(window, 'getComputedStyle').mockImplementation((el, p) => {
      const cs = real(el, p)
      return new Proxy(cs, { get: (t, k) => (k === 'borderLeftWidth' ? '0.5px' : Reflect.get(t, k)) })
    })
    const { container } = render(<Segmented aria-label="Pick" value="c" onChange={() => {}} options={options} />)
    expect(container.querySelector<HTMLElement>('.k2-segment-thumb')!.style.transform).toBe('translateX(29.5px)')
    style.mockRestore()
    rect.mockRestore()
  })

  it('arrow keys skip disabled segments and wrap', () => {
    const onChange = vi.fn()
    render(<Segmented aria-label="Pick" value="a" onChange={onChange} options={options} />)
    fireEvent.keyDown(screen.getByRole('radio', { name: 'A' }), { key: 'ArrowRight' })
    expect(onChange).toHaveBeenLastCalledWith('c')
    fireEvent.keyDown(screen.getByRole('radio', { name: 'A' }), { key: 'ArrowLeft' })
    expect(onChange).toHaveBeenLastCalledWith('c')
  })

  it('with manual activation, arrows move focus and choose nothing', () => {
    const onChange = vi.fn()
    render(<Segmented aria-label="Pick" activation="manual" value="a" onChange={onChange} options={options} />)
    const a = screen.getByRole('radio', { name: 'A' })
    a.focus()
    fireEvent.keyDown(a, { key: 'ArrowRight' })
    expect(document.activeElement).toBe(screen.getByRole('radio', { name: 'C' }))
    expect(onChange).not.toHaveBeenCalled()
    fireEvent.click(screen.getByRole('radio', { name: 'C' }))
    expect(onChange).toHaveBeenCalledWith('c')
  })

  it('with no value, nothing is checked, the indicator hides and the first usable option is the tab stop', () => {
    const { container } = render(<Segmented aria-label="Pick" value={null} onChange={() => {}} options={options} />)
    expect(screen.getAllByRole('radio').filter((r) => r.getAttribute('aria-checked') === 'true')).toHaveLength(0)
    expect(container.querySelector<HTMLElement>('.k2-segment-thumb')!.hidden).toBe(true)
    expect(screen.getByRole('radio', { name: 'A' }).getAttribute('tabindex')).toBe('0')
  })
})

describe('press spring', () => {
  const frames: FrameRequestCallback[] = []
  const settle = (): void => {
    let t = 0
    for (let i = 0; i < 400 && frames.length; i++) frames.shift()!((t += 16))
  }
  const reduced = (on: boolean): void => {
    vi.stubGlobal('matchMedia', (q: string) => ({ matches: on && q.includes('reduced-motion') }))
  }

  beforeEach(() => {
    frames.length = 0
    vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => frames.push(cb))
    vi.stubGlobal('cancelAnimationFrame', () => {})
    reduced(false)
  })
  afterEach(() => vi.unstubAllGlobals())

  const button = (): HTMLElement => {
    render(<Button>Save</Button>)
    return screen.getByRole('button', { name: 'Save' })
  }

  it('grows to the press scale while held and returns after release', () => {
    const el = button()
    el.style.setProperty('--motion-press-scale', '1.045')
    fireEvent.pointerDown(el)
    settle()
    expect(Number(el.style.getPropertyValue('scale'))).toBeCloseTo(1.045, 3)
    fireEvent.pointerUp(el)
    settle()
    expect(el.style.getPropertyValue('scale')).toBe('')
  })

  it('does nothing when the style has no press scale (Terminal)', () => {
    const el = button()
    el.style.setProperty('--motion-press-scale', '1')
    fireEvent.pointerDown(el)
    settle()
    expect(el.style.getPropertyValue('scale')).toBe('')
  })

  it('does nothing under reduced motion', () => {
    reduced(true)
    const el = button()
    el.style.setProperty('--motion-press-scale', '1.045')
    fireEvent.pointerDown(el)
    settle()
    expect(el.style.getPropertyValue('scale')).toBe('')
  })

  it('reaches the Toggle and Segmented segments, and skips a disabled control', () => {
    render(<Toggle checked={false} onChange={() => {}} aria-label="Sw" />)
    render(<Segmented aria-label="P" value="a" onChange={() => {}} options={[{ value: 'a', label: 'A' }]} />)
    render(<Button disabled>Off</Button>)
    const sw = screen.getByRole('switch', { name: 'Sw' })
    const seg = screen.getByRole('radio', { name: 'A' })
    const off = screen.getByRole('button', { name: 'Off' })
    for (const el of [sw, seg, off]) {
      el.style.setProperty('--motion-press-scale', '1.045')
      fireEvent.pointerDown(el)
    }
    settle()
    expect(Number(sw.style.getPropertyValue('scale'))).toBeGreaterThan(1)
    expect(Number(seg.style.getPropertyValue('scale'))).toBeGreaterThan(1)
    expect(off.style.getPropertyValue('scale')).toBe('')
  })
})

describe('ProgressiveHeaderBlur', () => {
  const setup = (): { band: HTMLElement; list: HTMLElement } => {
    const { container } = render(
      <div>
        <ProgressiveHeaderBlur />
        <div data-list="" />
      </div>
    )
    const list = container.querySelector<HTMLElement>('[data-list]')!
    Object.defineProperty(list, 'scrollHeight', { value: 500, configurable: true })
    Object.defineProperty(list, 'clientHeight', { value: 100, configurable: true })
    return { band: container.querySelector<HTMLElement>('.k2-header-blur')!, list }
  }

  it('is hidden from readers and marks itself once the list has scrolled', () => {
    const { band, list } = setup()
    expect(band.getAttribute('aria-hidden')).toBe('true')
    expect(band.dataset.scrolled).toBeUndefined()
    list.scrollTop = 40
    fireEvent.scroll(list)
    expect(band.dataset.scrolled).toBe('1')
    list.scrollTop = 0
    fireEvent.scroll(list)
    expect(band.dataset.scrolled).toBe('0')
  })

  it('ignores a scroller that does not scroll vertically', () => {
    const { band, list } = setup()
    Object.defineProperty(list, 'scrollHeight', { value: 100 })
    fireEvent.scroll(list)
    expect(band.dataset.scrolled).toBeUndefined()
  })
})
