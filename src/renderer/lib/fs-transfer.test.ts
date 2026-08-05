// Hosted-web download path: stream fs/read-range → Blob → browser save.
// Desktop path stays on Tauri local_download_chunk (not re-tested here).

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'

const daemonCliGetMock = vi.fn()
const isWebClientMock = vi.fn(() => true)
const beginMock = vi.fn(() => 'tid-1')
const updateMock = vi.fn()
const endMock = vi.fn()
const isCancelRequestedMock = vi.fn(() => false)
const addToastMock = vi.fn()

vi.mock('./daemon-cli', () => ({
  daemonCliGet: (...args: unknown[]) => daemonCliGetMock(...args),
  daemonCliPost: vi.fn(),
}))

vi.mock('./is-web', () => ({
  isWebClient: () => isWebClientMock(),
}))

vi.mock('@/stores/toast', () => ({
  useToastStore: {
    getState: () => ({ addToast: addToastMock }),
  },
}))

vi.mock('@/stores/transfer-progress', () => ({
  useTransferProgressStore: {
    getState: () => ({
      begin: beginMock,
      update: updateMock,
      end: endMock,
      isCancelRequested: isCancelRequestedMock,
    }),
  },
}))

// Avoid Tauri invoke if desktop path is accidentally hit.
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async () => {
    throw new Error('invoke should not be called on web download path')
  }),
}))

import {
  base64ToUint8Array,
  downloadFile,
  saveBlobInBrowser,
  triggerBrowserDownload,
} from './fs-transfer'

describe('base64ToUint8Array', () => {
  it('decodes standard base64 to raw bytes', () => {
    // "hi" → aGk=
    const bytes = base64ToUint8Array('aGk=')
    expect(Array.from(bytes)).toEqual([104, 105])
  })
})

describe('downloadFile (hosted web)', () => {
  beforeEach(() => {
    isWebClientMock.mockReturnValue(true)
    daemonCliGetMock.mockReset()
    beginMock.mockClear()
    updateMock.mockClear()
    endMock.mockClear()
    isCancelRequestedMock.mockReset()
    isCancelRequestedMock.mockReturnValue(false)
    addToastMock.mockClear()
  })

  afterEach(() => {
    vi.unstubAllGlobals()
  })

  it('streams read-range slices, saves via blob, never invokes Tauri', async () => {
    // Two chunks of "hi" and "!" — total "hi!"
    daemonCliGetMock
      .mockResolvedValueOnce({
        base64: 'aGk=', // hi
        len: 2,
        size: 3,
        eof: false,
      })
      .mockResolvedValueOnce({
        base64: 'IQ==', // !
        len: 1,
        size: 3,
        eof: true,
      })

    const write = vi.fn(async () => {})
    const close = vi.fn(async () => {})
    const showSaveFilePicker = vi.fn(async () => ({
      createWritable: async () => ({ write, close }),
    }))
    vi.stubGlobal('showSaveFilePicker', showSaveFilePicker)

    const result = await downloadFile('/srv/ws/notes.txt')

    expect(result).toBe('notes.txt')
    expect(daemonCliGetMock).toHaveBeenCalledTimes(2)
    expect(daemonCliGetMock).toHaveBeenNthCalledWith(1, 'fs/read-range', {
      path: '/srv/ws/notes.txt',
      offset: 0,
      len: 8 * 1024 * 1024,
    })
    expect(daemonCliGetMock).toHaveBeenNthCalledWith(2, 'fs/read-range', {
      path: '/srv/ws/notes.txt',
      offset: 2,
      len: 8 * 1024 * 1024,
    })
    expect(showSaveFilePicker).toHaveBeenCalledWith({ suggestedName: 'notes.txt' })
    expect(write).toHaveBeenCalledTimes(1)
    const blob = write.mock.calls[0][0] as Blob
    expect(blob).toBeInstanceOf(Blob)
    expect(blob.size).toBe(3)
    expect(addToastMock).toHaveBeenCalledWith(
      expect.stringMatching(/Downloaded.*notes\.txt/),
      'success',
      4000,
    )
    expect(endMock).toHaveBeenCalledWith('tid-1')
  })

  it('cancels mid-stream without saving', async () => {
    isCancelRequestedMock.mockReturnValueOnce(false).mockReturnValue(true)
    daemonCliGetMock.mockResolvedValue({
      base64: 'aGk=',
      len: 2,
      size: 100,
      eof: false,
    })

    const result = await downloadFile('/srv/ws/big.bin')
    expect(result).toBeNull()
    expect(addToastMock).toHaveBeenCalledWith('Download cancelled', 'info', 3000)
    expect(endMock).toHaveBeenCalled()
  })
})

describe('triggerBrowserDownload / saveBlobInBrowser', () => {
  it('falls back to anchor download when showSaveFilePicker is absent', async () => {
    const click = vi.fn()
    const remove = vi.fn()
    const appendChild = vi.fn()
    const createElement = vi.fn(() => ({
      href: '',
      download: '',
      rel: '',
      style: { display: '' },
      click,
      remove,
    }))
    const createObjectURL = vi.fn(() => 'blob:test')
    const revokeObjectURL = vi.fn()

    vi.stubGlobal('document', {
      createElement,
      body: { appendChild },
    })
    vi.stubGlobal('URL', { createObjectURL, revokeObjectURL })
    // Ensure no File System Access API.
    vi.stubGlobal('showSaveFilePicker', undefined)

    const name = await saveBlobInBrowser(new Blob([new Uint8Array([1, 2])]), 'x.bin')
    expect(name).toBe('x.bin')
    expect(createObjectURL).toHaveBeenCalled()
    expect(click).toHaveBeenCalled()
    expect(appendChild).toHaveBeenCalled()
  })

  it('triggerBrowserDownload sets download attribute', () => {
    const click = vi.fn()
    const remove = vi.fn()
    const el = {
      href: '',
      download: '',
      rel: '',
      style: { display: '' },
      click,
      remove,
    }
    vi.stubGlobal('document', {
      createElement: () => el,
      body: { appendChild: vi.fn() },
    })
    vi.stubGlobal('URL', {
      createObjectURL: () => 'blob:x',
      revokeObjectURL: vi.fn(),
    })
    triggerBrowserDownload(new Blob(['a']), 'a.txt')
    expect(el.download).toBe('a.txt')
    expect(click).toHaveBeenCalled()
  })
})
