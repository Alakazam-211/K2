// Tests for the external-drop invariant: OS → file-tree drops on the
// LOCAL host ALWAYS copy — never move — regardless of modifier keys.
// (Data-loss bug, 2026-07-07: Finder drops used fs/move by default and
// silently relocated files.)

import { describe, it, expect } from 'vitest'
import { planLocalExternalDrop } from './external-drop'

describe('planLocalExternalDrop', () => {
  it('always resolves to the fs/copy endpoint — never fs/move', () => {
    const plan = planLocalExternalDrop(['/Users/me/Desktop/report.pdf'], '/ws/docs')
    expect(plan.endpoint).toBe('fs/copy')
    // The type already forbids 'fs/move'; assert at runtime too so a
    // future refactor loosening the type still fails here.
    expect(plan.endpoint).not.toBe('fs/move')
  })

  it('copies the dropped sources into the target folder', () => {
    const plan = planLocalExternalDrop(
      ['/Users/me/Desktop/a.txt', '/Users/me/Downloads/b.png'],
      '/ws/assets',
    )
    expect(plan.payload).toEqual({
      sources: ['/Users/me/Desktop/a.txt', '/Users/me/Downloads/b.png'],
      destination: '/ws/assets',
    })
  })

  it('registers a copy undo entry with the created destination paths', () => {
    const plan = planLocalExternalDrop(
      ['/Users/me/Desktop/a.txt', '/Users/me/Downloads/b.png'],
      '/ws/assets',
    )
    expect(plan.undo).toEqual({
      type: 'copy',
      createdPaths: ['/ws/assets/a.txt', '/ws/assets/b.png'],
    })
  })

  it('words the toast as Copied with singular/plural item counts', () => {
    expect(planLocalExternalDrop(['/tmp/x'], '/ws').toast).toBe('Copied 1 item')
    expect(planLocalExternalDrop(['/tmp/x', '/tmp/y', '/tmp/z'], '/ws').toast).toBe(
      'Copied 3 items',
    )
  })
})
