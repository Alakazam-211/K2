// Host-aware icon upload — pick-remote-image tests.
//
// Rosson's bug: connected to a REMOTE host, the icon "Upload" button
// opened the LOCAL macOS Finder. The fix routes remote hosts through the
// RemoteFolderPicker in FILE mode + fs/read-binary. These tests exercise
// the shared `pickIconImage` chokepoint both surfaces call, plus the
// MIME/filter helpers. Failures throw loudly — no swallowed catches.

import { describe, it, expect, vi, beforeEach } from 'vitest'

import {
  isImageFileName,
  imageMimeFromPath,
  pickRemoteImageDataUrl,
  pickIconImage,
} from './pick-remote-image'
// REAL store (zustand works headless) — so these tests also prove the
// helper drives the picker's file mode end-to-end.
import { useRemoteFolderPickerStore } from '@/stores/remote-folder-picker'

// ── module mocks (IO + host + toast) ───────────────────────────────────

const daemonCliGetMock = vi.fn<(route: string, params?: unknown) => Promise<unknown>>()
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: (route: string, params?: unknown) => daemonCliGetMock(route, params),
}))

const hostState = { activeHost: 'local' as string }
vi.mock('@/stores/connect-host', () => ({
  useConnectHostStore: { getState: () => hostState },
}))

const addToastMock = vi.fn()
vi.mock('@/stores/toast', () => ({
  useToastStore: { getState: () => ({ addToast: addToastMock }) },
}))

beforeEach(() => {
  daemonCliGetMock.mockReset()
  addToastMock.mockReset()
  hostState.activeHost = 'local'
  // Settle any picker left open by a prior test.
  useRemoteFolderPickerStore.getState().cancel()
})

// ── filename filter + MIME inference ───────────────────────────────────

describe('isImageFileName', () => {
  it('accepts the icon image extensions, case-insensitively', () => {
    for (const name of [
      'a.png', 'b.jpg', 'c.jpeg', 'd.gif', 'e.webp', 'f.svg', 'g.bmp', 'h.heic',
      'SHOUTY.PNG', 'Mixed.JpEg',
    ]) {
      expect(isImageFileName(name), name).toBe(true)
    }
  })

  it('rejects non-images and extension-less names', () => {
    for (const name of ['notes.txt', 'archive.tar.gz', 'Makefile', 'movie.mp4', 'png', 'x.']) {
      expect(isImageFileName(name), name).toBe(false)
    }
  })
})

describe('imageMimeFromPath', () => {
  it('maps extensions to MIME types', () => {
    expect(imageMimeFromPath('/srv/pics/logo.png')).toBe('image/png')
    expect(imageMimeFromPath('/srv/pics/photo.JPG')).toBe('image/jpeg')
    expect(imageMimeFromPath('C:\\pics\\shot.webp')).toBe('image/webp')
    expect(imageMimeFromPath('/x/vector.svg')).toBe('image/svg+xml')
  })

  it('falls back to image/png for unknown extensions', () => {
    expect(imageMimeFromPath('/x/what.xyz')).toBe('image/png')
  })
})

// ── pickRemoteImageDataUrl ─────────────────────────────────────────────

describe('pickRemoteImageDataUrl', () => {
  it('opens the picker in FILE mode with the image filter and returns a data URL', async () => {
    daemonCliGetMock.mockResolvedValueOnce({ base64: 'QUJD' })

    const promise = pickRemoteImageDataUrl()
    // open() runs synchronously up to the picker promise — the store must
    // now be open in file mode with the image accept predicate + title.
    const s = useRemoteFolderPickerStore.getState()
    expect(s.isOpen).toBe(true)
    expect(s.mode).toBe('file')
    expect(s.title).toBe('Choose Image on Host')
    expect(s.accept).not.toBeNull()
    expect(s.accept!('pic.PNG')).toBe(true)
    expect(s.accept!('notes.txt')).toBe(false)

    // User clicks a file → helper reads bytes over the host-aware route.
    useRemoteFolderPickerStore.getState().select('/remote/home/pic.jpg')
    await expect(promise).resolves.toBe('data:image/jpeg;base64,QUJD')
    expect(daemonCliGetMock).toHaveBeenCalledWith('fs/read-binary', {
      path: '/remote/home/pic.jpg',
    })
  })

  it('returns null on cancel without touching the daemon', async () => {
    const promise = pickRemoteImageDataUrl()
    useRemoteFolderPickerStore.getState().cancel()
    await expect(promise).resolves.toBeNull()
    expect(daemonCliGetMock).not.toHaveBeenCalled()
  })

  it('a failed read (e.g. daemon size cap) toasts and returns null', async () => {
    daemonCliGetMock.mockRejectedValueOnce(new Error('payload too large'))
    const promise = pickRemoteImageDataUrl()
    useRemoteFolderPickerStore.getState().select('/remote/huge.png')
    await expect(promise).resolves.toBeNull()
    expect(addToastMock).toHaveBeenCalledTimes(1)
    expect(addToastMock.mock.calls[0][0]).toContain('payload too large')
    expect(addToastMock.mock.calls[0][1]).toBe('error')
  })
})

// ── pickIconImage (the branch both surfaces call) ──────────────────────

describe('pickIconImage', () => {
  it('local host: clicks the native input and never opens the remote picker', async () => {
    hostState.activeHost = 'local'
    const clickNativeInput = vi.fn()
    const setCropImage = vi.fn()

    await pickIconImage({ clickNativeInput, setCropImage })

    expect(clickNativeInput).toHaveBeenCalledOnce()
    expect(useRemoteFolderPickerStore.getState().isOpen).toBe(false)
    expect(setCropImage).not.toHaveBeenCalled()
    expect(daemonCliGetMock).not.toHaveBeenCalled()
  })

  it('remote host: opens the custom picker (NOT the native input) and feeds the data URL to the crop dialog', async () => {
    hostState.activeHost = 'nsi.k2.dev'
    daemonCliGetMock.mockResolvedValueOnce({ base64: 'aWNvbg==' })
    const clickNativeInput = vi.fn()
    const setCropImage = vi.fn()

    const promise = pickIconImage({ clickNativeInput, setCropImage })
    expect(useRemoteFolderPickerStore.getState().isOpen).toBe(true)
    expect(clickNativeInput).not.toHaveBeenCalled()

    useRemoteFolderPickerStore.getState().select('/home/rosson/avatar.webp')
    await promise

    expect(clickNativeInput).not.toHaveBeenCalled()
    expect(setCropImage).toHaveBeenCalledOnce()
    expect(setCropImage).toHaveBeenCalledWith('data:image/webp;base64,aWNvbg==')
  })

  it('remote host: cancel leaves the crop dialog untouched', async () => {
    hostState.activeHost = 'nsi.k2.dev'
    const clickNativeInput = vi.fn()
    const setCropImage = vi.fn()

    const promise = pickIconImage({ clickNativeInput, setCropImage })
    useRemoteFolderPickerStore.getState().cancel()
    await promise

    expect(clickNativeInput).not.toHaveBeenCalled()
    expect(setCropImage).not.toHaveBeenCalled()
  })
})
