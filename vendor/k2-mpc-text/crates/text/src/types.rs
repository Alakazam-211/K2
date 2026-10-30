//! The file-type table: which files this pack makes live, as what, with which settings.
//!
//! Data, not code: [`TABLE`] is a list of [`Group`]s (a family, codec settings, and the
//! extensions, exact file names and name prefixes that belong to it). Add a type by adding
//! its extension to a group, or a group with other settings. The tests check the table:
//! every key is lowercase, unique across groups and absent from [`NOT_TEXT`], and the
//! README lists every one.
//!
//! [`lookup`] decides a path, in this order:
//! 1. **An exact file name** (`Makefile`, `Dockerfile`, `.gitignore`, `go.mod`), ignoring
//!    ASCII case.
//! 2. **A name prefix** (`Dockerfile.dev`, `.env.local`).
//! 3. **The last extension** (`a.d.ts` is `ts`), ignoring ASCII case: in [`NOT_TEXT`] the
//!    file is not for this pack; in the table it is that group's type.
//! 4. **Anything else is [`Lookup::Unknown`]:** text if it passes the strictest guard
//!    ([`UNKNOWN`]: UTF-8, no NUL or other control bytes, one line-ending style, 4 MiB).

use crate::codec::{Controls, Eol, Settings};

/// What kind of text a file is: for the host's editor mode and preview, not for the codec.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Family {
    /// Prose and logs (`.txt`, `.log`, `README`).
    Text,
    /// Source code, scripts, build files and stylesheets.
    Code,
    /// Configuration and data as text (JSON, YAML, TOML, INI, dotfiles).
    Config,
    /// Tag and document markup (HTML, XML, SVG; reStructuredText, AsciiDoc, TeX).
    Markup,
    /// Diagram sources (Mermaid, PlantUML, Graphviz, draw.io XML).
    Diagram,
    /// Delimited tables as text (`.csv`, `.tsv`): `csv-as-text`. An early multiplayer path
    /// for tables until a grid pack exists; a host that has one routes these files there
    /// first. Rows are lines and cells are characters: concurrent edits to different rows
    /// (or cells) merge, but a column move or a sort is a text rewrite (see the README).
    CsvAsText,
}

impl Family {
    pub const ALL: &'static [Family] = &[
        Family::Text,
        Family::Code,
        Family::Config,
        Family::Markup,
        Family::Diagram,
        Family::CsvAsText,
    ];

    /// Stable name: `text`, `code`, `config`, `markup`, `diagram`, `csv-as-text`.
    pub fn name(self) -> &'static str {
        match self {
            Family::Text => "text",
            Family::Code => "code",
            Family::Config => "config",
            Family::Markup => "markup",
            Family::Diagram => "diagram",
            Family::CsvAsText => "csv-as-text",
        }
    }
}

/// A file type: its family and its codec settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FileType {
    pub family: Family,
    pub settings: Settings,
}

/// One row of the table: a type and the files that have it.
#[derive(Debug)]
pub struct Group {
    /// Short name, for docs and logs (`code`, `code-windows`, `logs`).
    pub label: &'static str,
    pub ty: FileType,
    /// Lowercase, without the dot.
    pub extensions: &'static [&'static str],
    /// Exact file names (no folder), matched ignoring ASCII case.
    pub names: &'static [&'static str],
    /// File-name prefixes (`Dockerfile.` matches `Dockerfile.dev`), ignoring ASCII case.
    pub prefixes: &'static [&'static str],
}

/// The default size cap: 4 MiB, as the Markdown pack.
pub const MAX_BYTES: usize = 4 << 20;

const fn settings(max_bytes: usize, new_eol: Eol) -> Settings {
    Settings {
        max_bytes,
        new_eol,
        controls: Controls::Allow,
    }
}

const fn ty(family: Family, max_bytes: usize, new_eol: Eol) -> FileType {
    FileType {
        family,
        settings: settings(max_bytes, new_eol),
    }
}

/// A file whose type the table doesn't know: plain text, under the strict guard.
pub const UNKNOWN: FileType = FileType {
    family: Family::Text,
    settings: Settings {
        max_bytes: MAX_BYTES,
        new_eol: Eol::Lf,
        controls: Controls::Refuse,
    },
};

