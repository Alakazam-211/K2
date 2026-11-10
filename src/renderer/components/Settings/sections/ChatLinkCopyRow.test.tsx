// @vitest-environment jsdom
// Settings → General → "Show a copy button beside links on hover" (Rosson
// 2026-11-04): off by default, this install's localStorage only, next to the
// font rows.
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import { ChatLinkCopyRow, CHAT_LINK_COPY_LABEL } from './ChatLinkCopyRow'
import {
  LS_CHAT_LINK_COPY_BUTTON,
  readChatLinkCopyButton,
  useChatLinkCopyStore,
  writeChatLinkCopyButton,
} from '@/stores/chat-link-copy'

beforeEach(() => {
  localStorage.removeItem(LS_CHAT_LINK_COPY_BUTTON)
  useChatLinkCopyStore.setState({ enabled: readChatLinkCopyButton() })
})

afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
  localStorage.removeItem(LS_CHAT_LINK_COPY_BUTTON)
  useChatLinkCopyStore.setState({ enabled: false })
})

describe('chat link copy setting storage', () => {
  it('is off when unset, and only "1" means on', () => {
    expect(readChatLinkCopyButton()).toBe(false)
    localStorage.setItem(LS_CHAT_LINK_COPY_BUTTON, 'true')
    expect(readChatLinkCopyButton()).toBe(false)
    localStorage.setItem(LS_CHAT_LINK_COPY_BUTTON, '1')
    expect(readChatLinkCopyButton()).toBe(true)
  })

  it('round-trips on this install, and off clears the key', () => {
    writeChatLinkCopyButton(true)
    expect(localStorage.getItem(LS_CHAT_LINK_COPY_BUTTON)).toBe('1')
    expect(readChatLinkCopyButton()).toBe(true)
    writeChatLinkCopyButton(false)
    expect(localStorage.getItem(LS_CHAT_LINK_COPY_BUTTON)).toBeNull()
    expect(readChatLinkCopyButton()).toBe(false)
  })

  it('a blocked store reads as off, and a blocked write still flips it for this session', () => {
    vi.spyOn(Storage.prototype, 'getItem').mockImplementation(() => {
      throw new Error('blocked')
    })
    vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
      throw new Error('blocked')
    })
    expect(readChatLinkCopyButton()).toBe(false)
    act(() => useChatLinkCopyStore.getState().setEnabled(true))
    expect(useChatLinkCopyStore.getState().enabled).toBe(true)
  })
})

describe('ChatLinkCopyRow', () => {
  it('shows the label, starts off, and the switch turns it on and off', () => {
    render(<ChatLinkCopyRow />)
    expect(screen.getByText('Show a copy button beside links on hover')).toBeTruthy()
    const sw = screen.getByRole('switch', { name: CHAT_LINK_COPY_LABEL })
    expect(sw.getAttribute('aria-checked')).toBe('false')
    act(() => {
      fireEvent.click(sw)
    })
    expect(sw.getAttribute('aria-checked')).toBe('true')
    expect(useChatLinkCopyStore.getState().enabled).toBe(true)
    expect(localStorage.getItem(LS_CHAT_LINK_COPY_BUTTON)).toBe('1')
    act(() => {
      fireEvent.click(sw)
    })
    expect(sw.getAttribute('aria-checked')).toBe('false')
    expect(localStorage.getItem(LS_CHAT_LINK_COPY_BUTTON)).toBeNull()
  })

  it('sits right after the font card on Settings → General, with a search entry', () => {
    const src = readFileSync(join(__dirname, 'GeneralSection.tsx'), 'utf8')
    const body = src.slice(src.indexOf('function GeneralTab('))
    const font = body.indexOf('<AppFontRows where="general"')
    const row = body.indexOf('<ChatLinkCopyRow />')
    const devtools = body.indexOf('<DevToolsRow />')
    expect(font).toBeGreaterThan(0)
    expect(row).toBeGreaterThan(font)
    expect(devtools).toBeGreaterThan(row)
    expect(src).toContain("{ id: 'general.link-copy-button', section: 'general', group: 'General', label: 'Show a copy button beside links on hover'")
    render(<ChatLinkCopyRow />)
    expect(document.querySelector('[data-settings-id="general.link-copy-button"]')).not.toBeNull()
  })
})
