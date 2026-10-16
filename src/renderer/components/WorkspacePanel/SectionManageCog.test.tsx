// @vitest-environment jsdom
import { describe, it, expect, vi } from 'vitest'
import { render, screen, fireEvent } from '@testing-library/react'
import { SectionManageCog } from './SectionManageCog'

describe('SectionManageCog', () => {
  it('draws the A3 settings gear and opens settings on click', () => {
    const onClick = vi.fn()
    render(<SectionManageCog title="Manage heartbeats" onClick={onClick} />)
    const button = screen.getByTitle('Manage heartbeats')
    const icon = button.querySelector('svg')
    expect(icon?.getAttribute('data-k2-icon')).toBe('settings')
    expect(icon?.getAttribute('width')).toBe('12')
    fireEvent.click(button)
    expect(onClick).toHaveBeenCalledTimes(1)
  })
})
