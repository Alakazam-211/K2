// Heading slugs as GitHub makes its anchors (github-slugger's rules), for the
// outline and a heading's address `@h:<slug>` (build-editors.md E3). Pure.
//
// K2 Design's E3 wrote this because the kit's own heading slugs (being added
// under lib/editor/live/) hadn't landed on the queue yet. When they do, one
// of the two should go; both follow the same rules:
//   - lower case;
//   - drop every character that isn't a letter, a mark, a number, a connector
//     (`_`), a hyphen or a space (emoji and punctuation go);
//   - each space becomes a hyphen (runs are kept, as GitHub keeps them);
//   - a repeat gets `-1`, `-2`… after it, skipping any slug already taken.

const DROP = /[^\p{L}\p{M}\p{N}\p{Pc} -]/gu

export function githubSlug(text: string): string {
  return text.toLowerCase().replace(DROP, '').replace(/ /g, '-')
}

/** Slugs for one document's headings in order, numbering repeats as GitHub does. */
export function headingSlugger(): { slug(text: string): string } {
  const seen = new Map<string, number>()
  return {
    slug(text: string): string {
      const base = githubSlug(text)
      let slug = base
      while (seen.has(slug)) {
        const n = (seen.get(base) ?? 0) + 1
        seen.set(base, n)
        slug = `${base}-${n}`
      }
      seen.set(slug, 0)
      return slug
    },
  }
}

/** A heading's address in a document: `@h:<slug>`. */
export const slugAddress = (slug: string): string => `@h:${slug}`
