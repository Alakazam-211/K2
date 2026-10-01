// @vitest-environment jsdom
// Home M4 — "Remote rooms (preview)" (MS55, answer Q3): off by default,
// per computer in localStorage, shared across windows, and the Settings row
// flips it.

import { describe, it, expect, beforeEach } from 'vitest'
import { createElement } from 'react'
import { createRoot } from 'react-dom/client'
import { act } from 'react'
import {
  LS_REMOTE_ROOMS_PREVIEW,
  installRemoteRoomsPreviewSync,
  parseRemoteRoomsPreview,
  readRemoteRoomsPreview,
  remoteRoomsPreviewEnabled,
  useRemoteRoomsPreview,
  useRemoteRoomsPreviewStore,
} from './remote-rooms-preview'
import { RemoteRoomsPreviewRow } from '@/components/Settings/sections/RemoteRoomsPreviewRow'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

beforeEach(() => {
  localStorage.clear()
  useRemoteRoomsPreviewStore.setState({ enabled: false })
})

describe('remote rooms preview switch', () => {
  it('is off by default and for anything but "1"', () => {
    expect(readRemoteRoomsPreview()).toBe(false)
    expect(parseRemoteRoomsPreview(null)).toBe(false)
    expect(parseRemoteRoomsPreview('')).toBe(false)
    expect(parseRemoteRoomsPreview('true')).toBe(false)
    expect(parseRemoteRoomsPreview('0')).toBe(false)
    expect(parseRemoteRoomsPreview('1')).toBe(true)
  })

  it('setEnabled writes the per-computer key and the store', () => {
    useRemoteRoomsPreviewStore.getState().setEnabled(true)
    expect(localStorage.getItem(LS_REMOTE_ROOMS_PREVIEW)).toBe('1')
    expect(remoteRoomsPreviewEnabled()).toBe(true)
    expect(readRemoteRoomsPreview()).toBe(true)
    useRemoteRoomsPreviewStore.getState().setEnabled(false)
    expect(localStorage.getItem(LS_REMOTE_ROOMS_PREVIEW)).toBe('0')
    expect(remoteRoomsPreviewEnabled()).toBe(false)
  })

  it('another window turning it on reaches this one (storage event)', () => {
    const target = new EventTarget() as unknown as Window
    const uninstall = installRemoteRoomsPreviewSync(target)
    localStorage.setItem(LS_REMOTE_ROOMS_PREVIEW, '1')
    target.dispatchEvent(new StorageEvent('storage', { key: LS_REMOTE_ROOMS_PREVIEW }))
    expect(remoteRoomsPreviewEnabled()).toBe(true)
    // An unrelated key changes nothing.
    localStorage.setItem(LS_REMOTE_ROOMS_PREVIEW, '0')
    target.dispatchEvent(new StorageEvent('storage', { key: 'k2.homes.v1' }))
    expect(remoteRoomsPreviewEnabled()).toBe(true)
    // A clear() elsewhere (key null) re-reads: off.
    target.dispatchEvent(new StorageEvent('storage', { key: null }))
    expect(remoteRoomsPreviewEnabled()).toBe(false)
    uninstall()
    localStorage.setItem(LS_REMOTE_ROOMS_PREVIEW, '1')
    target.dispatchEvent(new StorageEvent('storage', { key: LS_REMOTE_ROOMS_PREVIEW }))
    expect(remoteRoomsPreviewEnabled()).toBe(false)
  })

  it('the hook re-renders on change', () => {
    const seen: boolean[] = []
    function Probe(): null {
      seen.push(useRemoteRoomsPreview())
      return null
    }
    const el = document.createElement('div')
    const root = createRoot(el)
    act(() => root.render(createElement(Probe)))
    act(() => useRemoteRoomsPreviewStore.getState().setEnabled(true))
    act(() => root.unmount())
    expect(seen[0]).toBe(false)
    expect(seen[seen.length - 1]).toBe(true)
  })

  it('the Settings row shows the switch off and clicking turns it on', () => {
    const el = document.createElement('div')
    document.body.appendChild(el)
    const root = createRoot(el)
    act(() => root.render(createElement(RemoteRoomsPreviewRow)))
    expect(el.textContent).toContain('Remote rooms (preview)')
    const sw = el.querySelector('[aria-label="Remote rooms (preview)"]')
    if (!sw) throw new Error('no switch rendered')
    expect(sw.getAttribute('aria-checked')).toBe('false')
    act(() => (sw as HTMLElement).click())
    expect(remoteRoomsPreviewEnabled()).toBe(true)
    expect(localStorage.getItem(LS_REMOTE_ROOMS_PREVIEW)).toBe('1')
    expect(sw.getAttribute('aria-checked')).toBe('true')
    act(() => root.unmount())
    el.remove()
  })
})
