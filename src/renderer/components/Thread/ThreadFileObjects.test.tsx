// @vitest-environment jsdom
// Thread file objects (0.46.1, `thread-attachments-v1`): the chips under a
// Thread message.
//
//   1. One or two files are chips with the name and "size · type · lines";
//      a click opens the file (the provider's opener, with its line).
//   2. `changed`: an amber dot and "changed since sent", and it still opens.
//   3. `gone`: a struck name, "Not found (moved or deleted)", no open, and
//      Find in Files.
//   4. `hidden` (an App guest without the file rights): "A file (not shared
//      with you)", no name, never clickable.
//   5. Three or more: a list card ("N files"), five rows, then Show all.
//   6. ThreadBubble: chips under the bubble; a files-only post has no
//      bubble; without a provider the chips show but don't open.
//   7. The shared opener resolves through the server and opens a file tab
//      at the line; a gone or hidden file never reaches the server.
import { afterEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async () => ({})),
  daemonCliPost: vi.fn(async () => ({})),
  withHostCliSlot: async <T,>(_s: unknown, fn: () => Promise<T>): Promise<T> => fn(),
}))
const activateTab = vi.fn()
vi.mock('@/stores/panels', () => ({ usePanelsStore: { getState: () => ({ activateTab }) } }))

import type { JSX } from 'react'
import { cleanup, fireEvent, render } from '@testing-library/react'
import type { OverlayThreadItem, ThreadFileObject } from '@/components/SessionView/overlayThread'
import { useFileTreeStore } from '@/stores/filetree'
import { findThreadAttachment, openThreadAttachment } from '@/lib/thread-file-link-open'
import { takeFileLine } from '@/lib/file-open-requests'
import {
  ThreadFileActionsProvider,
  ThreadFileObjects,
  fileLineLabel,
  fileMetaLine,
  formatFileSize,
  type ThreadFileActions,
} from './ThreadFileObjects'
import { ThreadBubble } from './ThreadZenKit'

afterEach(() => {
  cleanup()
  activateTab.mockClear()
  vi.restoreAllMocks()
})

/** Pretend to be a Mac (or not) for the platform-aware tooltip. */
function onPlatform(mac: boolean): void {
  vi.spyOn(navigator, 'platform', 'get').mockReturnValue(mac ? 'MacIntel' : 'Linux x86_64')
  vi.spyOn(navigator, 'userAgent', 'get').mockReturnValue(mac ? 'Mozilla/5.0 (Macintosh)' : 'Mozilla/5.0 (X11; Linux x86_64)')
}

const pdf: ThreadFileObject = {
  kind: 'file',
  state: 'ok',
  path: 'docs/report-q3.pdf',
  name: 'report-q3.pdf',
  size: 245812,
  mime: 'application/pdf',
  modifiedAt: 1760790000000,
  sha256: 'ab',
}
const rs: ThreadFileObject = { ...pdf, path: 'src/session.rs', name: 'session.rs', size: 9000, mime: 'text/plain', line: 212, lineEnd: 240 }
const hidden: ThreadFileObject = { kind: 'file', state: 'hidden' }

function withActions(node: JSX.Element, actions: ThreadFileActions) {
  return render(<ThreadFileActionsProvider value={actions}>{node}</ThreadFileActionsProvider>)
}

describe('labels', () => {
  it('formats size, type and lines', () => {
    expect(formatFileSize(0)).toBe('0 B')
    expect(formatFileSize(812)).toBe('812 B')
    expect(formatFileSize(245812)).toBe('240 KB')
    expect(formatFileSize(5 * 1024 * 1024 + 300_000)).toBe('5.3 MB')
    expect(fileLineLabel({ line: 12 })).toBe(':12')
    expect(fileLineLabel({ line: 12, lineEnd: 40 })).toBe(':12–40')
    expect(fileLineLabel({})).toBe('')
    expect(fileMetaLine(pdf)).toBe('240 KB · PDF')
    expect(fileMetaLine(rs)).toBe('9 KB · RS · :212–240')
    expect(fileMetaLine({ ...pdf, state: 'changed' })).toBe('changed since sent')
    expect(fileMetaLine({ ...pdf, state: 'gone' })).toBe('Not found (moved or deleted)')
  })
})

