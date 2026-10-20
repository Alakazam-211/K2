// @vitest-environment jsdom
// 0.46.3: ticket chat, project chat and the Garden Thread link workspace
// paths like the main Thread, through `WorkspaceFileLinks`. A fake transport
// stands in for POST /cli/fs/resolve-links.
import { afterEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render } from '@testing-library/react'
import { ChatMessageBody } from './ChatMessage'
import { WorkspaceFileLinks } from './WorkspaceFileLinks'
import type { FileLinkOpen, FileLinkTransport } from './file-links'
import type { ServerScope } from '@/kessel/server-scope'

const ROOT = '/srv/ws'
const SCOPE = { hostKey: 'local' } as unknown as ServerScope

function fakeDaemon(): { transport: FileLinkTransport; seen: string[] } {
  const seen: string[] = []
  const transport: FileLinkTransport = vi.fn(async (workspace: string, paths: string[]) => {
    seen.push(workspace)
    return {
      root: ROOT,
      prefixes: [ROOT],
      links: paths.map((p) =>
        p === 'src/main.rs'
          ? { input: p, linkable: true, kind: 'file' as const, rel: p, path: `${ROOT}/${p}` }
          : { input: p, linkable: false },
      ),
    }
  })
  return { transport, seen }
}

async function flush(): Promise<void> {
  for (let i = 0; i < 5; i++) await act(async () => {})
}

afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
})

describe('WorkspaceFileLinks', () => {
  it('links a file in the workspace; a click opens it at its line', async () => {
    const { transport, seen } = fakeDaemon()
    const open = vi.fn<FileLinkOpen>()
    const { container } = render(
      <WorkspaceFileLinks workspace="/srv/ws" scope={SCOPE} transport={transport} open={open}>
        <ChatMessageBody text="Fixed src/main.rs:12 and notes/missing.md." />
      </WorkspaceFileLinks>,
    )
    await flush()
    const links = [...container.querySelectorAll('[role="link"]')]
    expect(links.map((l) => l.textContent)).toEqual(['src/main.rs:12'])
    expect(seen.every((w) => w === '/srv/ws')).toBe(true)
    fireEvent.click(links[0])
    expect(open).toHaveBeenCalledTimes(1)
    expect(open.mock.calls[0][0].path).toBe(`${ROOT}/src/main.rs`)
    expect(open.mock.calls[0][1]).toBe(12)
  })

  it('renders plain text and asks no server without a workspace (an unlinked ticket)', async () => {
    const { transport } = fakeDaemon()
    const { container, getByText } = render(
      <WorkspaceFileLinks workspace={null} scope={SCOPE} transport={transport}>
        <ChatMessageBody text="Fixed src/main.rs today." />
      </WorkspaceFileLinks>,
    )
    await flush()
    expect(container.querySelectorAll('[role="link"]').length).toBe(0)
    expect(getByText(/Fixed src\/main.rs today/)).toBeTruthy()
    expect(transport).not.toHaveBeenCalled()
  })
})
