// @vitest-environment jsdom
// The markdown editor reads at the Thread's text width (Rosson, 2026-11-15:
// "the default text width is too narrow"): both read "Thread text width"
// through useThreadTextCapPx, so changing it moves both. Full width is the
// wider of that and 100 characters; code files don't take a measure.

import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { act, cleanup, render, renderHook, screen } from '@testing-library/react'
import { afterEach, describe, expect, it } from 'vitest'
import { useSettingsStore } from '@/stores/settings'
import { THREAD_TEXT_WIDTH_DEFAULT, THREAD_TEXT_WIDTH_DEFAULT_PX } from '@/components/SessionView/threadTextWidth'
import { useThreadTextCapPx } from '@/components/SessionView/useThreadTextCap'
import { editorMeasure } from '@/lib/editor/measure'
import { EditorTab, type EditorTabProps } from './EditorTab'

afterEach(() => {
  cleanup()
  window.localStorage.clear()
  act(() => useSettingsStore.getState().setThreadTextWidth(THREAD_TEXT_WIDTH_DEFAULT))
})

function mount(p: Partial<EditorTabProps> = {}): ReturnType<typeof render> {
  return render(
    <EditorTab filePath="/ws/README.md" kind="markdown" text={'# Notes\n\nOne two three.\n'} dirty={false} saving={false} readOnly={null} cursor={{ line: 1, col: 1 }} width={1440} renderEditor={() => <div />} {...p} />,
  )
}
const measure = (): string => screen.getByTestId('editor-tab').style.getPropertyValue('--ed-measure')
const src = (rel: string): string => readFileSync(join(__dirname, '..', rel), 'utf8')

describe('the markdown editor reads at the Thread’s text width', () => {
  it('by default: the Thread’s default column, from the same setting', () => {
    const { result } = renderHook(() => useThreadTextCapPx())
    expect(result.current).toBe(THREAD_TEXT_WIDTH_DEFAULT_PX)
    mount()
    expect(measure()).toBe(`${THREAD_TEXT_WIDTH_DEFAULT_PX}px`)
  })

  it('changing "Thread text width" moves both', () => {
    const { result } = renderHook(() => useThreadTextCapPx())
    mount()
    act(() => useSettingsStore.getState().setThreadTextWidth({ limited: true, px: 640 }))
    expect(result.current).toBe(640)
    expect(measure()).toBe('640px')
    act(() => useSettingsStore.getState().setThreadTextWidth({ limited: false, px: 640 }))
    expect(result.current).toBeNull()
    expect(measure()).toBe('100%')
  })

  it('Full width is the wider of the Thread’s width and 100 characters', () => {
    window.localStorage.setItem('k2.editor.fullWidth.v1', '1')
    mount()
    expect(measure()).toBe(`max(${THREAD_TEXT_WIDTH_DEFAULT_PX}px, 100ch)`)
    expect(editorMeasure(null, true)).toBe('100%')
  })

  it('code files take no measure', () => {
    mount({ kind: 'code', filePath: '/ws/app.ts', text: 'export const a = 1\n' })
    expect(measure()).toBe('')
  })

  it('the Thread and the editor read the one hook; the editor CSS holds no width of its own', () => {
    for (const f of ['AgentPane/AgentChatPane.tsx', 'Projects/DashboardAgentTile.tsx', 'SessionView/SessionColumns.tsx', 'Editor/MarkdownEditor.tsx']) {
      expect(src(f)).toContain('useThreadTextCapPx()')
      expect(src(f)).not.toContain('threadTextCapPx(s.threadTextWidth)')
    }
    expect(src('Editor/editorCss.ts')).not.toMatch(/--ed-measure:\s*\d/)
  })
})
