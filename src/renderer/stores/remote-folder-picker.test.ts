// RemoteFolderPicker store — folder-mode default regression guard.
//
// The store grew an optional `open(opts)` for file-selection mode (icon
// upload on a remote host). Existing callers (pick-workspace-folder,
// handle-remote-drop, clone-to) call `open()` bare and MUST keep the
// historical folder-mode contract: resolve chosen path / null on cancel,
// no filter, no title override.

import { describe, it, expect, beforeEach } from 'vitest'

import { useRemoteFolderPickerStore } from './remote-folder-picker'

beforeEach(() => {
  // Settle any leftover open picker so state is pristine per test.
  useRemoteFolderPickerStore.getState().cancel()
})

describe('open() with no options (legacy folder mode)', () => {
  it('opens in folder mode with no accept filter and no title override', () => {
    void useRemoteFolderPickerStore.getState().open()
    const s = useRemoteFolderPickerStore.getState()
    expect(s.isOpen).toBe(true)
    expect(s.mode).toBe('folder')
    expect(s.accept).toBeNull()
    expect(s.title).toBeNull()
  })

  it('select(path) resolves the promise with the path and closes', async () => {
    const promise = useRemoteFolderPickerStore.getState().open()
    useRemoteFolderPickerStore.getState().select('/srv/projects')
    await expect(promise).resolves.toBe('/srv/projects')
    expect(useRemoteFolderPickerStore.getState().isOpen).toBe(false)
  })

  it('cancel() resolves the promise with null and closes', async () => {
    const promise = useRemoteFolderPickerStore.getState().open()
    useRemoteFolderPickerStore.getState().cancel()
    await expect(promise).resolves.toBeNull()
    expect(useRemoteFolderPickerStore.getState().isOpen).toBe(false)
  })

  it('re-opening while open settles the previous promise with null', async () => {
    const first = useRemoteFolderPickerStore.getState().open()
    const second = useRemoteFolderPickerStore.getState().open()
    await expect(first).resolves.toBeNull()
    useRemoteFolderPickerStore.getState().select('/x')
    await expect(second).resolves.toBe('/x')
  })
})

describe('open({ mode: "file", … })', () => {
  it('stores mode, accept, and title for the component', () => {
    const accept = (name: string): boolean => name.endsWith('.png')
    void useRemoteFolderPickerStore
      .getState()
      .open({ mode: 'file', accept, title: 'Choose Image on Host' })
    const s = useRemoteFolderPickerStore.getState()
    expect(s.isOpen).toBe(true)
    expect(s.mode).toBe('file')
    expect(s.accept).toBe(accept)
    expect(s.title).toBe('Choose Image on Host')
  })

  it('select(filePath) resolves with the FILE path', async () => {
    const promise = useRemoteFolderPickerStore.getState().open({ mode: 'file' })
    useRemoteFolderPickerStore.getState().select('/home/u/pic.png')
    await expect(promise).resolves.toBe('/home/u/pic.png')
  })

  it('closing resets mode/accept/title back to folder-mode defaults', async () => {
    const promise = useRemoteFolderPickerStore
      .getState()
      .open({ mode: 'file', accept: () => true, title: 't' })
    useRemoteFolderPickerStore.getState().cancel()
    await promise
    const s = useRemoteFolderPickerStore.getState()
    expect(s.mode).toBe('folder')
    expect(s.accept).toBeNull()
    expect(s.title).toBeNull()
  })
})
