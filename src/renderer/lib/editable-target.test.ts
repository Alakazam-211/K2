// @vitest-environment jsdom

import { afterEach, describe, expect, it } from 'vitest'
import { isEditableTarget } from './editable-target'

afterEach(() => {
  document.body.innerHTML = ''
})

function el(html: string): HTMLElement {
  document.body.innerHTML = html
  const found = document.body.querySelector<HTMLElement>('[data-t]')
  if (!found) throw new Error(`fixture has no [data-t]: ${html}`)
  return found
}

describe('isEditableTarget', () => {
  it('is true for text fields, text boxes and selects', () => {
    expect(isEditableTarget(el('<input data-t type="email">'))).toBe(true)
    expect(isEditableTarget(el('<input data-t type="password">'))).toBe(true)
    expect(isEditableTarget(el('<input data-t>'))).toBe(true)
    expect(isEditableTarget(el('<textarea data-t></textarea>'))).toBe(true)
    expect(isEditableTarget(el('<select data-t><option>a</option></select>'))).toBe(true)
  })

  it('is true inside a contenteditable, at any depth', () => {
    expect(isEditableTarget(el('<div data-t contenteditable="true"></div>'))).toBe(true)
    expect(isEditableTarget(el('<div data-t contenteditable=""></div>'))).toBe(true)
    expect(isEditableTarget(el('<div data-t contenteditable="plaintext-only"></div>'))).toBe(true)
    expect(isEditableTarget(el('<div contenteditable="true"><p><span data-t>x</span></p></div>'))).toBe(true)
  })

  it('is false for everything else', () => {
    expect(isEditableTarget(el('<button data-t>Go</button>'))).toBe(false)
    expect(isEditableTarget(el('<div data-t tabindex="0"></div>'))).toBe(false)
    expect(isEditableTarget(el('<div data-t contenteditable="false"></div>'))).toBe(false)
    expect(isEditableTarget(document.body)).toBe(false)
    expect(isEditableTarget(document)).toBe(false)
    expect(isEditableTarget(window)).toBe(false)
    expect(isEditableTarget(null)).toBe(false)
    expect(isEditableTarget(undefined)).toBe(false)
  })
})
