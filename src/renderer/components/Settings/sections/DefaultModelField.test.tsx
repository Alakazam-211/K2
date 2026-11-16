// @vitest-environment jsdom
// Settings → Providers / Models "Default model" (0.46.13): unset reads
// "Harness default", the list has no "Server default" row, and a pick is
// saved as `defaultModels.<harness>` (a refusal puts the old value back).
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'

const h = vi.hoisted(() => ({
  settingsGet: vi.fn(async () => ({ defaultModels: {} as Record<string, string> })),
  settingsUpdate: vi.fn(async (u: { defaultModels: Record<string, string> }) => ({
    defaultModels: Object.fromEntries(Object.entries(u.defaultModels).filter(([, v]) => v)),
  })),
}))

vi.mock('@/lib/daemon-settings', () => ({ settingsGet: h.settingsGet, settingsUpdate: h.settingsUpdate }))

import { DefaultModelField, presetModelHarness } from './DefaultModelField'
import { useDefaultModelsStore } from '@/stores/default-models'

describe('Default model (Settings)', () => {
  beforeEach(() => {
    useDefaultModelsStore.setState({ models: {}, loaded: false, error: null })
    h.settingsUpdate.mockClear()
  })
  afterEach(() => cleanup())

  it('shows for harness presets only', () => {
    expect(presetModelHarness('claude --dangerously-skip-permissions')).toBe('claude')
    expect(presetModelHarness('ollama run llama3.2')).toBeNull()
  })

  it('unset reads Harness default; a pick saves this harness’s default', async () => {
    render(<DefaultModelField harness="claude" />)
    const btn = await screen.findByRole('button', { name: /Default model for claude: Harness default/ })
    fireEvent.click(btn)
    const options = await screen.findAllByRole('option')
    expect(options[0].textContent).toContain('Harness default')
    expect(options.some((o) => (o.textContent ?? '').includes('Server default'))).toBe(false)
    expect(screen.getByTestId('settings-model-picker-footer').textContent).toBe('Every new chat on this server starts with this model.')
    fireEvent.click(options.find((o) => o.getAttribute('data-value') === 'sonnet')!)
    await waitFor(() => expect(h.settingsUpdate).toHaveBeenCalledWith({ defaultModels: { claude: 'sonnet' } }))
    expect(await screen.findByRole('button', { name: /Default model for claude: Sonnet 5.5/ })).toBeTruthy()
  })

  it('a refused save puts the old value back and says why', async () => {
    useDefaultModelsStore.setState({ models: { claude: 'opus' }, loaded: true, error: null })
    h.settingsUpdate.mockRejectedValueOnce(new Error('updating "defaultModels" requires the Owner or Admin role'))
    render(<DefaultModelField harness="claude" />)
    fireEvent.click(screen.getByRole('button', { name: /Default model for claude: Opus 5.5/ }))
    fireEvent.click((await screen.findAllByRole('option'))[0])
    expect((await screen.findByRole('alert')).textContent).toContain('Owner or Admin')
    expect(screen.getByRole('button', { name: /Default model for claude: Opus 5.5/ })).toBeTruthy()
  })
})
