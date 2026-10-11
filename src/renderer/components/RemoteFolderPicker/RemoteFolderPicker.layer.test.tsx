// @vitest-environment jsdom
//
// Rosson's bug: on a REMOTE server, Settings → Agent → Identity → Upload
// opened the host image picker "on a different page" — the picker
// (z-60) rendered BEHIND the opaque Settings overlay (z-200), so it only
// showed after leaving Settings and the agent image could never be set.
// These tests pin the stacking order: the picker must paint above the
// Settings overlay, and App.tsx must keep using the shared layer constant.

import { describe, it, expect, vi, afterEach } from 'vitest'
import React from 'react'
import { readFileSync } from 'fs'
import { resolve } from 'path'
import { render, screen, cleanup, act } from '@testing-library/react'

const daemonCliGetMock = vi.fn<(route: string, params?: unknown) => Promise<unknown>>()
vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly((route: string, params?: unknown) => daemonCliGetMock(route, params)),
    daemonCliPost: primaryOnly(() => {
      throw new Error('unexpected daemonCliPost')
    }),
  }
})
vi.mock('@/lib/server-capabilities', () => ({
  useServerSupports: () => true,
  featureMinVersion: () => '0.0.0',
}))

import RemoteFolderPicker from './RemoteFolderPicker'
import { useRemoteFolderPickerStore } from '@/stores/remote-folder-picker'
import { HOST_PICKER_Z, SETTINGS_OVERLAY_Z } from '@/lib/overlay-layers'

afterEach(() => {
  act(() => useRemoteFolderPickerStore.getState().cancel())
  cleanup()
  daemonCliGetMock.mockReset()
})

describe('RemoteFolderPicker stacking', () => {
  it('paints above the Settings overlay when opened for an agent image', async () => {
    daemonCliGetMock.mockImplementation(async (route: string) => {
      if (route === 'fs/info') return { home: '/home/k2', separator: '/', os: 'linux' }
      if (route === 'fs/read-dir') return []
      throw new Error(`unexpected route ${route}`)
    })
    render(<RemoteFolderPicker />)
    act(() => {
      void useRemoteFolderPickerStore.getState().open({ mode: 'file', title: 'Choose Image on Host' })
    })
    const root = await screen.findByTestId('remote-folder-picker')
    const z = Number(root.style.zIndex)
    expect(Number.isFinite(z)).toBe(true)
    expect(z).toBe(HOST_PICKER_Z)
    expect(z).toBeGreaterThan(SETTINGS_OVERLAY_Z)
    // No leftover Tailwind z class that could fight the inline layer.
    expect(root.className).not.toMatch(/\bz-\[/)
  })

  it('App.tsx Settings overlay uses the shared layer constant', () => {
    const src = readFileSync(resolve(__dirname, '../../App.tsx'), 'utf8')
    expect(src).toContain('style={{ zIndex: SETTINGS_OVERLAY_Z }}')
    expect(src).not.toContain('fixed inset-0 z-[200]')
  })
})