/// The table. See the module docs for how a path is looked up.
pub const TABLE: &[Group] = &[
    Group {
        label: "text",
        ty: ty(Family::Text, MAX_BYTES, Eol::Lf),
        extensions: &["txt", "text", "srt", "vtt"],
        names: &[
            "README",
            "LICENSE",
            "LICENCE",
            "COPYING",
            "NOTICE",
            "AUTHORS",
            "CONTRIBUTORS",
            "CHANGELOG",
            "CHANGES",
            "HISTORY",
            "TODO",
            "INSTALL",
            "VERSION",
            "MAINTAINERS",
        ],
        prefixes: &[],
    },
    Group {
        // logs are append streams written by programs: a smaller cap
        label: "logs",
        ty: ty(Family::Text, 1 << 20, Eol::Lf),
        extensions: &["log"],
        names: &[],
        prefixes: &[],
    },
    Group {
        label: "code",
        ty: ty(Family::Code, MAX_BYTES, Eol::Lf),
        extensions: &[
            // C family and systems
            "c",
            "h",
            "cc",
            "cpp",
            "cxx",
            "c++",
            "hpp",
            "hh",
            "hxx",
            "h++",
            "ino",
            "m",
            "mm",
            "rs",
            "go",
            "zig",
            "nim",
            "d",
            "swift",
            "cu",
            "cuh",
            // JVM and .NET
            "java",
            "kt",
            "kts",
            "scala",
            "sc",
            "groovy",
            "gradle",
            "clj",
            "cljs",
            "cljc",
            "edn",
            "cs",
            "csx",
            "fs",
            "fsi",
            "fsx",
            "vb",
            // scripting
            "py",
            "pyi",
            "pyw",
            "rb",
            "rake",
            "gemspec",
            "pl",
            "pm",
            "php",
            "lua",
            "r",
            "jl",
            "tcl",
            "awk",
            "dart",
            // JavaScript family
            "js",
            "mjs",
            "cjs",
            "jsx",
            "ts",
            "mts",
            "cts",
            "tsx",
            "vue",
            "svelte",
            "astro",
            "coffee",
            // shells
            "sh",
            "bash",
            "zsh",
            "fish",
            "ksh",
            "csh",
            "tcsh",
            "ps1",
            "psm1",
            "psd1",
            // functional
            "hs",
            "lhs",
            "elm",
            "ml",
            "mli",
            "ex",
            "exs",
            "erl",
            "hrl",
            "purs",
            "rkt",
            "scm",
            "ss",
            "lisp",
            "el",
            // stylesheets and templates
            "css",
            "scss",
            "sass",
            "less",
            "styl",
            "hbs",
            "handlebars",
            "mustache",
            "jinja",
            "jinja2",
            "j2",
            "ejs",
            "erb",
            "liquid",
            "twig",
            "njk",
            "pug",
            "haml",
            // query, schema and IDL
            "sql",
            "graphql",
            "gql",
            "proto",
            "thrift",
            "prisma",
            // build and infrastructure
            "cmake",
            "mk",
            "mak",
            "make",
            "bazel",
            "bzl",
            "nix",
            "tf",
            "tfvars",
            "hcl",
            "dockerfile",
            "containerfile",
            // hardware, shaders, numeric, legacy
            "v",
            "sv",
            "svh",
            "vhd",
            "vhdl",
            "asm",
            "s",
            "wat",
            "glsl",
            "vert",
            "frag",
            "hlsl",
            "wgsl",
            "metal",
            "f",
            "f90",
            "f95",
            "for",
            "pas",
            "ada",
            "adb",
            "ads",
            "cob",
            "cbl",
            "sol",
            "vim",
            // patches
            "diff",
            "patch",
        ],
        names: &[
            "Makefile",
            "GNUmakefile",
            "Dockerfile",
            "Containerfile",
            "Jenkinsfile",
            "Vagrantfile",
            "Gemfile",
            "Rakefile",
            "Podfile",
            "Brewfile",
            "Fastfile",
            "Procfile",
            "Justfile",
            "Tiltfile",
            "Snakefile",
            "Earthfile",
            "BUILD",
            "WORKSPACE",
            "PKGBUILD",
            ".bashrc",
            ".bash_profile",
            ".bash_logout",
            ".zshrc",
            ".zprofile",
            ".zshenv",
            ".profile",
            ".kshrc",
            ".vimrc",
            ".gvimrc",
            ".envrc",
        ],
        prefixes: &["Dockerfile.", "Containerfile.", "Makefile."],
    },
    Group {
        // Windows batch files: new line breaks are CRLF
        label: "code-windows",
        ty: ty(Family::Code, MAX_BYTES, Eol::Crlf),
        extensions: &["bat", "cmd"],
        names: &[],
        prefixes: &[],
    },
    Group {
        label: "config",
        ty: ty(Family::Config, MAX_BYTES, Eol::Lf),
        extensions: &[
            "json",
            "jsonc",
            "json5",
            "jsonl",
            "ndjson",
            "geojson",
            "webmanifest",
            "yaml",
            "yml",
            "toml",
            "ini",
            "cfg",
            "conf",
            "cnf",
            "properties",
            "env",
            "lock",
            "plist",
        ],
        names: &[
            ".gitignore",
            ".gitattributes",
            ".gitmodules",
            ".gitconfig",
            ".mailmap",
            ".editorconfig",
            ".npmrc",
            ".yarnrc",
            ".nvmrc",
            ".node-version",
            ".python-version",
            ".ruby-version",
            ".tool-versions",
            ".prettierrc",
            ".eslintrc",
            ".babelrc",
            ".stylelintrc",
            ".dockerignore",
            ".npmignore",
            ".eslintignore",
            ".prettierignore",
            ".hgignore",
            ".env",
            ".flake8",
            ".pylintrc",
            ".clang-format",
            ".clang-tidy",
            ".htaccess",
            ".curlrc",
            ".wgetrc",
            ".inputrc",
            "CODEOWNERS",
            "Caddyfile",
            "go.mod",
            "go.sum",
            "go.work",
            "Pipfile",
        ],
        prefixes: &[".env."],
    },
    Group {
        label: "markup",
        ty: ty(Family::Markup, MAX_BYTES, Eol::Lf),
        extensions: &[
            "html", "htm", "xhtml", "xml", "xsl", "xslt", "xsd", "dtd", "svg", "rss", "atom",
            "wsdl", "xaml", "csproj", "fsproj", "vbproj", "props", "targets", "resx", "tex",
            "latex", "sty", "cls", "bib", "rst", "adoc", "asciidoc", "org", "textile", "pod",
        ],
        names: &[],
        prefixes: &[],
    },
    Group {
        label: "diagram",
        ty: ty(Family::Diagram, MAX_BYTES, Eol::Lf),
        extensions: &[
            "mermaid", "mmd", "puml", "plantuml", "iuml", "dot", "gv", "drawio", "dio",
        ],
        names: &[],
        prefixes: &[],
    },
    Group {
        // Tables as text, until a grid pack takes these extensions over (it is asked first).
        // Only the two names every spreadsheet and database tool writes; other delimited
        // names (`.psv`, `.tab`, `.dsv`) stay unknown, opened as plain text under the strict
        // guard, so the grid pack's hand-over set is exactly this list. New line breaks are
        // LF, as in every other group: an Excel export already has CRLF breaks and keeps them.
        label: "csv",
        ty: ty(Family::CsvAsText, MAX_BYTES, Eol::Lf),
        extensions: &["csv", "tsv"],
        names: &[],
        prefixes: &[],
    },
];

