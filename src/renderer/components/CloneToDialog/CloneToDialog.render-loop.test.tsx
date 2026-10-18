// @vitest-environment jsdom
// 0.46.0 field crash: React #185 "Maximum update depth exceeded" in the
// Clone dialog's `.env` block. Its selector returned a fresh `[]` while
// `preview` (or `preview.envFiles`) was missing, so useSyncExternalStore saw
// a new snapshot on every check and looped. These render the block and the
// whole options panel in exactly that state and fail on any loop report.

import { describe, it, expect, beforeEach, afterEach, vi, type MockInstance } from 'vitest'
import { render, screen, cleanup } from '@testing-library/react'

vi.mock('@/stores/projects', () => ({
  useProjectsStore: { getState: () => ({ projects: [], fetchProjects: vi.fn(), setActiveProject: vi.fn() }) },
}))

import CloneToDialog from './CloneToDialog'
import { EnvFilesBlock } from './CloneOptionsBlocks'
import { useCloneToDialogStore } from '../../stores/clone-to-dialog'
import { normalizePreview, type ClonePreview } from '../../lib/clone-options'

const LOOP = /Maximum update depth|getSnapshot should be cached/

let errorSpy: MockInstance<(...args: unknown[]) => void>

function loopReports(): string[] {
  return errorSpy.mock.calls.map((args) => args.map(String).join(' ')).filter((line) => LOOP.test(line))
}

/** A preview from a daemon that left `envFiles` out entirely. */
function previewWithoutEnvFiles(): ClonePreview {
  const p = normalizePreview({
    threads: { available: true, conversations: 1, docs: 2, participants: ['you'] },
    secrets: { available: false, rows: [], notCarried: [] },
    callerCanCopySecrets: true,
  }) as unknown as Record<string, unknown>
  delete p.envFiles
  return p as unknown as ClonePreview
}

beforeEach(() => {
  useCloneToDialogStore.getState().close()
  errorSpy = vi.spyOn(console, 'error')
})
afterEach(() => {
  cleanup()
  errorSpy.mockRestore()
})

describe('Clone dialog renders without an update loop when envFiles is missing', () => {
  it('EnvFilesBlock renders with preview null', () => {
    expect(useCloneToDialogStore.getState().preview).toBeNull()
    render(<EnvFilesBlock pull={false} />)
    expect(screen.getByTestId('clone-env-block')).toBeTruthy()
    expect(screen.getByTestId('clone-env-files')).toBeTruthy()
    expect(loopReports()).toEqual([])
  })

  it('EnvFilesBlock renders with preview.envFiles undefined', () => {
    useCloneToDialogStore.setState({ preview: previewWithoutEnvFiles() })
    expect(useCloneToDialogStore.getState().preview?.envFiles).toBeUndefined()
    render(<EnvFilesBlock pull />)
    expect(screen.getByTestId('clone-env-block')).toBeTruthy()
    expect(loopReports()).toEqual([])
  })

  it('the whole options panel renders while the preview is still loading', () => {
    useCloneToDialogStore.getState().start({
      projectPath: '/w',
      projectName: 'sales',
      host: null,
      pull: false,
      onConfirm: vi.fn(),
      // Never answers: the dialog stays in "Checking what travels…".
      optionDeps: {
        lookupTarget: () => new Promise(() => {}),
        fetchPreview: () => new Promise(() => {}),
        carryConfirm: async () => 'c',
      },
    })
    expect(useCloneToDialogStore.getState().preview).toBeNull()
    render(<CloneToDialog />)
    expect(screen.getByTestId('clone-env-block')).toBeTruthy()
    expect(screen.getByTestId('clone-thread-block')).toBeTruthy()
    expect(screen.getByText('Checking what travels…')).toBeTruthy()
    expect(loopReports()).toEqual([])
  })

  it('the whole options panel renders with preview.envFiles undefined', () => {
    useCloneToDialogStore.getState().start({
      projectPath: '/w',
      projectName: 'sales',
      host: null,
      pull: true,
      onConfirm: vi.fn(),
    })
    useCloneToDialogStore.setState({ preview: previewWithoutEnvFiles() })
    render(<CloneToDialog />)
    expect(screen.getByTestId('clone-env-block')).toBeTruthy()
    expect(screen.getByTestId('clone-thread-block')).toBeTruthy()
    expect(loopReports()).toEqual([])
  })
})
