// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { ConfirmSheet, PasswordResetDialog } from './AccessDialogs'

afterEach(cleanup)

describe('Shared dialogs pin their buttons under the body', () => {
  it('a confirm keeps Cancel and the action in the footer, Cancel first with the focus', () => {
    const onConfirm = vi.fn()
    render(
      <ConfirmSheet title="Remove alex?" message="They can no longer sign in." confirmLabel="Remove" destructive onConfirm={onConfirm} onCancel={() => {}} testId="confirm" />,
    )
    const dialog = screen.getByTestId('confirm')
    const body = within(dialog).getByTestId('dialog-body')
    const foot = within(dialog).getByTestId('dialog-foot')
    expect(body.textContent).toContain('They can no longer sign in.')
    expect(Array.from(foot.querySelectorAll('button')).map((b) => b.textContent)).toEqual(['Cancel', 'Remove'])
    expect(document.activeElement?.textContent).toBe('Cancel')
    fireEvent.click(within(foot).getByRole('button', { name: 'Remove' }))
    expect(onConfirm).toHaveBeenCalledTimes(1)
  })

  it("a form's submit button in the footer still submits the form, and an error shows in the body above it", async () => {
    const submit = vi.fn(async () => {
      throw new Error('users file is locked')
    })
    render(
      <PasswordResetDialog
        who="alex"
        intro="Their password stops working."
        rules="Any password."
        problem={() => null}
        makeHint="Shown once."
        signIn="They sign in with it."
        submit={submit}
        onDone={() => {}}
        onClose={() => {}}
      />,
    )
    const dialog = screen.getByTestId('reset-form-alex')
    const foot = within(dialog).getByTestId('dialog-foot')
    const button = within(foot).getByRole('button', { name: 'Reset password' }) as HTMLButtonElement
    const form = within(dialog).getByTestId('dialog-body').querySelector('form') as HTMLFormElement
    expect(button.form).toBe(form)
    // Enter in the form submits it, as before.
    fireEvent.submit(form)
    const err = await within(dialog).findByTestId('dialog-error')
    expect(submit).toHaveBeenCalledWith(null)
    expect(within(dialog).getByTestId('dialog-body').contains(err)).toBe(true)
    expect(err.textContent).toContain('The server said: users file is locked.')
    fireEvent.click(button)
    await waitFor(() => expect(submit).toHaveBeenCalledTimes(2))
  })
})
