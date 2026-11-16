// The stills a picture source keeps, least recently used first out. A
// 1920 px still is about 15 MB (pixels plus the id image), and the shots'
// long pages crashed their renderer holding 48 of them, so the cap is
// small: enough for the views and revs a person steps between. An
// evicted still's bitmap is closed, so its memory goes at once; pixel
// arrays go with their last reference.

import type { Still } from './picture-source'

/** How many stills a picture source keeps. */
export const STILL_CACHE_MAX = 8

/** Free what a still holds that the collector won't free promptly (a decoded bitmap). */
export function releaseStill(st: Still): void {
  if (st.image.kind !== 'bitmap') return
  const img = st.image.image as { close?: () => void }
  if (typeof img.close === 'function') img.close()
}

export class StillCache {
  private readonly map = new Map<string, Still>()

  constructor(private readonly max = STILL_CACHE_MAX) {}

  get size(): number {
    return this.map.size
  }

  /** Oldest first. */
  keys(): string[] {
    return [...this.map.keys()]
  }

  /** The still for a key, now the most recently used. */
  get(key: string): Still | undefined {
    const st = this.map.get(key)
    if (st) {
      this.map.delete(key)
      this.map.set(key, st)
    }
    return st
  }

  set(key: string, st: Still): void {
    const old = this.map.get(key)
    if (old && old !== st) releaseStill(old)
    this.map.delete(key)
    this.map.set(key, st)
    while (this.map.size > this.max) {
      const [k, v] = this.map.entries().next().value as [string, Still]
      this.map.delete(k)
      releaseStill(v)
    }
  }

  clear(): void {
    for (const st of this.map.values()) releaseStill(st)
    this.map.clear()
  }
}
