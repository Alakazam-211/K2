// @vitest-environment jsdom
// Settings → Access pages: the link to Logs → Access Audit (0.45.7).
import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => {}), listen: vi.fn(async () => () => {}) }))

import { AccessAuditLink } from './AccessAuditLink'
import { useSettingsStore } from '@/stores/settings'

afterEach(() => cleanup())

describe('AccessAuditLink', () => {
  it('says where the login history is and opens Logs → Access Audit', () => {
    useSettingsStore.setState({ activeSection: 'vault' })
    render(<AccessAuditLink className="mt-1" />)
    const line = document.querySelector('[data-access-audit-link]') as HTMLElement
    expect(line.textContent).toBe('Who signed in, from where and when: Logs → Access Audit')
    expect(line.className).toContain('mt-1')
    const btn = screen.getByRole('button', { name: 'Logs → Access Audit' })
    expect(btn.className).toContain('cursor-pointer')
    fireEvent.click(btn)
    expect(useSettingsStore.getState().activeSection).toBe('access-audit')
  })
})