describe('chips', () => {
  it('one or two files are chips that open with their line', () => {
    const open = vi.fn()
    const { container } = withActions(<ThreadFileObjects files={[pdf, rs]} mine={false} />, { open })
    const chips = container.querySelectorAll('[data-thread-file]')
    expect(chips).toHaveLength(2)
    expect(container.querySelector('[data-thread-file-list]')).toBeNull()
    expect(chips[0].textContent).toContain('report-q3.pdf')
    expect(chips[0].textContent).toContain('240 KB · PDF')
    fireEvent.click(chips[1])
    expect(open).toHaveBeenCalledTimes(1)
    expect(open.mock.calls[0][0]).toBe(rs)
    // A plain click opens and shows it.
    expect(open.mock.calls[0][1]).toBeUndefined()
  })

  it('Option-click opens a chip in the background and stops the browser default', () => {
    const open = vi.fn()
    const { container } = withActions(<ThreadFileObjects files={[pdf, rs]} mine={false} />, { open })
    const chip = container.querySelectorAll('[data-thread-file]')[1]
    expect(fireEvent.mouseDown(chip, { altKey: true })).toBe(false)
    expect(fireEvent.click(chip, { altKey: true })).toBe(false)
    expect(open).toHaveBeenCalledTimes(1)
    expect(open.mock.calls[0][0]).toBe(rs)
    expect(open.mock.calls[0][1]).toEqual({ background: true })
  })

  it('the chip tooltip: path and lines, then the gesture (Mac)', () => {
    onPlatform(true)
    const { container } = withActions(<ThreadFileObjects files={[rs]} mine={false} />, { open: vi.fn() })
    const chip = container.querySelector('[data-thread-file]')!
    expect(chip.getAttribute('title')).toBe('src/session.rs:212–240\nClick to open · ⌥-click to open in background')
    expect(chip.getAttribute('aria-label')).toBe('Open session.rs. Option-click to open in background')
  })

  it('the chip tooltip off a Mac says Alt-click', () => {
    onPlatform(false)
    const { container } = withActions(<ThreadFileObjects files={[rs]} mine={false} />, { open: vi.fn() })
    const chip = container.querySelector('[data-thread-file]')!
    expect(chip.getAttribute('title')).toBe('src/session.rs:212–240\nClick to open · Alt-click to open in background')
    expect(chip.getAttribute('aria-label')).toBe('Open session.rs. Alt-click to open in background')
  })

  it('list-card rows take Option-click and carry the same tooltip', () => {
    onPlatform(true)
    const open = vi.fn()
    const files: ThreadFileObject[] = Array.from({ length: 3 }, (_, i) => ({ ...pdf, path: `docs/f${i}.md`, name: `f${i}.md`, line: i + 1 }))
    const { container } = withActions(<ThreadFileObjects files={files} mine={false} />, { open })
    const row = container.querySelector('[data-thread-file="docs/f2.md"]')!
    expect(row.getAttribute('title')).toBe('docs/f2.md:3\nClick to open · ⌥-click to open in background')
    expect(fireEvent.click(row, { altKey: true })).toBe(false)
    expect(open.mock.calls[0]).toEqual([files[2], { background: true }])
    fireEvent.click(row)
    expect(open.mock.calls[1][1]).toBeUndefined()
  })

  it('changed keeps an amber dot and still opens', () => {
    const open = vi.fn()
    const changed = { ...pdf, state: 'changed' as const }
    const { container } = withActions(<ThreadFileObjects files={[changed]} mine />, { open })
    const chip = container.querySelector('[data-thread-file]')!
    expect(chip.getAttribute('data-state')).toBe('changed')
    expect(chip.querySelector('[data-thread-file-changed]')).not.toBeNull()
    expect(chip.textContent).toContain('changed since sent')
    expect(chip.tagName).toBe('BUTTON')
    fireEvent.click(chip)
    expect(open).toHaveBeenCalledTimes(1)
  })

  it('gone strikes the name, does not open, and offers Find in Files', () => {
    const open = vi.fn()
    const find = vi.fn()
    const gone = { ...pdf, state: 'gone' as const }
    const { container, getByText } = withActions(<ThreadFileObjects files={[gone]} mine={false} />, { open, find })
    const chip = container.querySelector('[data-thread-file]')!
    expect(chip.tagName).not.toBe('BUTTON')
    expect((chip.querySelector('[data-thread-file-name]') as HTMLElement).style.textDecoration).toBe('line-through')
    expect(chip.textContent).toContain('Not found (moved or deleted)')
    fireEvent.click(chip)
    expect(open).not.toHaveBeenCalled()
    fireEvent.click(getByText('Find in Files'))
    expect(find).toHaveBeenCalledWith(gone)
  })

  it('hidden shows no name and never opens', () => {
    const open = vi.fn()
    const { container } = withActions(<ThreadFileObjects files={[hidden]} mine={false} />, { open })
    const chip = container.querySelector('[data-thread-file]')!
    expect(chip.getAttribute('data-thread-file')).toBe('hidden')
    // The only words are the placeholder (the rest is the type glyph).
    expect(chip.textContent!.replace(/[-]/g, '')).toBe('A file (not shared with you)')
    expect(chip.querySelector('[data-thread-file-name]')).toBeNull()
    expect(chip.tagName).not.toBe('BUTTON')
    expect(chip.getAttribute('title')).toBeNull()
    fireEvent.click(chip)
    expect(open).not.toHaveBeenCalled()
  })

  it('three or more become a list card, five rows then Show all', () => {
    const open = vi.fn()
    const files: ThreadFileObject[] = Array.from({ length: 7 }, (_, i) => ({ ...pdf, path: `docs/f${i}.md`, name: `f${i}.md` }))
    files[1] = { ...hidden }
    const { container, getByText } = withActions(<ThreadFileObjects files={files} mine={false} />, { open })
    const card = container.querySelector('[data-thread-file-list]')!
    expect(card.textContent).toContain('7 files')
    expect(card.querySelectorAll('[data-thread-file]')).toHaveLength(5)
    expect(card.querySelectorAll('[data-thread-file="hidden"]')).toHaveLength(1)
    fireEvent.click(getByText('Show all 7'))
    expect(card.querySelectorAll('[data-thread-file]')).toHaveLength(7)
    fireEvent.click(card.querySelector('[data-thread-file="docs/f6.md"]')!)
    expect(open.mock.calls[0][0]).toBe(files[6])
    fireEvent.click(card.querySelector('[data-thread-file="hidden"]')!)
    expect(open).toHaveBeenCalledTimes(1)
  })

  it('no files, nothing drawn', () => {
    const { container } = render(<ThreadFileObjects files={[]} mine={false} />)
    expect(container.innerHTML).toBe('')
  })
})

