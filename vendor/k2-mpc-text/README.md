# k2-mpc-text

The plain-text type pack for the K2 multiplayer kit
([k2-mpc-base](https://github.com/Alakazam-211/k2-mpc-base)). People and agents edit the same
text file live, with presence and attribution, and the file on disk stays exactly what it was
byte for byte, except where someone edited it.

It is one pack for every plain-text type: source code (over 200 extensions), configuration
and data as text (JSON, YAML, TOML, INI, dotfiles), markup (HTML, XML and SVG source),
diagram sources (Mermaid, PlantUML, Graphviz, draw.io XML), and, until a grid pack exists,
CSV and TSV tables as text (the `csv-as-text` family, below).

- **Schema:** one text root (`source`) over the file's text. Every language, comment and blank
  line survives, and the kit's line tools (`grep`, `sed`, `patch`, ...) map directly onto it.
  All types share one schema (`text`), so a doc keeps its history across a rename from one
  text type to another.
- **File-type table:** data in `src/types.rs`. Each extension, well-known file name
  (`Makefile`, `Dockerfile`, `.gitignore`) or name prefix (`Dockerfile.dev`, `.env.local`)
  maps to a family and codec settings. `TextPack::for_path(path)` gives the pack for a file.
- **Byte fidelity:** the codec restores the BOM, the line endings (LF, CRLF, or CR) and the
  final newline (or its absence), and never normalises anything. A file it can't
  round-trip is **refused with a typed reason, never mangled**. A fidelity corpus of 53 files
  (46 accepted across the six families, 7 refused) runs through doc, store and live sessions
  on disk, plus a random-edit property test.
- **Agent parity:** `docs/PARITY.md` maps every person action to a command-line verb.
  `tests/agent_parity.rs` runs each through the kit's parity harness. The kit's text verbs
  cover every row, so this pack adds no verbs.
- **Host integration:** `docs/HOST-INTEGRATION.md` explains how a host opens files with this
  pack, its family names, the refusals to show, and the size caps.

Check this repo out next to `k2-mpc-base` (sibling directories). `scripts/check.sh` runs
format, lints, tests, the licence gate and the kit's engine-free check.

Status: early. Licence: not yet chosen.

## How a file is decided

`lookup(path)` looks only at the file name. It tries these in order:

1. an exact name (ignoring ASCII case);
2. a name prefix;
3. the last extension (ignoring case; `a.d.ts` is `.ts`).

If the extension is on the not-text list, the file is not for this pack. If the extension is
in the table, the file gets that group's type. Anything else is **unknown**: it is opened as
plain text if it passes the strictest guard. That means UTF-8, no NUL, no control bytes
other than tab, LF, VT, FF, CR and ESC, one line-ending style, and at most 4 MiB.

These settings are the same for every type, and the pack has no switch for them:

- **BOM:** preserved.
- **Line endings:** preserved, one style per file (LF, CRLF or CR). A mix is refused.
- **Final newline:** preserved.
- **Encoding:** UTF-8 only. UTF-16 and UTF-32 (by BOM), and any invalid UTF-8, are refused.
- **NUL:** refused (binary).
- **Normalisation:** none (no Unicode, whitespace, tab or newline fixes).

These settings vary by type:

- **Cap:** the size limit.
- **New line breaks:** the style used when a file has no line break yet (an empty or
  one-line file).
- **Control bytes:** known types accept every control byte except NUL. Unknown types refuse
  them.

## CSV and TSV as text (`csv-as-text`)

`.csv` and `.tsv` open here as text, in their own family, `csv-as-text`, so that tables can
be multiplayer before a grid pack exists. When one ships, a host asks it first and these two
extensions move there; the family name says which mode a doc was opened in.

A table here is its text. A row is a line, a cell is the characters between two delimiters,
and the pack doesn't parse either. That gives:

- **What works:** concurrent edits to different rows merge, and so do edits to different
  cells of one row and two appended rows. People merge as live text; agents merge through
  `write --base` (by character, with no last-writer-wins). The tests show two agents
  editing two rows of the same version at the same moment, and both edits land.
- **Fidelity, as for every type:** byte-exact. CRLF (the RFC 4180 and Excel style) and CR
  (old Mac Excel) are kept, as are the BOM of an Excel "CSV UTF-8" export, the delimiter
  (comma, semicolon, tab), the quoting and the final newline. A quoted field with a line
  break inside is just text.
- **Mixed line endings are refused,** as for every type. Excel on Windows writes a line
  break inside a cell as a bare LF in a file whose rows end in CRLF; such a file opens the
  non-live way, with `mixed_line_endings` and the counts.

What text mode does **not** do (the grid pack's job):

- **No row identity.** Moving a row, or sorting, is a delete and an insert. An edit
  another agent made to that row from the same base doesn't follow it: it conflicts and
  is re-done on the new text. For people typing live, text typed into a row while someone
  else moves it is expected to stay behind where the row was, as a fragment (standard text-CRDT
  behaviour; not yet covered by a test here). A sort rewrites most lines, so
  it conflicts with nearly any concurrent edit.
- **No column identity.** Adding, removing or moving a column rewrites every line. An
  agent's cell edit from the same base then conflicts in any rewritten row (the tests show
  this), and is re-done on the new text. The merge can't carry it to the column's new
  place.
- **No table checks.** Nothing keeps rows the same width or quotes balanced. A cell edit
  that types a delimiter or a quote changes the table's shape, and the file is written
  as typed. There are no types, formulas, or a header row.
- **No dialect knowledge.** Delimiter, quoting and encoding are whatever the bytes are.
  The size cap is 4 MiB, as for other text.

Other delimited names (`.psv`, `.tab`, `.dsv`) aren't in the table. They open as unknown
text under the strict guard, so the set a grid pack takes over is exactly `.csv` and `.tsv`.

## The table

| Group | Family | Cap | New line breaks | Extensions | Names and prefixes |
|---|---|---|---|---|---|
| text | text | 4 MiB | LF | `.txt` `.text` `.srt` `.vtt` | `README` `LICENSE` `LICENCE` `COPYING` `NOTICE` `AUTHORS` `CONTRIBUTORS` `CHANGELOG` `CHANGES` `HISTORY` `TODO` `INSTALL` `VERSION` `MAINTAINERS` |
| logs | text | 1 MiB | LF | `.log` | — |
| code | code | 4 MiB | LF | `.c` `.h` `.cc` `.cpp` `.cxx` `.c++` `.hpp` `.hh` `.hxx` `.h++` `.ino` `.m` `.mm` `.rs` `.go` `.zig` `.nim` `.d` `.swift` `.cu` `.cuh` `.java` `.kt` `.kts` `.scala` `.sc` `.groovy` `.gradle` `.clj` `.cljs` `.cljc` `.edn` `.cs` `.csx` `.fs` `.fsi` `.fsx` `.vb` `.py` `.pyi` `.pyw` `.rb` `.rake` `.gemspec` `.pl` `.pm` `.php` `.lua` `.r` `.jl` `.tcl` `.awk` `.dart` `.js` `.mjs` `.cjs` `.jsx` `.ts` `.mts` `.cts` `.tsx` `.vue` `.svelte` `.astro` `.coffee` `.sh` `.bash` `.zsh` `.fish` `.ksh` `.csh` `.tcsh` `.ps1` `.psm1` `.psd1` `.hs` `.lhs` `.elm` `.ml` `.mli` `.ex` `.exs` `.erl` `.hrl` `.purs` `.rkt` `.scm` `.ss` `.lisp` `.el` `.css` `.scss` `.sass` `.less` `.styl` `.hbs` `.handlebars` `.mustache` `.jinja` `.jinja2` `.j2` `.ejs` `.erb` `.liquid` `.twig` `.njk` `.pug` `.haml` `.sql` `.graphql` `.gql` `.proto` `.thrift` `.prisma` `.cmake` `.mk` `.mak` `.make` `.bazel` `.bzl` `.nix` `.tf` `.tfvars` `.hcl` `.dockerfile` `.containerfile` `.v` `.sv` `.svh` `.vhd` `.vhdl` `.asm` `.s` `.wat` `.glsl` `.vert` `.frag` `.hlsl` `.wgsl` `.metal` `.f` `.f90` `.f95` `.for` `.pas` `.ada` `.adb` `.ads` `.cob` `.cbl` `.sol` `.vim` `.diff` `.patch` | `Makefile` `GNUmakefile` `Dockerfile` `Containerfile` `Jenkinsfile` `Vagrantfile` `Gemfile` `Rakefile` `Podfile` `Brewfile` `Fastfile` `Procfile` `Justfile` `Tiltfile` `Snakefile` `Earthfile` `BUILD` `WORKSPACE` `PKGBUILD` `.bashrc` `.bash_profile` `.bash_logout` `.zshrc` `.zprofile` `.zshenv` `.profile` `.kshrc` `.vimrc` `.gvimrc` `.envrc` `Dockerfile.*` `Containerfile.*` `Makefile.*` |
| code-windows | code | 4 MiB | CRLF | `.bat` `.cmd` | — |
| config | config | 4 MiB | LF | `.json` `.jsonc` `.json5` `.jsonl` `.ndjson` `.geojson` `.webmanifest` `.yaml` `.yml` `.toml` `.ini` `.cfg` `.conf` `.cnf` `.properties` `.env` `.lock` `.plist` | `.gitignore` `.gitattributes` `.gitmodules` `.gitconfig` `.mailmap` `.editorconfig` `.npmrc` `.yarnrc` `.nvmrc` `.node-version` `.python-version` `.ruby-version` `.tool-versions` `.prettierrc` `.eslintrc` `.babelrc` `.stylelintrc` `.dockerignore` `.npmignore` `.eslintignore` `.prettierignore` `.hgignore` `.env` `.flake8` `.pylintrc` `.clang-format` `.clang-tidy` `.htaccess` `.curlrc` `.wgetrc` `.inputrc` `CODEOWNERS` `Caddyfile` `go.mod` `go.sum` `go.work` `Pipfile` `.env.*` |
| markup | markup | 4 MiB | LF | `.html` `.htm` `.xhtml` `.xml` `.xsl` `.xslt` `.xsd` `.dtd` `.svg` `.rss` `.atom` `.wsdl` `.xaml` `.csproj` `.fsproj` `.vbproj` `.props` `.targets` `.resx` `.tex` `.latex` `.sty` `.cls` `.bib` `.rst` `.adoc` `.asciidoc` `.org` `.textile` `.pod` | — |
| diagram | diagram | 4 MiB | LF | `.mermaid` `.mmd` `.puml` `.plantuml` `.iuml` `.dot` `.gv` `.drawio` `.dio` | — |
| csv | csv-as-text | 4 MiB | LF | `.csv` `.tsv` | — |

**Unknown names:** `text` family, 4 MiB, LF, strict control-byte guard.

**Not text (opened some other way, e.g. tier 0):**
`.png` `.jpg` `.jpeg` `.gif` `.webp` `.bmp` `.ico` `.icns` `.heic` `.heif` `.avif` `.tif` `.tiff` `.psd` `.svgz` `.mp3` `.wav` `.ogg` `.flac` `.m4a` `.aac` `.opus` `.mp4` `.webm` `.mov` `.mkv` `.avi` `.m4v` `.pdf` `.doc` `.docx` `.xls` `.xlsx` `.ppt` `.pptx` `.odt` `.ods` `.odp` `.key` `.pages` `.numbers` `.epub` `.zip` `.gz` `.tgz` `.bz2` `.xz` `.zst` `.7z` `.rar` `.tar` `.jar` `.war` `.apk` `.ipa` `.dmg` `.iso` `.wasm` `.bin` `.exe` `.dll` `.so` `.dylib` `.class` `.o` `.a` `.lib` `.pyc` `.pyo` `.pdb` `.woff` `.woff2` `.ttf` `.otf` `.eot` `.sqlite` `.sqlite3` `.db` `.glb` `.3mf` `.blend`

Files another pack serves (`.md` and `.markdown` for Markdown; later `.ipynb`, ...) aren't
listed here: a host asks the more specific pack first, and this pack is the fallback. `.csv`
and `.tsv` are listed until a grid pack serves them.

To add a type, add its extension or name to a group, or add a group with other settings. Then
add it to this table. The tests fail if the table and this README disagree, or if a key is
listed twice.
