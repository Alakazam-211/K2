// @vitest-environment jsdom
import { describe, it, expect, vi, afterEach } from 'vitest'
import { render, screen, fireEvent, cleanup, within } from '@testing-library/react'
import {
  PublishedByoDetailsModal,
  PublishedServiceDetailsModal,
} from './PublishedServiceDetailsModal'
import type { PublishedService } from './urls-ports'

afterEach(() => {
  cleanup()
})

function svc(over: Partial<PublishedService> & { name: string }): PublishedService {
  return {
    cmd: 'npm start',
    cwd: '/proj',
    port: 3000,
    expose: 'tunnel',
    desired: 'running',
    status: 'running',
    pid: 42,
    url: 'https://web.rosson.k2.dev',
    target: 'localhost:3000',
    error: null,
    lastExitCode: null,
    kind: 'cmd',
    skinRoot: '',
    ...over,
  }
}

describe('PublishedServiceDetailsModal', () => {
  it('is view-only: no Start/Stop, Esc and backdrop close', () => {
    const onClose = vi.fn()
    render(
      <PublishedServiceDetailsModal service={svc({ name: 'web' })} hostLabel="This Mac" onClose={onClose} />,
    )
    const dialog = screen.getByRole('dialog')
    expect(within(dialog).queryByRole('button', { name: 'Start' })).toBeNull()
    expect(within(dialog).queryByRole('button', { name: 'Stop' })).toBeNull()
    fireEvent.keyDown(window, { key: 'Escape' })
    expect(onClose).toHaveBeenCalled()
  })

  it('shows status, desired, pid, and This Mac for a local cmd row', () => {
    render(
      <PublishedServiceDetailsModal service={svc({ name: 'web' })} hostLabel="This Mac" onClose={() => {}} />,
    )
    expect(screen.getByText('running · healthy')).toBeTruthy()
    expect(screen.getByText('42')).toBeTruthy()
    expect(screen.getByText('npm start')).toBeTruthy()
    expect(screen.getByText('This Mac')).toBeTruthy()
    expect(screen.getByText('site')).toBeTruthy()
  })

  it('skin Launch is the official gateway, never the (skin) sentinel; empty root is bundled', () => {
    render(
      <PublishedServiceDetailsModal
        service={svc({ name: 'agents', kind: 'skin', cmd: '(skin)', skinRoot: '' })}
        hostLabel="This Mac"
        onClose={() => {}}
      />,
    )
    expect(screen.getByText('Official skin gateway')).toBeTruthy()
    expect(screen.queryByText('(skin)')).toBeNull()
    expect(screen.getByText('bundled')).toBeTruthy()
    expect(screen.getByText('Skin')).toBeTruthy()
  })

  it('non-empty skinRoot is shown as the workspace-relative dir', () => {
    render(
      <PublishedServiceDetailsModal
        service={svc({ name: 'agents', kind: 'skin', cmd: '(skin)', skinRoot: 'ui' })}
        hostLabel="Hetzner box"
        onClose={() => {}}
      />,
    )
    expect(screen.getByText('ui')).toBeTruthy()
    expect(screen.queryByText('bundled')).toBeNull()
    expect(screen.getByText('Hetzner box')).toBeTruthy()
    expect(screen.queryByText('This Mac')).toBeNull()
  })

  it('follows the live service prop — Stop then refresh shows not running (P14)', () => {
    const running = svc({ name: 'web', pid: 99, status: 'running' })
    const { rerender } = render(
      <PublishedServiceDetailsModal service={running} hostLabel="This Mac" onClose={() => {}} />,
    )
    expect(screen.getByText('99')).toBeTruthy()
    rerender(
      <PublishedServiceDetailsModal
        service={svc({ name: 'web', pid: null, status: 'stopped', desired: 'stopped' })}
        hostLabel="This Mac"
        onClose={() => {}}
      />,
    )
    expect(screen.getByText('not running')).toBeTruthy()
    expect(screen.queryByText('99')).toBeNull()
  })

  it('unhealthy is not healthy (no HTTP probe)', () => {
    render(
      <PublishedServiceDetailsModal
        service={svc({ name: 'web', status: 'unhealthy', pid: 7 })}
        hostLabel="This Mac"
        onClose={() => {}}
      />,
    )
    expect(screen.getByText('unhealthy')).toBeTruthy()
    expect(screen.queryByText('running · healthy')).toBeNull()
  })

  it('BYO leftover modal has target and URL, no PID, no Start/Stop', () => {
    render(
      <PublishedByoDetailsModal
        leftover={{
          label: 'staging',
          url: 'https://staging.rosson.k2.dev',
          target: 'localhost:4000',
        }}
        hostLabel="This Mac"
        onClose={() => {}}
      />,
    )
    const dialog = screen.getByRole('dialog')
    expect(within(dialog).getByText('staging')).toBeTruthy()
    expect(within(dialog).getByText('https://staging.rosson.k2.dev')).toBeTruthy()
    expect(within(dialog).getByText('localhost:4000')).toBeTruthy()
    expect(within(dialog).queryByText(/PID/)).toBeNull()
    expect(within(dialog).queryByRole('button', { name: 'Start' })).toBeNull()
    expect(within(dialog).queryByRole('button', { name: 'Stop' })).toBeNull()
  })
})