/// Extensions that are never text for this pack: media, office documents, archives,
/// program binaries, fonts and databases. A host opens them some other way (tier 0).
pub const NOT_TEXT: &[&str] = &[
    // images
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "icns", "heic", "heif", "avif", "tif",
    "tiff", "psd", "svgz", // audio and video
    "mp3", "wav", "ogg", "flac", "m4a", "aac", "opus", "mp4", "webm", "mov", "mkv", "avi", "m4v",
    // documents
    "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "odt", "ods", "odp", "key", "pages",
    "numbers", "epub", // archives
    "zip", "gz", "tgz", "bz2", "xz", "zst", "7z", "rar", "tar", "jar", "war", "apk", "ipa", "dmg",
    "iso", // program binaries
    "wasm", "bin", "exe", "dll", "so", "dylib", "class", "o", "a", "lib", "pyc", "pyo", "pdb",
    // fonts
    "woff", "woff2", "ttf", "otf", "eot", // databases and 3D binaries
    "sqlite", "sqlite3", "db", "glb", "3mf", "blend",
];

/// How a path was matched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchedBy {
    Name,
    Prefix,
    Extension,
}

/// What the table says about a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lookup {
    /// A known text type.
    Known {
        group: &'static Group,
        by: MatchedBy,
    },
    /// Not in the table: text if it passes the strict guard ([`UNKNOWN`]).
    Unknown,
    /// Never text for this pack ([`NOT_TEXT`]).
    NotText,
}

