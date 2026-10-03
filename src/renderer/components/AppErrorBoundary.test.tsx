// @vitest-environment jsdom
// The crash panel names the component that threw: console.error carries the
// React component stack, and Details shows it with a Copy button.
import { afterEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { AppErrorBoundary, crashReportText } from './AppErrorBoundary'

function SpreadsAnObject(): React.JSX.Element {
  const rows = {} as unknown as string[]
  return <div>{[...rows].join(',')}</div>
}

afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
})

describe('AppErrorBoundary', () => {
  it('logs the error with the component stack and shows it under Details', async () => {
    const errors: unknown[][] = []
    vi.spyOn(console, 'error').mockImplementation((...args: unknown[]) => {
      errors.push(args)
    })
    const writeText = vi.fn(async (_text: string) => undefined)
    Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true })

    render(
      <AppErrorBoundary>
        <SpreadsAnObject />
      </AppErrorBoundary>,
    )

    expect(screen.getByText('Something went wrong rendering K2.')).toBeTruthy()
    const crash = errors.find((args) => args[0] === '[AppErrorBoundary] CRASH:')
    expect(crash).toBeTruthy()
    expect(crash![1]).toBeInstanceOf(Error)
    expect(String(crash![3])).toContain('SpreadsAnObject')

    const stack = screen.getByTestId('app-error-component-stack')
    expect(stack.closest('details')).toBeTruthy()
    expect(screen.getByText('Details').tagName).toBe('SUMMARY')
    expect(stack.textContent).toContain('SpreadsAnObject')

    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Copy details' }))
    })
    expect(writeText).toHaveBeenCalledTimes(1)
    expect(writeText.mock.calls[0][0]).toContain('SpreadsAnObject')
    await waitFor(() => expect(screen.getByRole('button', { name: 'Copied' })).toBeTruthy())
  })

  it('crashReportText joins the message, the JS stack and the component stack', () => {
    const err = new Error('boom')
    err.stack = 'Error: boom\n    at f'
    expect(crashReportText(err, '\n    at Row\n    at List')).toBe(
      'boom\nStack:\nError: boom\n    at f\nComponent stack:\n    at Row\n    at List',
    )
    const bare = new Error('bare')
    bare.stack = undefined
    expect(crashReportText(bare, null)).toBe('bare')
  })
})
