// @vitest-environment jsdom
// OA5: the Mail tab shows for Owner, Admin and Member logins (and the owner
// token, whose whoami says owner), never for a Voice & Chat only login or an
// unknown role.
import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, render, screen, waitFor } from '@testing-library/react'

const h = vi.hoisted(() => ({ role: null as string | null }))

vi.mock('@/components/Presence/PresenceKickButton', () => ({
  fetchViewerRole: vi.fn(async () => h.role),
}))

import { mailTabShownFor, useMailTabShown } from './mail-tab'

afterEach(cleanup)

function Probe(): React.JSX.Element {
  return <span data-testid="shown">{String(useMailTabShown())}</span>
}

describe('Mail tab gate', () => {
  it('Owner, Admin and Member see it; Voice & Chat only and no role do not', () => {
    expect(['owner', 'admin', 'member'].map(mailTabShownFor)).toEqual([true, true, true])
    expect(['voice_chat', 'viewer', ''].map(mailTabShownFor)).toEqual([false, false, false])
    expect(mailTabShownFor(null)).toBe(false)
  })

  it('the hook follows whoami', async () => {
    h.role = 'member'
    render(<Probe />)
    await waitFor(() => expect(screen.getByTestId('shown').textContent).toBe('true'))
    cleanup()
    h.role = null
    render(<Probe />)
    await waitFor(() => expect(screen.getByTestId('shown').textContent).toBe('false'))
  })
})
