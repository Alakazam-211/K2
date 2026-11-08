// @vitest-environment jsdom
// prd-voice-chat-only-members-v1, shares: Settings → Users and Access shows
// a Voice & Chat only login's shares and saves both lists at once.

import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { VoiceChatSharesEditor } from './VoiceChatSharesEditor'

afterEach(cleanup)

describe('VoiceChatSharesEditor', () => {
  it('shows the current shares and saves the edited lists', async () => {
    const onSave = vi.fn(async () => null)
    render(
      <VoiceChatSharesEditor
        username="reviewer"
        shares={{ agents: [{ id: 'p1', address: 'demo', name: 'Demo' }], gardens: [{ id: 'g-1', name: 'Phone' }] }}
        onSave={onSave}
      />,
    )
    const agents = screen.getByLabelText('reviewer shared agents') as HTMLInputElement
    const gardens = screen.getByLabelText('reviewer shared Gardens') as HTMLInputElement
    expect(agents.value).toBe('demo')
    expect(gardens.value).toBe('g-1')
    const save = screen.getByText('Save shares') as HTMLButtonElement
    expect(save.disabled).toBe(true)
    fireEvent.change(agents, { target: { value: 'demo, sales' } })
    fireEvent.change(gardens, { target: { value: '' } })
    expect(save.disabled).toBe(false)
    fireEvent.click(save)
    await waitFor(() => expect(onSave).toHaveBeenCalledWith(['demo', 'sales'], []))
  })

  it("shows the daemon's refusal", async () => {
    render(
      <VoiceChatSharesEditor
        username="reviewer"
        shares={{ agents: [], gardens: [] }}
        onSave={async () => 'unknown agent: nope'}
      />,
    )
    fireEvent.change(screen.getByLabelText('reviewer shared agents'), { target: { value: 'nope' } })
    fireEvent.click(screen.getByText('Save shares'))
    await waitFor(() => expect(screen.getByText('unknown agent: nope')).toBeTruthy())
  })
})
