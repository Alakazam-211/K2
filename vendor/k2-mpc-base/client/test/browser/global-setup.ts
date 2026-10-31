// Bundle the page the browser tests load: the editor, CodeMirror and the fake server, as one
// script (test/browser/.build/harness.js).

import { build } from "esbuild";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));

export const HARNESS = join(here, ".build", "harness.js");

export default async function globalSetup(): Promise<void> {
  await build({
    entryPoints: [join(here, "harness.ts")],
    outfile: HARNESS,
    bundle: true,
    format: "iife",
    target: "es2020",
    sourcemap: "inline",
    logLevel: "warning",
  });
}
