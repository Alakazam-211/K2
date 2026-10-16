// @vitest-environment jsdom
// The C2/C3 seam: C2's slot props reach the lazy C3 viewer as-is.
import { afterEach, beforeAll, describe, expect, it, vi } from 'vitest'
import { cleanup, render, screen } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => {}), listen: vi.fn(async () => () => {}) }))

import { ChangesDiffSlot } from './ChangesDiffSlot'
import type { ChangesFile, DiffSides } from '@/stores/git-changes'

beforeAll(() => {
  const rect = { x: 0, y: 0, top: 0, left: 0, bottom: 0, right: 0, width: 0, height: 0, toJSON: () => ({}) }
  Range.prototype.getBoundingClientRect = () => rect as DOMRect
  Range.prototype.getClientRects = () => Object.assign([], { item: () => null }) as unknown as DOMRectList
})

afterEach(cleanup)

const file: ChangesFile = { path: 'a.ts', oldPath: null, group: 'staged', status: 'M', additions: 1, deletions: 1, binary: false, dir: false }
const data: DiffSides = {
  path: 'a.ts',
  oldPath: null,
  mode: 'staged',
  baseRef: null,
  rev: 'r1',
  old: { label: 'HEAD', exists: true, text: 'a\n', size: 2, binary: false, tooLarge: false },
  new: { label: 'INDEX', exists: true, text: 'b\n', size: 2, binary: false, tooLarge: false },
  binary: false,
  tooLarge: false,
  additions: 1,
  deletions: 1,
}

describe('ChangesDiffSlot', () => {
  it('renders the C3 viewer (lazy) with the selected file in split mode', async () => {
    const { container } = render(
      <ChangesDiffSlot
        workspacePath="/abs/ws"
        view="single"
        mode="split"
        base={null}
        file={file}
        files={[file]}
        sides={{ status: 'ready', data }}
        rev="r1"
        loadSides={vi.fn(async () => data)}
      />,
    )
    const ed = await screen.findByTestId('merge-diff-editor')
    expect(ed.getAttribute('data-layout')).toBe('split')
    expect(container.querySelector('[data-changes-diff-slot]')!.getAttribute('data-mode')).toBe('split')
    expect(container.querySelector('.cm-mergeView')).not.toBeNull()
  })
})