impl PartialEq for Group {
    fn eq(&self, other: &Self) -> bool {
        self.label == other.label
    }
}
impl Eq for Group {}

impl Lookup {
    /// The type to open the file as: `None` for [`Lookup::NotText`].
    pub fn file_type(self) -> Option<FileType> {
        match self {
            Lookup::Known { group, .. } => Some(group.ty),
            Lookup::Unknown => Some(UNKNOWN),
            Lookup::NotText => None,
        }
    }
}

/// Decide a path (`/` or `\` separated; only the file name counts).
pub fn lookup(path: &str) -> Lookup {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    if name.is_empty() {
        return Lookup::Unknown;
    }
    let eq = |a: &str, b: &str| a.eq_ignore_ascii_case(b);
    let starts = |n: &str, p: &str| n.len() > p.len() && eq(&n[..p.len()], p);
    for g in TABLE {
        if g.names.iter().any(|n| eq(n, name)) {
            return Lookup::Known {
                group: g,
                by: MatchedBy::Name,
            };
        }
    }
    for g in TABLE {
        if g.prefixes
            .iter()
            .any(|p| name.is_char_boundary(p.len()) && starts(name, p))
        {
            return Lookup::Known {
                group: g,
                by: MatchedBy::Prefix,
            };
        }
    }
    let Some(ext) = name
        .rsplit_once('.')
        .and_then(|(stem, ext)| (!stem.is_empty() && !ext.is_empty()).then_some(ext))
    else {
        return Lookup::Unknown;
    };
    if NOT_TEXT.iter().any(|e| eq(e, ext)) {
        return Lookup::NotText;
    }
    for g in TABLE {
        if g.extensions.iter().any(|e| eq(e, ext)) {
            return Lookup::Known {
                group: g,
                by: MatchedBy::Extension,
            };
        }
    }
    Lookup::Unknown
}

