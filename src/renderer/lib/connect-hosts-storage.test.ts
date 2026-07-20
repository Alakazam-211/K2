import { describe, it, expect, beforeEach } from 'vitest'
import {
  CONNECT_HOSTS_STORAGE_KEY,
  LEGACY_CONNECT_HOSTS_STORAGE_KEY,
  readConnectHostsStorage,
  writeConnectHostsStorage,
  clearConnectHostsStorage,
} from './connect-hosts-storage'

class MemoryStorage {
  private map = new Map<string, string>()
  getItem(k: string): string | null {
    return this.map.has(k) ? this.map.get(k)! : null
  }
  setItem(k: string, v: string): void {
    this.map.set(k, v)
  }
  removeItem(k: string): void {
    this.map.delete(k)
  }
}

describe('connect-hosts storage dual-read', () => {
  let storage: MemoryStorage

  beforeEach(() => {
    storage = new MemoryStorage()
  })

  it('canonical key is k2.connect-hosts.v1 (not k2so)', () => {
    expect(CONNECT_HOSTS_STORAGE_KEY).toBe('k2.connect-hosts.v1')
    expect(LEGACY_CONNECT_HOSTS_STORAGE_KEY).toBe('k2so.connect-hosts.v1')
  })

  it('prefers k2.* when both keys exist', () => {
    storage.setItem(LEGACY_CONNECT_HOSTS_STORAGE_KEY, '["legacy"]')
    storage.setItem(CONNECT_HOSTS_STORAGE_KEY, '["canonical"]')
    expect(readConnectHostsStorage(storage)).toBe('["canonical"]')
  })

  it('falls back to k2so.* when canonical is missing', () => {
    storage.setItem(LEGACY_CONNECT_HOSTS_STORAGE_KEY, '["legacy"]')
    expect(readConnectHostsStorage(storage)).toBe('["legacy"]')
  })

  it('returns null when neither key is set', () => {
    expect(readConnectHostsStorage(storage)).toBeNull()
  })

  it('write migrates: sets k2.* and removes k2so.*', () => {
    storage.setItem(LEGACY_CONNECT_HOSTS_STORAGE_KEY, '["legacy"]')
    writeConnectHostsStorage(storage, '["next"]')
    expect(storage.getItem(CONNECT_HOSTS_STORAGE_KEY)).toBe('["next"]')
    expect(storage.getItem(LEGACY_CONNECT_HOSTS_STORAGE_KEY)).toBeNull()
    expect(readConnectHostsStorage(storage)).toBe('["next"]')
  })

  it('clear drops both keys', () => {
    storage.setItem(CONNECT_HOSTS_STORAGE_KEY, '[]')
    storage.setItem(LEGACY_CONNECT_HOSTS_STORAGE_KEY, '[]')
    clearConnectHostsStorage(storage)
    expect(storage.getItem(CONNECT_HOSTS_STORAGE_KEY)).toBeNull()
    expect(storage.getItem(LEGACY_CONNECT_HOSTS_STORAGE_KEY)).toBeNull()
  })
})
