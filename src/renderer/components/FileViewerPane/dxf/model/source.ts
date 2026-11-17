// Where the viewer reads a drawing's model: its header, its pages of
// entities (model space, a layout, or a block definition), one file at one
// version. While a server doesn't have the route (404, 405, 501), the header
// is null and the viewer reads the DXF text itself, as it always has. Tests
// and shots use the fixture source.

import { daemonCliGet } from '@/lib/daemon-cli'
import type { ServerScope } from '@/kessel/server-scope'
import { isHeader, isPage, type ModelHeader, type ModelPage } from './types'

export interface DrawingModelSource {
  header(): Promise<ModelHeader | null>
  /** One page of a space (`model`, a layout's or a block's `@h:`), from a cursor the previous page gave. */
  page(space: string, cursor?: string | null): Promise<ModelPage | null>
  /** A block definition's entities, in the block's own coordinates. */
  blockPage(handle: string): Promise<ModelPage | null>
}

// TODO: K2 adds this route. `GET docs/cad/draw {path, what: header}` answers the
// CAD pack's `cad.drawing.header/1`; `{path, what: page, space, cursor}` answers
// `cad.drawing.page/1`. The host keeps the drawing open per version while a
// viewer pages through it.
export const DRAW_ROUTE = 'docs/cad/draw'

export function daemonDrawingSource(scope: ServerScope, path: string): DrawingModelSource {
  const get = async (q: Record<string, string>): Promise<unknown> => {
    try {
      return await daemonCliGet<unknown>(scope, DRAW_ROUTE, { path, ...q })
    } catch {
      // No route yet, or any failure: the viewer reads the file itself.
      return null
    }
  }
  const page = async (space: string, cursor?: string | null): Promise<ModelPage | null> => {
    const r = await get(cursor ? { what: 'page', space, cursor } : { what: 'page', space })
    return isPage(r) ? r : null
  }
  return {
    async header() {
      const r = await get({ what: 'header' })
      return isHeader(r) ? r : null
    },
    page,
    blockPage: (handle) => page(handle),
  }
}

/** The cursor in a page's `next` call, or null on the last page. */
export function nextCursor(p: ModelPage): string | null {
  if (!p.next) return null
  const i = p.next.indexOf('--cursor')
  return i >= 0 ? (p.next[i + 1] ?? null) : null
}

/** Canned answers by space (each a list of pages, in order), for tests and shots. */
export function fixtureDrawingSource(fx: { header: ModelHeader | null; pages: Record<string, ModelPage[]> }): DrawingModelSource & {
  calls: { header: number; page: string[]; block: string[] }
  setHeader(h: ModelHeader | null): void
} {
  let header = fx.header
  const calls = { header: 0, page: [] as string[], block: [] as string[] }
  const at = (space: string, cursor?: string | null): ModelPage | null => {
    const pages = fx.pages[space] ?? []
    if (!cursor) return pages[0] ?? null
    const i = pages.findIndex((p) => nextCursor(p) === cursor)
    return i >= 0 ? (pages[i + 1] ?? null) : null
  }
  return {
    calls,
    setHeader: (h) => void (header = h),
    async header() {
      calls.header++
      return header
    },
    async page(space, cursor) {
      calls.page.push(space)
      return at(space, cursor)
    },
    async blockPage(handle) {
      calls.block.push(handle)
      return at(handle)
    },
  }
}
