// @vitest-environment jsdom
// OA6: the Mail page hands @k2/mail-reader K2's own controls and icons. Every
// icon the reader names draws a K2 icon, and the kit's controls are the
// shared primitives (cut corners in Terminal, tiles in Glass).
import { afterEach, describe, expect, it } from 'vitest'
import { cleanup, render, screen } from '@testing-library/react'
import { MAIL_ICON_NAMES, IconButton, MailUiProvider } from '@k2/mail-reader'
import { K2_MAIL_UI, MAIL_ICONS } from './mail-ui'

afterEach(cleanup)

describe('K2_MAIL_UI', () => {
  it('maps every reader icon name to a K2 icon', () => {
    expect(Object.keys(MAIL_ICONS).sort()).toEqual([...MAIL_ICON_NAMES].sort())
    for (const name of MAIL_ICON_NAMES) {
      const { container } = render(<>{K2_MAIL_UI.Icon && <K2_MAIL_UI.Icon name={name} />}</>)
      expect(container.querySelector('svg[data-k2-icon]'), name).toBeTruthy()
      cleanup()
    }
  })

  it('an icon button is the shared Button with a name and a tooltip', () => {
    render(
      <MailUiProvider ui={K2_MAIL_UI}>
        <IconButton icon="archive" label="Archive" data-testid="b" />
      </MailUiProvider>,
    )
    const b = screen.getByTestId('b')
    expect(b.getAttribute('data-k2-chamfer')).toBe('control')
    expect(b.getAttribute('aria-label')).toBe('Archive')
    expect(b.getAttribute('title')).toBe('Archive')
    expect(b.querySelector('svg[data-k2-icon="archive"]')).toBeTruthy()
    expect(b.textContent).toBe('')
  })

  it('panes are tiles and cards are cut', () => {
    expect(K2_MAIL_UI.shape('pane')).toEqual({ 'data-material': 'tile', 'data-clip': '' })
    expect(K2_MAIL_UI.shape('card')).toEqual({ 'data-k2-chamfer': 'surface' })
  })
})
