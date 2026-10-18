// @vitest-environment jsdom
// Thread file links: the renderer marks path candidates, the daemon decides
// (here a fake transport stands in for POST /cli/fs/resolve-links), and only
// linkable answers become links that open on click or Enter.
import { afterEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import { ChatMessageBody } from './ChatMessage'
import {
  FileLinkProvider,
  FileLinkResolver,
  type FileLinkAnswer,
  type FileLinkOpen,
  type FileLinkTransport,
} from './file-links'

const ROOT = '/Users/me/Dev Projects/ws'

/** A daemon stand-in: `inside` maps candidate → answer; everything else is
 *  a bare "not linkable". Records every batch it was sent. */
function fakeDaemon(inside: Record<string, Omit<FileLinkAnswer, 'input' | 'linkable'>>) {
  const batches: string[][] = []
  const transport: FileLinkTransport = vi.fn(async (_workspace: string, paths: string[]) => {
    batches.push(paths)
    return {
      root: ROOT,
      prefixes: [ROOT],
      links: paths.map((p) => (inside[p] ? { input: p, linkable: true, ...inside[p] } : { input: p, linkable: false })),
    }
  })
  return { transport, batches }
}

async function flush(): Promise<void> {
  // The resolver batches on a microtask; the fake answers on another.
  for (let i = 0; i < 5; i++) await act(async () => {})
}

function renderThread(text: string, transport: FileLinkTransport, open: FileLinkOpen = vi.fn()) {
  return render(
    <FileLinkProvider workspace="ws" transport={transport} open={open}>
      <ChatMessageBody text={text} />
    </FileLinkProvider>,
  )
}

afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
})

/** Pretend to be a Mac (or not) for the platform-aware tooltip. */
function onPlatform(mac: boolean): void {
  vi.spyOn(navigator, 'platform', 'get').mockReturnValue(mac ? 'MacIntel' : 'Win32')
  vi.spyOn(navigator, 'userAgent', 'get').mockReturnValue(mac ? 'Mozilla/5.0 (Macintosh)' : 'Mozilla/5.0 (Windows NT 10.0)')
}

