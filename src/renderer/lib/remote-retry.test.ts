// Unit tests for the shared retry-on-network-error helper.
//
// This is the single source of truth for (a) classifying a connection-level
// error and (b) the backoff schedule that lets a remote daemon restart/update
// self-heal in WKWebView (the throw evicts the dead pooled socket; a retry
// opens a fresh one). The matrix below locks BOTH halves: only true network
// failures retry; a non-2xx app error / 401 surfaces immediately.

import { describe, it, expect, vi, afterEach } from 'vitest'
import {
  isConnectionLevelError,
  withRemoteRetry,
  DEFAULT_REMOTE_RETRY_DELAYS_MS,
} from './remote-retry'

describe('isConnectionLevelError', () => {
  it('matches WKWebView / browser / kernel network-failure shapes', () => {
    for (const msg of [
      'Load failed', // WKWebView (Safari)
      'TypeError: Failed to fetch', // Chromium
      'NetworkError when attempting to fetch resource', // Firefox
      'connect ECONNREFUSED 127.0.0.1:47800',
      'connection refused',
      'daemon_ws_url invoke failed: not ready',
      'daemon not reachable',
    ]) {
      expect(isConnectionLevelError(new Error(msg))).toBe(true)
    }
  })

  it('is case-insensitive', () => {
    expect(isConnectionLevelError(new Error('LOAD FAILED'))).toBe(true)
    expect(isConnectionLevelError(new Error('FAILED TO FETCH'))).toBe(true)
  })

  it('does NOT match application (non-2xx) errors — those are authoritative', () => {
    for (const msg of [
      'bad request',
      'daemon /cli/projects/list 500',
      'session expired', // 401-shape app message → must NOT retry
      'unauthorized',
      'not found',
      'You don’t have permission to restart this server.',
    ]) {
      expect(isConnectionLevelError(new Error(msg))).toBe(false)
    }
  })

  it('returns false for non-Error throwables', () => {
    expect(isConnectionLevelError('Load failed')).toBe(false)
    expect(isConnectionLevelError(null)).toBe(false)
    expect(isConnectionLevelError(undefined)).toBe(false)
    expect(isConnectionLevelError({ message: 'Load failed' })).toBe(false)
  })
})

describe('withRemoteRetry', () => {
  afterEach(() => {
    vi.useRealTimers()
  })

  it('resolves on the first try without retrying', async () => {
    const op = vi.fn(async () => 'ok')
    await expect(withRemoteRetry(op)).resolves.toBe('ok')
    expect(op).toHaveBeenCalledTimes(1)
  })

  it('retries on a connection error then resolves on a later attempt', async () => {
    vi.useFakeTimers()
    const op = vi
      .fn()
      .mockRejectedValueOnce(new Error('Load failed'))
      .mockRejectedValueOnce(new Error('Load failed'))
      .mockResolvedValueOnce('recovered')

    const p = withRemoteRetry(op)
    // First retry is immediate (delay 0); second waits 400ms.
    await vi.runAllTimersAsync()
    await expect(p).resolves.toBe('recovered')
    expect(op).toHaveBeenCalledTimes(3)
  })

  it('rethrows IMMEDIATELY on a non-connection error (no retry)', async () => {
    const op = vi.fn(async () => {
      throw new Error('session expired') // 401-shape → authoritative
    })
    await expect(withRemoteRetry(op)).rejects.toThrow('session expired')
    expect(op).toHaveBeenCalledTimes(1)
  })

  it('gives up after the max attempts and rethrows the last error', async () => {
    vi.useFakeTimers()
    const op = vi.fn().mockRejectedValue(new Error('Failed to fetch'))
    const p = withRemoteRetry(op)
    // Attach a rejection handler before draining timers so the rejection is
    // never momentarily unhandled.
    const assertion = expect(p).rejects.toThrow('Failed to fetch')
    await vi.runAllTimersAsync()
    await assertion
    // 1 initial try + DEFAULT_REMOTE_RETRY_DELAYS_MS.length retries.
    expect(op).toHaveBeenCalledTimes(DEFAULT_REMOTE_RETRY_DELAYS_MS.length + 1)
  })

  it('calls onRetry between attempts (not before the first, not after success)', async () => {
    vi.useFakeTimers()
    const onRetry = vi.fn()
    const op = vi
      .fn()
      .mockRejectedValueOnce(new Error('Load failed'))
      .mockResolvedValueOnce('ok')

    const p = withRemoteRetry(op, { onRetry })
    await vi.runAllTimersAsync()
    await expect(p).resolves.toBe('ok')
    // One failed attempt → exactly one onRetry before the (successful) retry.
    expect(onRetry).toHaveBeenCalledTimes(1)
  })

  it('honours a custom delaysMs schedule (attempts = delays.length + 1)', async () => {
    vi.useFakeTimers()
    const op = vi.fn().mockRejectedValue(new Error('Load failed'))
    const p = withRemoteRetry(op, { delaysMs: [0, 10] })
    const assertion = expect(p).rejects.toThrow('Load failed')
    await vi.runAllTimersAsync()
    await assertion
    expect(op).toHaveBeenCalledTimes(3) // 1 try + 2 retries
  })
})
