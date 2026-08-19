// @vitest-environment jsdom
import { describe, it, expect } from 'vitest'
import { createElement, type ReactNode } from 'react'
import { createRoot } from 'react-dom/client'
import { act } from 'react'
import {
  PageLiveContext,
  TabVisibilityContext,
  useIsTabVisible,
} from './TabVisibilityContext'

function readVisible(wrapper?: (node: ReactNode) => ReactNode): boolean {
  let value = false
  function Probe(): null {
    value = useIsTabVisible()
    return null
  }
  const el = document.createElement('div')
  const root = createRoot(el)
  const tree = createElement(Probe)
  act(() => {
    root.render(wrapper ? wrapper(tree) : tree)
  })
  act(() => {
    root.unmount()
  })
  return value
}

describe('useIsTabVisible', () => {
  it('is true when both tab and page are live (defaults)', () => {
    expect(readVisible()).toBe(true)
  })

  it('is false when the Agents page is covered even if the tab is selected', () => {
    expect(
      readVisible((node) =>
        createElement(
          PageLiveContext.Provider,
          { value: false },
          createElement(TabVisibilityContext.Provider, { value: true }, node),
        ),
      ),
    ).toBe(false)
  })

  it('stays true on a Projects overlay that re-enables page live', () => {
    expect(
      readVisible((node) =>
        createElement(
          PageLiveContext.Provider,
          { value: false },
          createElement(
            PageLiveContext.Provider,
            { value: true },
            createElement(TabVisibilityContext.Provider, { value: true }, node),
          ),
        ),
      ),
    ).toBe(true)
  })
})