describe('Thread file links', () => {
  it('links a workspace file in prose and in inline code; clicking opens it at its line', async () => {
    const { transport } = fakeDaemon({
      'src/a.ts': { kind: 'file', rel: 'src/a.ts', path: `${ROOT}/src/a.ts` },
      'crates/x/y.rs': { kind: 'file', rel: 'crates/x/y.rs', path: `${ROOT}/crates/x/y.rs` },
    })
    const open = vi.fn<FileLinkOpen>()
    const { container } = renderThread('I changed src/a.ts:12:3 and `crates/x/y.rs:42` today.', transport, open)
    await flush()
    const links = [...container.querySelectorAll('[role="link"]')]
    expect(links.map((l) => l.textContent)).toEqual(['src/a.ts:12:3', 'crates/x/y.rs:42'])
    // The inline code stays code, inside the link.
    expect(links[1].querySelector('code')?.textContent).toBe('crates/x/y.rs:42')
    expect(links.every((l) => l.getAttribute('tabindex') === '0')).toBe(true)

    fireEvent.click(links[0])
    expect(open).toHaveBeenCalledTimes(1)
    expect(open.mock.calls[0][0].path).toBe(`${ROOT}/src/a.ts`)
    expect(open.mock.calls[0][1]).toBe(12)
    expect(open.mock.calls[0][2]).toBe(3)

    fireEvent.keyDown(links[1], { key: 'Enter' })
    expect(open).toHaveBeenCalledTimes(2)
    expect(open.mock.calls[1][0].path).toBe(`${ROOT}/crates/x/y.rs`)
    expect(open.mock.calls[1][1]).toBe(42)
    expect(open.mock.calls[1][2]).toBeUndefined()
  })

  it('leaves a path the daemon refuses (outside, missing) as plain text', async () => {
    const { transport, batches } = fakeDaemon({})
    const { container } = renderThread('see /etc/passwd/x.txt and src/missing.ts and `../outside/a.ts`', transport)
    await flush()
    expect(container.querySelectorAll('[role="link"]').length).toBe(0)
    expect(container.textContent).toContain('/etc/passwd/x.txt')
    // They were asked about (the daemon decides), in one batch.
    expect(batches.flat().sort()).toEqual(['../outside/a.ts', '/etc/passwd/x.txt', 'src/missing.ts'])
  })

  it('never links inside a fenced code block, a URL or a markdown link', async () => {
    const { transport, batches } = fakeDaemon({ 'src/a.ts': { kind: 'file', rel: 'src/a.ts', path: `${ROOT}/src/a.ts` } })
    const { container } = renderThread(
      '```\nsrc/a.ts\n```\n\nhttps://github.com/x/src/a.ts and [src/a.ts](https://k2.dev/x)',
      transport,
    )
    await flush()
    expect(container.querySelectorAll('[role="link"]').length).toBe(0)
    expect(batches.flat()).toEqual([])
  })

  it('a folder link opens as a directory', async () => {
    const { transport } = fakeDaemon({ 'src/renderer/': { kind: 'directory', rel: 'src/renderer', path: `${ROOT}/src/renderer` } })
    const open = vi.fn<FileLinkOpen>()
    renderThread('look in src/renderer/ first', transport, open)
    await flush()
    const link = screen.getByRole('link')
    expect(link.getAttribute('data-k2-file-link')).toBe('directory')
    fireEvent.keyDown(link, { key: ' ' })
    expect(open.mock.calls[0][0].kind).toBe('directory')
  })

  it('takes the workspace root with spaces whole once the daemon names it', async () => {
    const abs = `${ROOT}/src/a.ts`
    const { transport } = fakeDaemon({ [abs]: { kind: 'file', rel: 'src/a.ts', path: abs } })
    const { container } = renderThread(`wrote ${abs} just now`, transport)
    await flush()
    const links = [...container.querySelectorAll('[role="link"]')]
    expect(links.map((l) => l.textContent)).toEqual([abs])
  })

  it('without a provider nothing is linked and nothing is asked', async () => {
    const { container } = render(<ChatMessageBody text={'I changed src/a.ts and `src/b.ts`'} />)
    await flush()
    expect(container.querySelectorAll('[role="link"]').length).toBe(0)
    expect(container.querySelector('[data-k2-file-candidate]')).toBeNull()
  })

  it('a refused workspace (unknown Thread) leaves everything plain and stops asking', async () => {
    const transport: FileLinkTransport = vi.fn(async () => {
      throw new Error('404 workspace not found')
    })
    const { container } = renderThread('I changed src/a.ts', transport)
    await flush()
    expect(container.querySelectorAll('[role="link"]').length).toBe(0)
    const calls = (transport as ReturnType<typeof vi.fn>).mock.calls.length
    const r = new FileLinkResolver('import:x', transport)
    r.prime()
    await flush()
    r.request('src/z.ts')
    await flush()
    expect(r.get('src/z.ts')).toBeUndefined()
    expect((transport as ReturnType<typeof vi.fn>).mock.calls.length).toBe(calls + 1)
  })
})