describe('ThreadBubble', () => {
  const item = (body: string, attachments: ThreadFileObject[]): OverlayThreadItem => ({
    collection: 'thread',
    seq: 4,
    id: `d${body.length}`,
    doc: { id: 'd', kind: 'text', from: 'sales', body, created_at: 1, attachments },
  })

  it('draws the chips under the bubble, on the message side', () => {
    const { container } = render(<ThreadBubble item={item('Report is ready', [pdf])} mine={false} author="sales" timeLabel="now" viewer={null} />)
    const bubble = container.querySelector('[data-zen-bubble]')!
    const files = container.querySelector('[data-thread-files]')!
    expect(bubble.textContent).toContain('Report is ready')
    expect(bubble.compareDocumentPosition(files) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    // No provider (Zen, Home): it shows but is not a button.
    expect(files.querySelector('[data-thread-file]')!.tagName).not.toBe('BUTTON')
  })

  it('a files-only post has no empty bubble', () => {
    const { container } = render(<ThreadBubble item={item('', [pdf, rs])} mine={false} author="sales" timeLabel="now" viewer={null} />)
    expect(container.querySelector('[data-zen-bubble]')).toBeNull()
    expect(container.querySelectorAll('[data-thread-file]')).toHaveLength(2)
  })

  it('an older doc with no attachments is unchanged', () => {
    const { container } = render(<ThreadBubble item={item('hi', [])} mine author="You" timeLabel="now" viewer={null} />)
    expect(container.querySelector('[data-zen-bubble]')).not.toBeNull()
    expect(container.querySelector('[data-thread-files]')).toBeNull()
  })
})

describe('the shared opener', () => {
  const fakeRoom = () => {
    const openFileAsTab = vi.fn()
    return { room: { scope: { hostKey: 'local' }, tabs: { getState: () => ({ openFileAsTab }) }, isPrimary: true } as never, openFileAsTab }
  }

  it('resolves the stored path through the server, then opens a tab at the line', async () => {
    const { room, openFileAsTab } = fakeRoom()
    const transport = vi.fn(async () => ({
      root: '/ws',
      prefixes: ['/ws'],
      links: [{ input: 'src/session.rs', linkable: true, kind: 'file' as const, rel: 'src/session.rs', path: '/ws/src/session.rs' }],
    }))
    await expect(openThreadAttachment(room, transport, 'sales', rs)).resolves.toBe(true)
    expect(transport).toHaveBeenCalledWith('sales', ['src/session.rs'])
    expect(openFileAsTab).toHaveBeenCalledWith('/ws/src/session.rs')
    expect(takeFileLine('local', '/ws/src/session.rs')).toEqual({ line: 212 })
  })

  it('Option-click: the same resolve, then the tab opens unselected and the line waits for it', async () => {
    const { room, openFileAsTab } = fakeRoom()
    const transport = vi.fn(async () => ({
      root: '/ws',
      prefixes: ['/ws'],
      links: [{ input: 'src/session.rs', linkable: true, kind: 'file' as const, rel: 'src/session.rs', path: '/ws/src/session.rs' }],
    }))
    await expect(openThreadAttachment(room, transport, 'sales', rs, { background: true })).resolves.toBe(true)
    expect(openFileAsTab).toHaveBeenCalledWith('/ws/src/session.rs', { activate: false })
    expect(activateTab).not.toHaveBeenCalled()
    expect(takeFileLine('local', '/ws/src/session.rs')).toEqual({ line: 212, background: true })
  })

  it('a file the server no longer links does not open', async () => {
    const { room, openFileAsTab } = fakeRoom()
    const transport = vi.fn(async () => ({ root: '/ws', prefixes: [], links: [{ input: 'docs/report-q3.pdf', linkable: false }] }))
    await expect(openThreadAttachment(room, transport, 'sales', pdf)).resolves.toBe(false)
    expect(openFileAsTab).not.toHaveBeenCalled()
  })

  it('gone and hidden never reach the server', async () => {
    const { room } = fakeRoom()
    const transport = vi.fn()
    await expect(openThreadAttachment(room, transport, 'sales', { ...pdf, state: 'gone' })).resolves.toBe(false)
    await expect(openThreadAttachment(room, transport, 'sales', hidden)).resolves.toBe(false)
    expect(transport).not.toHaveBeenCalled()
  })

  it('Find in Files searches the drawer for the name and shows it', () => {
    findThreadAttachment({ isPrimary: true } as never, { ...pdf, state: 'gone' })
    expect(useFileTreeStore.getState().searchQuery).toBe('report-q3.pdf')
    expect(activateTab).toHaveBeenCalledWith('files')
  })
})
