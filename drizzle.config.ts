import { defineConfig } from 'drizzle-kit'

// SQL migration output lands alongside the Rust code that reads them
// via `include_str!` at build time (crates/k2so-core/src/db/mod.rs).
// Pre-0.33.0 there were two parallel copies (this `out:` dir at the
// repo root + src-tauri/drizzle_sql/) that had to be hand-synced.
// Unified here so drizzle-kit's generator is the sole writer.
export default defineConfig({
  schema: './src/main/lib/db/schema.ts',
  out: './crates/k2so-core/drizzle_sql',
  dialect: 'sqlite'
})