describe('Option/Alt-click and the tooltip', () => {
  const daemonWithA = () => fakeDaemon({ 'src/a.ts': { kind: 'file', rel: 'src/a.ts', path: `${ROOT}/src/a.ts` } })

  it('Option-click opens in the background and stops the browser\'s own Option-click', async () => {
    const { transport } = daemonWithA()
    const open = vi.fn<FileLinkOpen>()
    renderThread('see src/a.ts:12 here', transport, open)
    await flush()
    const link = screen.getByRole('link')
    // Pressing the mouse with Option keeps focus where it is (the composer).
    expect(fireEvent.mouseDown(link, { altKey: true })).toBe(false)
    // fireEvent returns false when the handler called preventDefault.
    expect(fireEvent.click(link, { altKey: true })).toBe(false)
    expect(open).toHaveBeenCalledTimes(1)
    expect(open.mock.calls[0][0].path).toBe(`${ROOT}/src/a.ts`)
    expect(open.mock.calls[0][1]).toBe(12)
    expect(open.mock.calls[0][3]).toEqual({ background: true })
  })

  it('a plain click opens and shows it (no background)', async () => {
    const { transport } = daemonWithA()
    const open = vi.fn<FileLinkOpen>()
    renderThread('see src/a.ts here', transport, open)
    await flush()
    const link = screen.getByRole('link')
    expect(fireEvent.mouseDown(link)).toBe(true)
    fireEvent.click(link)
    expect(open).toHaveBeenCalledTimes(1)
    expect(open.mock.calls[0][3]).toBeUndefined()
  })

  it('Alt+Enter opens in the background too', async () => {
    const { transport } = daemonWithA()
    const open = vi.fn<FileLinkOpen>()
    renderThread('see src/a.ts here', transport, open)
    await flush()
    fireEvent.keyDown(screen.getByRole('link'), { key: 'Enter', altKey: true })
    expect(open.mock.calls[0][3]).toEqual({ background: true })
  })

  it('the tooltip names the path and line, then the gesture, on a Mac', async () => {
    onPlatform(true)
    const { transport } = daemonWithA()
    renderThread('see src/a.ts:12:3 here', transport)
    await flush()
    const link = screen.getByRole('link')
    expect(link.getAttribute('title')).toBe('src/a.ts:12:3\nClick to open · ⌥-click to open in background')
    expect(link.getAttribute('aria-label')).toBe('Open src/a.ts:12:3. Option-click to open in background')
  })

  it('the tooltip says Alt-click off a Mac', async () => {
    onPlatform(false)
    const { transport } = daemonWithA()
    renderThread('see src/a.ts here', transport)
    await flush()
    const link = screen.getByRole('link')
    expect(link.getAttribute('title')).toBe('src/a.ts\nClick to open · Alt-click to open in background')
    expect(link.getAttribute('aria-label')).toBe('Open src/a.ts. Alt-click to open in background')
  })

  it('a folder link shows in Files: no background gesture offered', async () => {
    onPlatform(true)
    const { transport } = fakeDaemon({ 'src/renderer/': { kind: 'directory', rel: 'src/renderer', path: `${ROOT}/src/renderer` } })
    renderThread('look in src/renderer/ first', transport)
    await flush()
    expect(screen.getByRole('link').getAttribute('title')).toBe('Show src/renderer in Files')
  })
})

describe('FileLinkResolver', () => {
  it('batches requests made in one tick and caches answers', async () => {
    const { transport, batches } = fakeDaemon({ 'src/a.ts': { kind: 'file', rel: 'src/a.ts', path: `${ROOT}/src/a.ts` } })
    const r = new FileLinkResolver('ws', transport)
    r.request('src/a.ts')
    r.request('src/b.ts')
    r.request('src/a.ts')
    expect(r.get('src/a.ts')).toBe('pending')
    await flush()
    expect(batches).toEqual([['src/a.ts', 'src/b.ts']])
    expect(r.get('src/a.ts')).toMatchObject({ linkable: true, path: `${ROOT}/src/a.ts` })
    expect(r.get('src/b.ts')).toEqual({ input: 'src/b.ts', linkable: false })
    r.request('src/a.ts')
    await flush()
    expect(batches.length).toBe(1)
    expect(r.prefixes()).toEqual([ROOT])
  })

  it('splits a large batch', async () => {
    const { transport, batches } = fakeDaemon({})
    const r = new FileLinkResolver('ws', transport)
    for (let i = 0; i < 450; i++) r.request(`src/f${i}.ts`)
    await flush()
    expect(batches.map((b) => b.length)).toEqual([200, 200, 50])
  })

  it('a daemon answer missing a candidate counts as not linkable', async () => {
    const transport: FileLinkTransport = async () => ({ root: ROOT, prefixes: [], links: [] })
    const r = new FileLinkResolver('ws', transport)
    r.request('src/a.ts')
    await flush()
    expect(r.get('src/a.ts')).toEqual({ input: 'src/a.ts', linkable: false })
  })
})
