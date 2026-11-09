// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { Segmented, Slider, Toggle } from '@/components/ui'

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

  it('arrow keys skip disabled segments and wrap', () => {
    const onChange = vi.fn()
    render(<Segmented aria-label="Pick" value="a" onChange={onChange} options={options} />)
    fireEvent.keyDown(screen.getByRole('radio', { name: 'A' }), { key: 'ArrowRight' })
    expect(onChange).toHaveBeenLastCalledWith('c')
    fireEvent.keyDown(screen.getByRole('radio', { name: 'A' }), { key: 'ArrowLeft' })
    expect(onChange).toHaveBeenLastCalledWith('c')
  })
})
