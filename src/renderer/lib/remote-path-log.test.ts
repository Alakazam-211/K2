import { describe, it, expect } from 'vitest'
import { classifyRemoteFetchError, redactRemoteUrl } from './remote-path-log'

describe('redactRemoteUrl', () => {
  it('drops token query', () => {
    expect(
      redactRemoteUrl(
        'https://rpmavs.k2.dev/cli/chat/list?project_path=/home/k2/ai/Argus&token=SECRET',
      ),
    ).toBe('rpmavs.k2.dev/cli/chat/list')
  })

  it('keeps path on a non-URL string', () => {
    expect(redactRemoteUrl('/cli/boot-status?token=SECRET')).toBe('/cli/boot-status')
  })
})

describe('classifyRemoteFetchError', () => {
  it('classifies Safari / timeout / CORS shapes', () => {
    expect(classifyRemoteFetchError(new Error('Load failed'))).toBe('load-failed')
    expect(classifyRemoteFetchError(new Error('Failed to fetch'))).toBe('load-failed')
    expect(
      classifyRemoteFetchError(
        new Error('Origin tauri://localhost is not allowed by Access-Control-Allow-Origin'),
      ),
    ).toBe('cors')
    const timeout = new Error('signal timed out')
    timeout.name = 'TimeoutError'
    expect(classifyRemoteFetchError(timeout)).toBe('timeout')
  })
})