/// Every extension in the table, in table order.
pub fn extensions() -> impl Iterator<Item = &'static str> {
    TABLE.iter().flat_map(|g| g.extensions.iter().copied())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn family(p: &str) -> Option<Family> {
        lookup(p).file_type().map(|t| t.family)
    }

    #[test]
    fn the_table_is_well_formed() {
        let mut seen = HashSet::new();
        for e in extensions().chain(NOT_TEXT.iter().copied()) {
            assert!(!e.is_empty() && !e.starts_with('.'), "{e:?}");
            assert_eq!(e, e.to_ascii_lowercase(), "{e:?} must be lowercase");
            assert!(seen.insert(e), "extension {e:?} listed twice");
        }
        let mut names = HashSet::new();
        for g in TABLE {
            for n in g.names.iter().chain(g.prefixes) {
                assert!(!n.contains('/'), "{n:?}");
                assert!(
                    names.insert(n.to_ascii_lowercase()),
                    "name {n:?} listed twice"
                );
            }
            for p in g.prefixes {
                assert!(p.ends_with('.'), "prefix {p:?} must end with a dot");
            }
            assert!(g.ty.settings.max_bytes > 0);
            assert_eq!(g.ty.settings.controls, Controls::Allow, "{}", g.label);
        }
        let labels: HashSet<&str> = TABLE.iter().map(|g| g.label).collect();
        assert_eq!(labels.len(), TABLE.len(), "group labels are unique");
        for f in Family::ALL {
            assert!(
                TABLE.iter().any(|g| g.ty.family == *f),
                "{f:?} has no group"
            );
        }
        let n = extensions().count();
        assert!(n >= 150, "the table lists {n} extensions");
    }

    #[test]
    fn lookups() {
        use Family::*;
        for (p, f) in [
            ("src/main.rs", Code),
            ("a/b/types.d.ts", Code),
            ("App.TSX", Code),
            ("Makefile", Code),
            ("makefile", Code),
            ("sub\\dir\\Dockerfile", Code),
            ("Dockerfile.dev", Code),
            ("x.Dockerfile", Code),
            ("Makefile.am", Code),
            ("install.bat", Code),
            ("package.json", Config),
            ("tsconfig.json", Config),
            ("docker-compose.yml", Config),
            ("Cargo.toml", Config),
            ("Cargo.lock", Config),
            (".gitignore", Config),
            (".env", Config),
            (".env.local", Config),
            ("go.mod", Config),
            ("index.html", Markup),
            ("icon.SVG", Markup),
            ("flow.mmd", Diagram),
            ("seq.puml", Diagram),
            ("deps.gv", Diagram),
            ("board.drawio", Diagram),
            ("notes.txt", Text),
            ("README", Text),
            ("server.log", Text),
            ("t.csv", CsvAsText),
            ("exports/Q3.CSV", CsvAsText),
            ("people.tsv", CsvAsText),
        ] {
            assert_eq!(family(p), Some(f), "{p}");
            assert!(matches!(lookup(p), Lookup::Known { .. }), "{p}");
        }
        for p in [
            "photo.png",
            "a.tar.gz",
            "Report.DOCX",
            "font.woff2",
            "x.svgz",
        ] {
            assert_eq!(lookup(p), Lookup::NotText, "{p}");
            assert_eq!(lookup(p).file_type(), None);
        }
        for p in [
            "data.xyz",
            "noext",
            ".hidden",
            "dir/",
            "trailingdot.",
            "a.md",
            "pipes.psv",
            "t.tab",
        ] {
            assert_eq!(lookup(p), Lookup::Unknown, "{p}");
            assert_eq!(lookup(p).file_type(), Some(UNKNOWN));
        }
        assert!(matches!(
            lookup("Makefile"),
            Lookup::Known {
                by: MatchedBy::Name,
                ..
            }
        ));
        assert!(matches!(
            lookup(".env.production"),
            Lookup::Known {
                by: MatchedBy::Prefix,
                ..
            }
        ));
        // a prefix alone is not a match: "Dockerfile." with nothing after it
        assert!(matches!(
            lookup("Dockerfile."),
            Lookup::Known {
                by: MatchedBy::Extension,
                ..
            } | Lookup::Unknown
        ));
        // per-type settings
        let s = |p: &str| lookup(p).file_type().unwrap().settings;
        assert_eq!(s("run.bat").new_eol, Eol::Crlf);
        assert_eq!(s("run.sh").new_eol, Eol::Lf);
        assert_eq!(s("app.log").max_bytes, 1 << 20);
        assert_eq!(s("app.txt").max_bytes, MAX_BYTES);
        assert_eq!(s("t.csv").max_bytes, MAX_BYTES);
        assert_eq!(s("t.csv").new_eol, Eol::Lf);
        assert_eq!(s("t.tsv").controls, Controls::Allow);
        assert_eq!(Family::CsvAsText.name(), "csv-as-text");
        assert_eq!(s("blob.xyz").controls, Controls::Refuse);
        assert_eq!(s("blob.txt").controls, Controls::Allow);
    }

    #[test]
    fn non_ascii_names_do_not_panic() {
        for p in [
            "é",
            "Dockerfilé",
            "ドキュメント.txt",
            "Dockerfil\u{e9}x",
            ".env\u{e9}",
        ] {
            let _ = lookup(p);
        }
        assert_eq!(family("ドキュメント.txt"), Some(Family::Text));
    }
}
