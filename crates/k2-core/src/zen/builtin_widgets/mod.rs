//! Built-in custom widgets: `k2:<name>@<n>` (prd-zen-user-widgets-v2
//! UWB21, UWB26).
//!
//! **Day-0 interface (Zen v2, 2026-10-08). Owner: B1** (the sources and
//! [`BUILTIN_WIDGETS`]); B2 reads [`WidgetRef`] in the Garden schema and the
//! bundler. Frozen: the name rule below and the `k2:diary@1` name.
//!
//! **The name rule.**
//! - A **user widget** is a folder name under `~/.k2/zen/widgets/`: the theme
//!   name rule (`valid_theme_name`: lower-case letters, digits, `-`, `_`,
//!   starting with a letter or digit, up to 40). It can never contain `:`.
//! - A **built-in widget** is `k2:<name>@<n>`: `<name>` follows the same rule,
//!   `<n>` is a version from 1 with no leading zero. It is compiled into
//!   k2-core (`include_str!`), read-only, never a folder.
//! - **Immutable per version.** Once a release ships `k2:diary@1`, its bytes
//!   never change (TUWB6 pins them by hash). Better code ships as
//!   `k2:diary@2`, and only a template moves to it, so an own-copy Garden
//!   keeps `@1` (prd-zen-garden-sync-defaults-v1 GS16, GS45a). There is no
//!   grant to carry (no permissions since 2026-10-08).
//! - A **Garden file** must name a built-in with its version
//!   (`widget = "k2:diary@1"`). `k2 zen widget new my-diary --from k2:diary`
//!   (no version) copies the latest version into a user folder to edit.

use std::fmt;

/// The Diary widget (UWB21, UWB24; Rosson 2026-10-07), as first released.
pub const DIARY_WIDGET: &str = "k2:diary@1";

/// The Diary every new Diary Garden uses (Rosson 2026-10-08, for 0.45.2):
/// @1 plus your sent words staying dark ink, markdown built as nodes, and a
/// ghost teasing in the empty pen. `diary-2.toml` moves the template to it.
pub const DIARY_WIDGET_LATEST: &str = "k2:diary@2";

/// The Kanban board (Rosson 2026-10-15, for 0.45.7): columns and cards in
/// the widget's Garden data (`data:k2:kanban`), the catalog's
/// `kanban-1.toml` fills the window with it.
pub const KANBAN_WIDGET: &str = "k2:kanban@1";

/// The Kanban every new Kanban Garden uses (Rosson 2026-10-18, for 0.46.1):
/// @1's board with the Thread panel in K2's Thread look (bubbles, "author ·
/// time", round Send, the working ear) and K2 Companion's colours as its
/// default. `kanban-2.toml` moved the template to it; an own copy keeps @1.
/// @3 (Rosson 2026-10-20, for 0.46.3): @2 with K2's stick-to-bottom in the
/// Thread panel. A message arriving never moves a reader who scrolled up
/// (WebKit reset @2's rebuilt list to the top); a "new messages" pill
/// counts what came in below. `kanban-3.toml` moved the template; an own
/// copy keeps the board it has.
/// @4 (Rosson 2026-10-20, Thread parity for 0.46.3): @3 with the Thread's
/// files and links: attachment chips (ok / changed / gone / hidden), images
/// drawn from K2's thumbnails, workspace file links, picked files riding a
/// post, mark-read, delta catch-up and paging back. `kanban-4.toml` moves
/// the template; an own copy keeps the board it has.
/// @5 (Rosson 2026-10-24, for 0.46.4: "the Kanban should get the same
/// update"): the board alone. Its Thread is K2's own Conversation widget in
/// a collapsible "Thread" column beside it (`pick = "all"`, following the
/// board), so nothing of the Thread is redrawn; the board's Thread button
/// folds that column and Send board posts to the agent its header shows.
/// `kanban-5.toml` moves the template; an own copy keeps the board it has.
pub const KANBAN_WIDGET_LATEST: &str = "k2:kanban@5";

/// Box usage's Garden card (prd-app-resources-disk-guard-v1 Slice B, for
/// 0.46.4): this computer's disks against the disk guard's lines, the
/// biggest users and their agents, what K2 clears on its own. Read only (no
/// caps, no buttons): it reads the garden-local `app.resources` once a
/// minute. Place it with `widget = "k2:box-disk@1"`.
pub const BOX_DISK_WIDGET: &str = "k2:box-disk@1";

/// The prefix every built-in widget name starts with.
pub const BUILTIN_PREFIX: &str = "k2:";

/// One built-in widget version: the same flat file set a user folder has.
#[derive(Debug, Clone, Copy)]
pub struct BuiltinWidget {
    pub name: &'static str,
    pub version: u32,
    /// `(file name, bytes)`: `manifest.json`, the entry HTML, `*.js`,
    /// `*.css`. Text only: images and fonts come from the standard library
    /// (`requires.libs`) or are inlined in CSS as `data:` URLs.
    pub files: &'static [(&'static str, &'static str)],
}

impl BuiltinWidget {
    /// `k2:<name>@<version>`.
    pub fn id(&self) -> String {
        format!("{BUILTIN_PREFIX}{}@{}", self.name, self.version)
    }
}

/// Every built-in widget version K2 ships, oldest first. An entry is never
/// removed or edited once released: better code is a new version with its
/// own folder (`diary-2/`), and only a template moves to it.
pub const BUILTIN_WIDGETS: &[BuiltinWidget] = &[
    BuiltinWidget {
        name: "diary",
        version: 1,
        files: &[
            ("manifest.json", include_str!("diary/manifest.json")),
            ("index.html", include_str!("diary/index.html")),
            ("diary.css", include_str!("diary/diary.css")),
            ("diary.js", include_str!("diary/diary.js")),
        ],
    },
    BuiltinWidget {
        name: "diary",
        version: 2,
        files: &[
            ("manifest.json", include_str!("diary-2/manifest.json")),
            ("index.html", include_str!("diary-2/index.html")),
            ("diary.css", include_str!("diary-2/diary.css")),
            ("diary.js", include_str!("diary-2/diary.js")),
        ],
    },
    // Garden v3 data-source cards (contract-sources-v1 §11.4). Each card is
    // its own files plus K2's card kit (`sources-kit/`), included byte for
    // byte so every version stays one flat, immutable file set.
    BuiltinWidget {
        name: "fleet-grid",
        version: 1,
        files: &[
            ("manifest.json", include_str!("fleet-grid-1/manifest.json")),
            ("index.html", include_str!("fleet-grid-1/index.html")),
            ("kit.css", include_str!("sources-kit/kit.css")),
            ("kit.js", include_str!("sources-kit/kit.js")),
            ("card.js", include_str!("fleet-grid-1/card.js")),
        ],
    },
    BuiltinWidget {
        name: "version-spread",
        version: 1,
        files: &[
            ("manifest.json", include_str!("version-spread-1/manifest.json")),
            ("index.html", include_str!("version-spread-1/index.html")),
            ("kit.css", include_str!("sources-kit/kit.css")),
            ("kit.js", include_str!("sources-kit/kit.js")),
            ("card.js", include_str!("version-spread-1/card.js")),
        ],
    },
    BuiltinWidget {
        name: "relays-dns",
        version: 1,
        files: &[
            ("manifest.json", include_str!("relays-dns-1/manifest.json")),
            ("index.html", include_str!("relays-dns-1/index.html")),
            ("kit.css", include_str!("sources-kit/kit.css")),
            ("kit.js", include_str!("sources-kit/kit.js")),
            ("card.js", include_str!("relays-dns-1/card.js")),
        ],
    },
    BuiltinWidget {
        name: "deploys",
        version: 1,
        files: &[
            ("manifest.json", include_str!("deploys-1/manifest.json")),
            ("index.html", include_str!("deploys-1/index.html")),
            ("kit.css", include_str!("sources-kit/kit.css")),
            ("kit.js", include_str!("sources-kit/kit.js")),
            ("card.js", include_str!("deploys-1/card.js")),
        ],
    },
    BuiltinWidget {
        name: "money-alerts",
        version: 1,
        files: &[
            ("manifest.json", include_str!("money-alerts-1/manifest.json")),
            ("index.html", include_str!("money-alerts-1/index.html")),
            ("kit.css", include_str!("sources-kit/kit.css")),
            ("kit.js", include_str!("sources-kit/kit.js")),
            ("card.js", include_str!("money-alerts-1/card.js")),
        ],
    },
    // The Kanban Garden's board (0.45.7): ported from Rosson's Roadmap
    // board widget, generic, its cards and columns in Garden data.
    BuiltinWidget {
        name: "kanban",
        version: 1,
        files: &[
            ("manifest.json", include_str!("kanban-1/manifest.json")),
            ("index.html", include_str!("kanban-1/index.html")),
            ("kanban.css", include_str!("kanban-1/kanban.css")),
            ("md.js", include_str!("kanban-1/md.js")),
            ("kanban.js", include_str!("kanban-1/kanban.js")),
            ("picker.js", include_str!("kanban-1/picker.js")),
            ("dock.js", include_str!("kanban-1/dock.js")),
        ],
    },
    // The Kanban's second version (0.46.1, Rosson 2026-10-18): the Thread
    // panel wears K2's Thread look in K2 Companion's colours. Its board
    // code (md.js, kanban.js, picker.js) is @1's; dock.js, kanban.css and
    // index.html are new. `kanban-1/` stays as shipped.
    BuiltinWidget {
        name: "kanban",
        version: 2,
        files: &[
            ("manifest.json", include_str!("kanban-2/manifest.json")),
            ("index.html", include_str!("kanban-2/index.html")),
            ("kanban.css", include_str!("kanban-2/kanban.css")),
            ("md.js", include_str!("kanban-2/md.js")),
            ("kanban.js", include_str!("kanban-2/kanban.js")),
            ("picker.js", include_str!("kanban-2/picker.js")),
            ("dock.js", include_str!("kanban-2/dock.js")),
        ],
    },
    // The Kanban's third version (0.46.3, Rosson 2026-10-20): @2 with the
    // Thread panel patched in place instead of rebuilt, K2's stick-to-bottom
    // (hooks/useStickToBottom.ts) and a "new messages" pill. Only dock.js and
    // kanban.css are new; the rest is @2's file, byte for byte. `kanban-2/`
    // stays as shipped.
    BuiltinWidget {
        name: "kanban",
        version: 3,
        files: &[
            ("manifest.json", include_str!("kanban-2/manifest.json")),
            ("index.html", include_str!("kanban-2/index.html")),
            ("kanban.css", include_str!("kanban-3/kanban.css")),
            ("md.js", include_str!("kanban-2/md.js")),
            ("kanban.js", include_str!("kanban-2/kanban.js")),
            ("picker.js", include_str!("kanban-2/picker.js")),
            ("dock.js", include_str!("kanban-3/dock.js")),
        ],
    },
    // The Kanban's fourth version (0.46.3 Thread parity, Rosson 2026-10-20):
    // @3 with the Thread panel's files (chips, pictures through <img> data:
    // URLs), workspace file links, K2's picker in the message box,
    // mark-read, delta catch-up and paging back, each behind `k2.can`. Only
    // dock.js and kanban.css are new; the rest is @2's file, byte for byte.
    // `kanban-3/` stays as shipped.
    BuiltinWidget {
        name: "kanban",
        version: 4,
        files: &[
            ("manifest.json", include_str!("kanban-2/manifest.json")),
            ("index.html", include_str!("kanban-2/index.html")),
            ("kanban.css", include_str!("kanban-4/kanban.css")),
            ("md.js", include_str!("kanban-2/md.js")),
            ("kanban.js", include_str!("kanban-2/kanban.js")),
            ("picker.js", include_str!("kanban-2/picker.js")),
            ("dock.js", include_str!("kanban-4/dock.js")),
        ],
    },
    // The Kanban's fifth version (0.46.4, Rosson 2026-10-24): the board
    // alone. The Thread is K2's built-in Conversation in its own column of
    // the Garden page (`kanban-5.toml`), so dock.js and picker.js are gone;
    // thread.js folds that column and sends the board to the agent it
    // shows. md.js and kanban.js are @2's, byte for byte; `kanban-4/` stays
    // as shipped, and @1–@4 stay for own copies (retirement ledger).
    BuiltinWidget {
        name: "kanban",
        version: 5,
        files: &[
            ("manifest.json", include_str!("kanban-5/manifest.json")),
            ("index.html", include_str!("kanban-5/index.html")),
            ("kanban.css", include_str!("kanban-5/kanban.css")),
            ("md.js", include_str!("kanban-2/md.js")),
            ("kanban.js", include_str!("kanban-2/kanban.js")),
            ("thread.js", include_str!("kanban-5/thread.js")),
        ],
    },
    // The data-source cards' second version (0.46.1): several sources per
    // card (config.source + config.sources, merged by item key), "soft crit
    // 2/3", named lists, K2 Web's ops shapes, and a refused subscription
    // that subscribes again by itself. `sources-kit-2/`; @1 stays as shipped.
    BuiltinWidget {
        name: "fleet-grid",
        version: 2,
        files: &[
            ("manifest.json", include_str!("fleet-grid-2/manifest.json")),
            ("index.html", include_str!("fleet-grid-2/index.html")),
            ("kit.css", include_str!("sources-kit-2/kit.css")),
            ("kit.js", include_str!("sources-kit-2/kit.js")),
            ("card.js", include_str!("fleet-grid-2/card.js")),
        ],
    },
    BuiltinWidget {
        name: "version-spread",
        version: 2,
        files: &[
            ("manifest.json", include_str!("version-spread-2/manifest.json")),
            ("index.html", include_str!("version-spread-2/index.html")),
            ("kit.css", include_str!("sources-kit-2/kit.css")),
            ("kit.js", include_str!("sources-kit-2/kit.js")),
            ("card.js", include_str!("version-spread-2/card.js")),
        ],
    },
    BuiltinWidget {
        name: "relays-dns",
        version: 2,
        files: &[
            ("manifest.json", include_str!("relays-dns-2/manifest.json")),
            ("index.html", include_str!("relays-dns-2/index.html")),
            ("kit.css", include_str!("sources-kit-2/kit.css")),
            ("kit.js", include_str!("sources-kit-2/kit.js")),
            ("card.js", include_str!("relays-dns-2/card.js")),
        ],
    },
    BuiltinWidget {
        name: "deploys",
        version: 2,
        files: &[
            ("manifest.json", include_str!("deploys-2/manifest.json")),
            ("index.html", include_str!("deploys-2/index.html")),
            ("kit.css", include_str!("sources-kit-2/kit.css")),
            ("kit.js", include_str!("sources-kit-2/kit.js")),
            ("card.js", include_str!("deploys-2/card.js")),
        ],
    },
    BuiltinWidget {
        name: "money-alerts",
        version: 2,
        files: &[
            ("manifest.json", include_str!("money-alerts-2/manifest.json")),
            ("index.html", include_str!("money-alerts-2/index.html")),
            ("kit.css", include_str!("sources-kit-2/kit.css")),
            ("kit.js", include_str!("sources-kit-2/kit.js")),
            ("card.js", include_str!("money-alerts-2/card.js")),
        ],
    },
    // The data-source cards' third version (0.46.4, Rosson 2026-10-23: "not
    // useful"): @2 without "Ask an agent" on a tile, a row or the money
    // banner (`sources-kit-3/`, and money-alerts' own button). Manifest,
    // page and style are @2's bytes; @1 and @2 stay as shipped (their button
    // now gets k2.sources.ask's `retired` answer).
    BuiltinWidget {
        name: "fleet-grid",
        version: 3,
        files: &[
            ("manifest.json", include_str!("fleet-grid-2/manifest.json")),
            ("index.html", include_str!("fleet-grid-2/index.html")),
            ("kit.css", include_str!("sources-kit-2/kit.css")),
            ("kit.js", include_str!("sources-kit-3/kit.js")),
            ("card.js", include_str!("fleet-grid-3/card.js")),
        ],
    },
    BuiltinWidget {
        name: "version-spread",
        version: 3,
        files: &[
            ("manifest.json", include_str!("version-spread-2/manifest.json")),
            ("index.html", include_str!("version-spread-2/index.html")),
            ("kit.css", include_str!("sources-kit-2/kit.css")),
            ("kit.js", include_str!("sources-kit-3/kit.js")),
            ("card.js", include_str!("version-spread-3/card.js")),
        ],
    },
    BuiltinWidget {
        name: "relays-dns",
        version: 3,
        files: &[
            ("manifest.json", include_str!("relays-dns-2/manifest.json")),
            ("index.html", include_str!("relays-dns-2/index.html")),
            ("kit.css", include_str!("sources-kit-2/kit.css")),
            ("kit.js", include_str!("sources-kit-3/kit.js")),
            ("card.js", include_str!("relays-dns-3/card.js")),
        ],
    },
    BuiltinWidget {
        name: "deploys",
        version: 3,
        files: &[
            ("manifest.json", include_str!("deploys-2/manifest.json")),
            ("index.html", include_str!("deploys-2/index.html")),
            ("kit.css", include_str!("sources-kit-2/kit.css")),
            ("kit.js", include_str!("sources-kit-3/kit.js")),
            ("card.js", include_str!("deploys-3/card.js")),
        ],
    },
    BuiltinWidget {
        name: "money-alerts",
        version: 3,
        files: &[
            ("manifest.json", include_str!("money-alerts-2/manifest.json")),
            ("index.html", include_str!("money-alerts-2/index.html")),
            ("kit.css", include_str!("sources-kit-2/kit.css")),
            ("kit.js", include_str!("sources-kit-3/kit.js")),
            ("card.js", include_str!("money-alerts-3/card.js")),
        ],
    },
    BuiltinWidget {
        name: "box-disk",
        version: 1,
        files: &[
            ("manifest.json", include_str!("box-disk-1/manifest.json")),
            ("index.html", include_str!("box-disk-1/index.html")),
            ("card.css", include_str!("box-disk-1/card.css")),
            ("card.js", include_str!("box-disk-1/card.js")),
        ],
    },
];

/// The data-source cards and the presets each pairs with (contract
/// §11.4): `k2 sources propose --preset <preset>` makes the source.
/// The newest version of each (0.46.4: `@3`, no "Ask an agent"); `@1` and
/// `@2` still run in Gardens that name them.
pub const SOURCE_CARDS: &[(&str, &[&str])] = &[
    ("k2:fleet-grid@3", &["k2-fleet", "k2-fleet-summary"]),
    ("k2:version-spread@3", &["k2-fleet", "k2-fleet-summary", "github-latest-release"]),
    ("k2:relays-dns@3", &["k2-fleet-summary", "gatus"]),
    ("k2:deploys@3", &["vercel-deployments"]),
    ("k2:money-alerts@3", &["stripe-past-due"]),
];

/// The sha256 of each released built-in's files, `name\0bytes\0` in list
/// order (TUWB6). A changed byte fails `released_builtins_never_change`:
/// ship a new version instead.
pub const BUILTIN_WIDGET_HASHES: &[(&str, &str)] = &[
    ("k2:diary@1", "d1b03245b1dbed5d01c6498be2b5c8626084fa8b594f8a3f5d3a4ae69348166d"),
    ("k2:diary@2", "a38b8bb61324f22d6803d3dc4de38138f913337e90082bcf2b4a9fa3269ec07f"),
    ("k2:fleet-grid@1", "fc758ce7eaac8aaa4e79aef4251aa3682ede3430f185b6d426a65f1f4c56d2c1"),
    ("k2:version-spread@1", "051f4b389a0fef87f2a5895abb43f7b2c58048986989c7d0d458a4176efe505b"),
    ("k2:relays-dns@1", "8b54362a6ef697f6575008790b975a0d793d08aa14c5974f2447a3a54715062e"),
    ("k2:deploys@1", "7b0f85cc0e7035fa62cb2d8df96653ed2b293be8a5b8d4a46ffec6f00090dfdf"),
    ("k2:money-alerts@1", "52d89d4d821428d496e38b133f4849c1c697abc5c7d3acce21f54e8fdd1cf4a7"),
    ("k2:kanban@1", "0baefa0890243635a3cc181c522ed7a59bc55272d84f5ffc293d3106bfaa786a"),
    ("k2:fleet-grid@2", "f66df538d95282f2e59f13620f72379d021a630a7a1eb7757a45116fa2388ae3"),
    ("k2:version-spread@2", "ea6032c420054fe7d8d12710d9edea7fbafe4b5a98a871f8dffaac76c1c4c3be"),
    ("k2:relays-dns@2", "ba1c6b83f7ae7f5d0b3cc37ef9d363dbf207ff7ce614fba195712a78beda0f78"),
    ("k2:deploys@2", "77bdf1a576a2a3b10acd7337d179200ffdf0bab81fe8f7a8666bbf5147c2a640"),
    ("k2:money-alerts@2", "aff2ee34006f71436793145e1989623349a82b6d538beb34d51e94074b69a5c7"),
    ("k2:kanban@2", "8ac36405d18351d0bb52a8f71426dffb32ad99831bcfd9b7c87e4b017d0c7884"),
    ("k2:kanban@3", "e9aae003a07e3d4e17e39738b376aab6ab7556250cd3a6e662211521e8425682"),
    ("k2:kanban@4", "81cff0f7ee199170f8531b0dcfcbdf4f39387810bcfc91ac0492f5d7fba2a3bf"),
    ("k2:kanban@5", "d591d4b1a8872fc9b67e1a303f5bc873a990cc3fcf292127558833a9f38896ca"),
    ("k2:fleet-grid@3", "2d5e128bfd793463df4bfb7c17252c8468bfc6fa89e019e5bf4ef715b374a5cc"),
    ("k2:version-spread@3", "ed09b33978cddee93ba2d7b0297c2e71471504aacda0fda014d0814011d3c53b"),
    ("k2:relays-dns@3", "4cfc13971951b12442d9d40f05a2e7e4417af5094c4a016c7c182a332c584a0d"),
    ("k2:deploys@3", "6a9e85ef5a3ac11635533ef862ef426ff82d077ef70985a8650162c980481686"),
    ("k2:money-alerts@3", "07a5203fced5553be85ef51222ce86ab4646f9001cfdf5c029981470289a5bf5"),
    ("k2:box-disk@1", "544d39934b526377777dc1ec832bf77096df1b4dee123f7d25727ef7864c19ce"),
];

/// The hash [`BUILTIN_WIDGET_HASHES`] pins.
pub fn builtin_widget_hash(w: &BuiltinWidget) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    for (name, bytes) in w.files {
        h.update(name.as_bytes());
        h.update([0u8]);
        h.update(bytes.as_bytes());
        h.update([0u8]);
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// A parsed `widget = "…"` value or `--from` argument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WidgetRef {
    /// A folder under `~/.k2/zen/widgets/`.
    User(String),
    /// `k2:<name>@<n>`, or `k2:<name>` (latest; only valid for `--from`).
    Builtin { name: String, version: Option<u32> },
}

impl fmt::Display for WidgetRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WidgetRef::User(n) => f.write_str(n),
            WidgetRef::Builtin { name, version: Some(v) } => write!(f, "{BUILTIN_PREFIX}{name}@{v}"),
            WidgetRef::Builtin { name, version: None } => write!(f, "{BUILTIN_PREFIX}{name}"),
        }
    }
}

/// Parse a widget reference. Errors are one plain sentence.
pub fn parse_widget_ref(s: &str) -> Result<WidgetRef, String> {
    let Some(rest) = s.strip_prefix(BUILTIN_PREFIX) else {
        if super::valid_theme_name(s) {
            return Ok(WidgetRef::User(s.to_string()));
        }
        return Err(format!(
            "'{s}' isn't a widget name: lower-case letters, digits, - and _, up to 40, or a built-in like {DIARY_WIDGET}"
        ));
    };
    let (name, version) = match rest.split_once('@') {
        None => (rest, None),
        Some((n, v)) => {
            let ok = !v.is_empty() && !v.starts_with('0') && v.chars().all(|c| c.is_ascii_digit()) && v.len() <= 6;
            let v: u32 = if ok { v.parse().map_err(|_| format!("bad version in '{s}'"))? } else { 0 };
            if v == 0 {
                return Err(format!("'{s}': a built-in widget version is a whole number from 1, like {DIARY_WIDGET}"));
            }
            (n, Some(v))
        }
    };
    if !super::valid_theme_name(name) {
        return Err(format!("'{s}' isn't a built-in widget name; built-ins look like {DIARY_WIDGET}"));
    }
    Ok(WidgetRef::Builtin { name: name.to_string(), version })
}

/// The built-in for a reference: an exact version, or the latest when
/// `version` is `None`. `None` for a user widget or an unknown built-in.
pub fn builtin_widget(r: &WidgetRef) -> Option<&'static BuiltinWidget> {
    let WidgetRef::Builtin { name, version } = r else { return None };
    BUILTIN_WIDGETS
        .iter()
        .filter(|w| w.name == name && version.is_none_or(|v| w.version == v))
        .max_by_key(|w| w.version)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_and_builtin_names_parse() {
        assert_eq!(parse_widget_ref("agent-arcade"), Ok(WidgetRef::User("agent-arcade".into())));
        assert_eq!(
            parse_widget_ref(DIARY_WIDGET),
            Ok(WidgetRef::Builtin { name: "diary".into(), version: Some(1) })
        );
        assert_eq!(parse_widget_ref("k2:diary"), Ok(WidgetRef::Builtin { name: "diary".into(), version: None }));
        assert_eq!(parse_widget_ref("k2:diary@12").map(|r| r.to_string()), Ok("k2:diary@12".into()));
    }

    #[test]
    fn bad_names_are_refused() {
        for bad in ["", "Diary", "k2:", "k2:Diary@1", "k2:diary@", "k2:diary@0", "k2:diary@01", "k2:diary@x",
            "k2:diary@1@2", "x:diary@1", "../diary", "k2:diary@1234567"]
        {
            assert!(parse_widget_ref(bad).is_err(), "'{bad}' must be refused");
        }
    }

    #[test]
    fn released_builtins_are_unique_and_named_by_the_rule() {
        for (i, w) in BUILTIN_WIDGETS.iter().enumerate() {
            assert!(w.version >= 1);
            assert_eq!(parse_widget_ref(&w.id()), Ok(WidgetRef::Builtin { name: w.name.into(), version: Some(w.version) }));
            assert!(
                !BUILTIN_WIDGETS[..i].iter().any(|o| o.name == w.name && o.version == w.version),
                "{} listed twice",
                w.id()
            );
            assert!(w.files.iter().any(|(f, _)| *f == "manifest.json"), "{} has a manifest", w.id());
        }
    }

    /// TUWB6: a released built-in's bytes are pinned. To change the Diary,
    /// add `k2:diary@2`; never edit `@1`.
    #[test]
    fn released_builtins_never_change() {
        for w in BUILTIN_WIDGETS {
            let id = w.id();
            let pinned = BUILTIN_WIDGET_HASHES
                .iter()
                .find(|(i, _)| *i == id)
                .map(|(_, h)| *h)
                .unwrap_or_else(|| panic!("{id} has no pinned hash in BUILTIN_WIDGET_HASHES"));
            assert_eq!(builtin_widget_hash(w), pinned, "{id} changed; ship a new version instead");
        }
        assert_eq!(BUILTIN_WIDGET_HASHES.len(), BUILTIN_WIDGETS.len(), "one pin per built-in");
    }

    /// 0.46.5 WS8 (vs-live L7): every built-in stays under the large-widget
    /// warning, so no K2 widget ever shows it.
    #[test]
    fn every_builtin_stays_under_the_large_widget_warning() {
        for w in BUILTIN_WIDGETS {
            let id = w.id();
            let c = crate::zen::widgets::check_files(&format!("builtin:{id}"), w.name, &crate::zen::widgets::builtin_files(w));
            let b = c.bundle.as_ref().unwrap_or_else(|| panic!("{id} bundles: {:?}", c.errors));
            assert!(b.code_bytes <= crate::zen::bundle::WARN_CODE_BYTES, "WS8: {id} is {} bytes of code", b.code_bytes);
            assert!(!c.warnings.iter().any(|d| d.message.starts_with("this widget is large")), "{id}: {:?}", c.warnings);
        }
    }

    /// Every released Diary version, oldest first.
    fn diaries() -> Vec<&'static BuiltinWidget> {
        let all: Vec<&BuiltinWidget> = BUILTIN_WIDGETS.iter().filter(|w| w.name == "diary").collect();
        assert_eq!(all.iter().map(|w| w.id()).collect::<Vec<_>>(), [DIARY_WIDGET, DIARY_WIDGET_LATEST]);
        all
    }

    fn file(w: &BuiltinWidget, n: &str) -> &'static str {
        w.files.iter().find(|(f, _)| *f == n).map(|(_, b)| *b).unwrap_or_else(|| panic!("{} has {n}", w.id()))
    }

    #[test]
    fn the_diary_is_a_clean_custom_widget() {
        for w in diaries() {
            clean_diary(w);
        }
        assert_eq!(builtin_widget(&parse_widget_ref("k2:diary").expect("parses")).map(|w| w.id()).as_deref(), Some(DIARY_WIDGET_LATEST));
    }

    fn clean_diary(w: &'static BuiltinWidget) {
        let file = |n: &str| w.files.iter().find(|(f, _)| *f == n).map(|(_, b)| *b).unwrap_or_else(|| panic!("diary has {n}"));
        let manifest: serde_json::Value = serde_json::from_str(file("manifest.json")).expect("manifest is JSON");
        assert_eq!(manifest["schema"], 1);
        assert_eq!(manifest["entry"], "index.html");
        let caps: Vec<&str> = manifest["caps"].as_array().expect("caps").iter().filter_map(|c| c.as_str()).collect();
        assert_eq!(caps, ["agents:read", "thread:read", "thread:post"]);
        for c in &caps {
            assert!(crate::zen::USER_WIDGET_CAPS.contains(c), "{c}");
            assert!(manifest["reasons"][*c].as_str().is_some_and(|r| r.len() <= 140), "a reason for {c}");
        }
        // Libraries from K2's standard library, named by major version so a
        // vendored minor bump never breaks an immutable built-in (UWB13,
        // UWB24: perfect-freehand and the Caveat handwriting font).
        let libs: Vec<crate::zen::stdlib::LibRef> = manifest["requires"]["libs"]
            .as_array()
            .expect("requires.libs")
            .iter()
            .map(|l| crate::zen::stdlib::LibRef::from_json(l).unwrap_or_else(|e| panic!("{e}")))
            .collect();
        let named: Vec<(String, String)> = libs
            .iter()
            .map(|l| match l {
                crate::zen::stdlib::LibRef::Named { id, version } => (id.clone(), version.clone()),
                other => panic!("a built-in never loads from a CDN: {other:?}"),
            })
            .collect();
        // @2 adds PixiJS (the paper under the words) and Tone.js (the room's
        // music), Rosson 2026-10-08.
        let want: &[(&str, &str)] = if w.id() == DIARY_WIDGET {
            &[("perfect-freehand", "1"), ("font-caveat", "5")]
        } else {
            &[("perfect-freehand", "1"), ("font-caveat", "5"), ("pixi.js", "8"), ("tone", "15")]
        };
        let want: Vec<(String, String)> = want.iter().map(|(i, v)| (i.to_string(), v.to_string())).collect();
        assert_eq!(named, want, "{} names its libraries by major version", w.id());
        let html = file("index.html");
        let js = file("diary.js");
        // UW10's errors: no inline handlers, no network, no frames.
        for bad in [" onclick=", " onload=", " onerror=", "<iframe", "<object", "<embed", "<base", "http-equiv", "src=\"http", "href=\"http", "//cdn"] {
            assert!(!html.contains(bad), "index.html has {bad}");
        }
        assert!(html.contains("<script src=\"diary.js\"></script>") && html.contains("href=\"diary.css\""));
        // UW10's warnings and the skill's rules: textContent, never innerHTML.
        for bad in ["innerHTML", "outerHTML", "insertAdjacentHTML", "document.write", "fetch(", "XMLHttpRequest", "WebSocket", "EventSource", "localStorage", "sessionStorage", "indexedDB", "eval(", "new Function", "import "] {
            assert!(!js.contains(bad), "diary.js uses {bad}");
        }
        // Only widget helpers the catalog exposes.
        let widget: Vec<String> = crate::contract::catalog()
            .verbs_exposed_to(crate::contract::Exposure::Widget)
            .iter()
            .map(|v| v.verb.clone())
            .collect();
        for (i, _) in js.match_indices("k2.") {
            let name: String = js[i + 3..].chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '.').collect();
            let name = name.trim_end_matches('.');
            let ok = widget.iter().any(|w| w == name)
                || crate::contract::gen::RUNTIME_MEMBERS.contains(&name)
                || ["thread", "agents", "theme", "conversation", "motion.reduced"].contains(&name);
            assert!(ok, "diary.js calls k2.{name}, which isn't a widget helper");
        }
        let code: usize = w.files.iter().filter(|(f, _)| *f != "manifest.json").map(|(_, b)| b.len()).sum();
        assert!(code <= crate::zen::bundle::WARN_CODE_BYTES, "WS8: {} is {code} bytes of code", w.id());
        // Rosson 2026-10-08: one page per agent on THIS computer. K2 binds
        // the grant to `{server: "local"}`; the page list keeps only
        // `<handle>::local` rows as a second lock.
        assert!(js.contains("var LOCAL_HOST = 'local'") && js.contains(".filter(isLocal)"), "the Diary keeps local agents only");
        // Pages turn by drag, click and keys; reduced motion stills it all.
        for needle in ["'pointerdown'", "'pointermove'", "'ArrowRight'", "'ArrowLeft'", "'PageDown'", "rotateY("] {
            assert!(js.contains(needle), "diary.js lost {needle}");
        }
        let css = file("diary.css");
        assert!(css.contains("@media (prefers-reduced-motion: reduce)") && css.contains(":root.reduced"), "reduced motion");
        assert!(js.contains("if (reduced()) {"), "a reduced-motion turn is instant");
        // Nothing but haunted controls on the page: no agent chooser, no
        // theme or model buttons.
        for bad in ["id=\"search\"", "id=\"back\"", "Contents", "theme.get", "theme.changed"] {
            assert!(!html.contains(bad) && !js.contains(bad), "the Diary still has {bad}");
        }
    }

    /// k2:diary@2 (Rosson 2026-10-08): what you sent stays dark ink, words
    /// read as markdown built as nodes, and the ghost in the empty pen only
    /// ever teases for fears, secrets, names and memories, never anything
    /// real and sensitive (its words go to an agent).
    #[test]
    fn diary_2_keeps_your_ink_reads_markdown_and_its_ghost_asks_for_nothing_real() {
        let w = builtin_widget(&parse_widget_ref(DIARY_WIDGET_LATEST).expect("parses")).expect("k2:diary@2 ships");
        let (js, css, html) = (file(w, "diary.js"), file(w, "diary.css"), file(w, "index.html"));
        // Your words: no rule fades, blurs or slants them.
        let mine: Vec<&str> = css.split('}').filter(|r| r.split('{').next().is_some_and(|sel| sel.contains(".entry.mine"))).collect();
        assert!(!mine.is_empty(), "diary.css styles your words");
        for rule in mine {
            for bad in ["opacity", "blur", "filter", "italic"] {
                assert!(!rule.contains(bad), "your sent words fade again ({bad}): {rule}");
            }
        }
        assert!(!js.contains("absorbed"), "nothing marks sent words to fade");
        // Markdown is built node by node (the clean check refuses innerHTML).
        for needle in ["function markdown(", "createTreeWalker", "'md-link'", "'md-url'"] {
            assert!(js.contains(needle), "diary.js lost {needle}");
        }
        assert!(!js.contains(".href") && !js.contains("'href'") && !js.contains("setAttribute('on"), "a link is words, never a target");
        // The ghost: a placeholder over the pen, never its value.
        assert!(html.contains("id=\"ghost\" aria-hidden=\"true\" hidden"), "the ghost is hidden from readers");
        assert!(!js.contains("ink.value = GHOST") && !js.contains("ink.value = words.charAt"), "the ghost never writes into the textarea");
        for needle in ["'visibilitychange'", "ghostFreeze()", "if (reduced()) {", "GHOST_STILL", "if (!ghost.woke) ghostStart(false)"] {
            assert!(js.contains(needle), "diary.js lost {needle}");
        }
        // No plain "write here" anywhere, and no buttons on the page but its
        // corners (Rosson 2026-10-08: no seal, no whispers switch).
        assert!(!html.contains("placeholder=") && !js.contains(".placeholder = '") && !html.contains("id=\"hint\""), "a plain prompt is back");
        assert!(html.contains("aria-label=\"Write in the diary\""), "the pen keeps a name for screen readers");
        assert!(!html.contains("id=\"send\"") && !html.contains("id=\"whispers\"") && !js.contains("AudioContext"), "seal and whispers are gone");
        // The pen scratches while the agent works: SVG built in code, each
        // stroke drawn by its dash offset; the page glides, one write a frame.
        for needle in ["createElementNS(SVG_NS, 'path')", "strokeDashoffset", "function glideStep(", "Math.exp(-dt / GLIDE_TAU_MS)"] {
            assert!(js.contains(needle), "diary.js lost {needle}");
        }
        assert!(!js.contains("scrollIntoView"), "the page glides; it never jumps to a line");
        // Ghost counts (0.45.2's AgentRow.counts) are read only when K2 sends
        // them; an older K2's row shows nothing extra.
        assert!(js.contains("function counted(row)") && js.contains("if (!c || typeof c !== 'object') return null"), "counts are feature-detected");
        assert!(js.contains("clamp(q[0], SCRIBE_PAD, SCRIBE_W - SCRIBE_PAD)"), "scribble points stay inside the drawing");
        // Ink, paper and sound (Rosson 2026-10-08). The working scribble is
        // the old wild stroked pen ("more scary/chaotic"); perfect-freehand
        // draws only the flourish, the blots and the speaker.
        for needle in ["function inkOutline(", "simulatePressure: o.pressure !== false", "function frustration()", "p.style.strokeDashoffset = String(len)"] {
            assert!(js.contains(needle), "diary.js lost {needle}");
        }
        assert!(!js.contains("SCRIBE_INK") && !js.contains("scribe-hand-"), "the scribble went back to filled outlines");
        // The paper: one WebGL canvas under the words, drawn on demand (30
        // fps at most while ink is wet), halted when unseen, one still frame
        // under reduced motion; the candle steps with the CSS flicker.
        for needle in ["window.PIXI", "preference: 'webgl'", "autoStart: false", "autoDensity: true", "dom.page.insertBefore(canvas, dom.page.firstChild)", "var r = dom.page.getBoundingClientRect()", "new window.ResizeObserver(fxResized)"] {
            assert!(js.contains(needle), "diary.js lost {needle}");
        }
        // One flame drives every candle (the room's, the page glow, the
        // shader's light), sampled once a frame; no CSS clock of its own.
        for needle in ["function flameAt(ms)", "var FLAME_FPS = 20", "dom.candle.style.opacity", "dom.glow.style.opacity", "u.uLight[3] = light", "var on = shown() && !reduced()"] {
            assert!(js.contains(needle), "diary.js lost {needle}");
        }
        assert!(!css.contains("@keyframes flicker") && !css.contains("@keyframes gutter"), "a candle runs on its own CSS clock again");
        // The music: made in code, quiet, on arrival, gone when unseen or
        // removed, muted from the top left or with M; never an AudioWorklet
        // node (refused in a sealed widget).
        for needle in ["window.Tone", "var MUSIC_DB = -24", "music.arrived = true", "'pagehide', musicClose", "raw.suspend()", "(e.key === 'm' || e.key === 'M') && e.target !== dom.ink"] {
            assert!(js.contains(needle), "diary.js lost {needle}");
        }
        for bad in ["Freeverb", "JCReverb", "FeedbackCombFilter", "BitCrusher", "Tone.start", "Player(", ".mp3", ".ogg", ".wav"] {
            assert!(!js.contains(bad), "the music uses {bad}");
        }
        assert!(html.contains("<button class=\"hush\" id=\"hush\" type=\"button\" aria-label=\"Mute the music (M)\" aria-pressed=\"false\">"), "the speaker: named and always shown");
        assert!(js.contains("function musicState()") && js.contains("'blocked by the browser'") && js.contains("'waiting for a click'"), "the speaker says what the music is doing");
        let hush = css.split("\n.hush {").nth(1).and_then(|r| r.split('}').next()).expect(".hush rule");
        assert!(hush.contains("top: 58px;") && hush.contains("left: 14px;") && !hush.contains("right:"), "the speaker sits top left, under K2's floating top band");
        assert!(hush.contains("border-radius: 999px;") && hush.contains("background: var(--zen-surface"), "the speaker is a Zen glass tile");
        // Corners curl on reach; no dog-ears; square pages.
        assert!(js.contains("function curlSvg(name)") && !css.contains("var(--night) 0 50%"), "the corners are dog-eared again");
        assert!(css.contains(".haunt") && css.contains(":root.asleep .haunt *") && css.contains(":root.reduced .haunt *"), "the room moves, stills and sleeps");
        let start = js.find("var GHOST_LINES = [").expect("GHOST_LINES");
        let end = start + js[start..].find("\n  ]").expect("GHOST_LINES ends");
        let lines: Vec<String> = js[start..end].lines().skip(1).filter_map(|l| l.trim().strip_prefix('\'')?.strip_suffix("',").map(str::to_string)).collect();
        assert!((15..=20).contains(&lines.len()), "15 to 20 teases, got {}: {lines:?}", lines.len());
        let words = ["pin", "pins", "card", "cards", "code", "codes", "ssn", "cvv", "cvc", "otp", "iban", "key", "keys", "bank", "token", "account", "accounts", "zip"];
        let parts = ["password", "passcode", "passphrase", "login", "log in", "sign in", "credential", "credit", "debit", "social security",
            "routing", "address", "email", "e-mail", "phone", "birthday", "date of birth", "maiden", "security question", "username",
            "wallet", "seed phrase", "recovery", "verification", "postcode", "license", "passport"];
        for l in &lines {
            let lower = l.to_lowercase();
            for p in parts {
                assert!(!lower.contains(p), "the ghost asks for something real ({p}): {l}");
            }
            for t in lower.split(|c: char| !c.is_alphanumeric()) {
                assert!(!words.contains(&t), "the ghost asks for something real ({t}): {l}");
            }
        }
    }

    /// k2:kanban@1 (Rosson 2026-10-15, for 0.45.7): a clean custom widget
    /// that keeps its board in Garden data, renders notes with the bundled
    /// marked + DOMPurify, drags with pointer events, and ships none of the
    /// Roadmap board's K2-internal cards or hints.
    #[test]
    fn the_kanban_is_a_clean_custom_widget_with_a_generic_board() {
        let w = builtin_widget(&parse_widget_ref(KANBAN_WIDGET).expect("parses")).expect("k2:kanban@1 ships");
        clean_kanban(w);
        // Theme: colours come from K2's --zen-* variables, not a fixed dark palette.
        let css = file(w, "kanban.css");
        for token in ["--zen-surface", "--zen-text", "--zen-border", "--zen-accent", "--zen-font-family", "--zen-radius"] {
            assert!(css.contains(token), "kanban.css ignores {token}");
        }
        for dark in ["#0d1117", "#161b22", "#1c2330", "padding-top:56px", "padding-top: 56px"] {
            assert!(!css.contains(dark), "kanban.css still has {dark}");
        }
    }

    /// k2:kanban@2 (Rosson 2026-10-18, for 0.46.1): @1's board, clean the
    /// same way, with the Thread panel in K2's Thread look (ThreadZenKit:
    /// bubbles yours right / the agent's left, "author · time" under each,
    /// markdown, a round Send, Enter sends and Shift+Enter is a new line, the
    /// working ear with its counters and clock) and K2 Companion's colours by
    /// default, light and dark from the Garden's scheme; `look = "garden"`
    /// still follows the Garden theme's --zen-* tokens. @2 still resolves
    /// for own copies (the bare `k2:kanban` is @3 since 0.46.3).
    #[test]
    fn the_kanban_2_wears_the_thread_look_in_k2_companion_colours() {
        let w = builtin_widget(&parse_widget_ref("k2:kanban@2").expect("parses")).expect("k2:kanban@2 ships");
        clean_kanban(w);
        let one = builtin_widget(&parse_widget_ref(KANBAN_WIDGET).expect("parses")).expect("k2:kanban@1 ships");
        // The board itself is @1's, byte for byte; the panel is new.
        for f in ["manifest.json", "md.js", "picker.js"] {
            assert_eq!(file(w, f), file(one, f), "{f} is @1's");
        }
        assert_eq!(file(w, "kanban.js").replacen("k2:kanban@2", "k2:kanban@1", 1), file(one, "kanban.js"), "kanban.js is @1's");
        // A message's code block or table scrolls sideways only, never up
        // and down inside its bubble (0.46.1 RC smoke).
        let css = file(w, "kanban.css");
        for needle in [
            ".md pre { margin: 0 0 6px; padding: 6px 8px; overflow-x: auto; overflow-y: hidden;",
            ".md table { border-collapse: collapse; margin: 0 0 6px; display: block; overflow-x: auto; overflow-y: hidden; }",
        ] {
            assert!(css.contains(needle), "kanban.css lost {needle}");
        }
        let html = file(w, "index.html");
        for needle in ["class=\"send-round\"", "title=\"Send (Enter)\"", "aria-label=\"Send\"", "class=\"dock-turn thread-ear\"", "class=\"compose-box\""] {
            assert!(html.contains(needle), "index.html lost {needle}");
        }
        let js = file(w, "dock.js");
        for needle in [
            "doc.via === 'compose'",
            "'msg ' + (mine ? 'mine' : 'theirs')",
            "'bubble ' + (mine ? 'bubble-me' : 'bubble-agent')",
            "authorOf(doc, mine)",
            "' · '",
            "fillMarkdown(body, doc.body || '')",
            "e.key !== 'Enter' || e.shiftKey",
            "k2.thread.post(to, text)",
            "k2.thread.subscribe(row.address, drawView",
            "countTags(t.counts)",
            "stripClock(now - since)",
            "look === 'garden'",
            "seenLabels = labels",
        ] {
            assert!(js.contains(needle), "dock.js lost {needle}");
        }
        let css = file(w, "kanban.css");
        // K2 Companion's palette (dark: the phone app's; light: iOS's).
        for c in ["#0a0a0a", "#1c1c1e", "#0a84ff", "#3a3a3c", "#e4e4e7", "#007aff", "#e9e9eb", "#f2f2f7", "-apple-system"] {
            assert!(css.contains(c), "kanban.css lost Companion's {c}");
        }
        for rule in [
            ":root[data-zen-scheme=\"dark\"]",
            ":root.look-garden",
            "--kb-bubble-radius: 18px",
            "--kb-field-radius: 20px",
            ".bubble-me {",
            ".bubble-agent {",
            ".msg.mine { align-self: flex-end",
            ".msg.theirs { align-self: flex-start",
            ".send-round {",
            "border-radius: 50%",
            ".thread-ear {",
            ".ear-tag {",
            ".ear-clock {",
        ] {
            assert!(css.contains(rule), "kanban.css lost {rule}");
        }
        // The Garden look still reads K2's tokens.
        for token in ["--zen-surface", "--zen-text", "--zen-border", "--zen-accent", "--zen-font-family", "--zen-radius", "--zen-bubble-me"] {
            assert!(css.contains(token), "look = garden ignores {token}");
        }
        // No chamfers or square Style: K2 Companion is round.
        assert!(!css.contains("clip-path") && !css.contains("chamfer"), "no chamfered corners");
    }

    /// k2:kanban@3 (Rosson 2026-10-20, for 0.46.3): @2's widget with only
    /// the Thread panel's scrolling changed. A message arriving while the
    /// reader is scrolled up leaves them where they are (the list is patched,
    /// never rebuilt: WebKit put @2's rebuilt list back at the top) and a
    /// "new messages" pill counts it; at the bottom they stay pinned (K2's
    /// stick-to-bottom). @3 still resolves for own copies (the bare
    /// `k2:kanban` is @4 since Thread parity).
    #[test]
    fn the_kanban_3_keeps_a_scrolled_up_reader_in_place() {
        let w = builtin_widget(&parse_widget_ref("k2:kanban@3").expect("parses")).expect("k2:kanban@3 ships");
        clean_kanban(w);
        let two = builtin_widget(&parse_widget_ref("k2:kanban@2").expect("parses")).expect("k2:kanban@2 ships");
        // Everything but the panel's scrolling is @2's, byte for byte.
        for f in ["manifest.json", "index.html", "md.js", "kanban.js", "picker.js"] {
            assert_eq!(file(w, f), file(two, f), "{f} is @2's");
        }
        let js = file(w, "dock.js");
        assert!(!js.contains("bodyEl.replaceChildren(list)"), "dock.js rebuilds the whole list on every message");
        for needle in [
            "var STICK_PX = 32",
            "function pin(smooth)",
            "function anchorNow()",
            "keepAnchor(anchor)",
            "listEl.replaceChild(node, had.node)",
            "new ResizeObserver(sync)",
            "'dock-newer'",
            "stickNext = true",
            "known = seenNow",
        ] {
            assert!(js.contains(needle), "dock.js lost {needle}");
        }
        let css = file(w, "kanban.css");
        assert!(css.contains(".dock-newer {"), "kanban.css has no new-messages pill");
        assert!(css.contains(".dock-foot { position: relative;"), "the pill sits on the foot");
        // The rest of the look is @2's: @3's CSS is @2's plus the pill.
        let base = file(two, "kanban.css");
        for line in base.lines().filter(|l| !l.starts_with(".dock-foot") && !l.starts_with("/* Kanban (k2:kanban@2)")) {
            assert!(css.contains(line), "kanban.css lost @2's {line}");
        }
    }

    /// k2:kanban@4 (Rosson 2026-10-20, Thread parity for 0.46.3): "app
    /// gateway and garden gateway both need the attachments and any other
    /// systems to match parity with the new thread features." @3's widget
    /// with the Thread panel's files and links: chips with their state (a
    /// hidden file has no name), images drawn only through an <img> with a
    /// data: URL from K2's thumbnail, workspace file links, picked files
    /// riding a post, mark-read, a delta catch-up and paging back, each
    /// behind `k2.can`. @4 still resolves for own copies (the bare
    /// `k2:kanban` is @5 since 0.46.4).
    #[test]
    fn the_kanban_4_has_the_threads_files_and_links() {
        let w = builtin_widget(&parse_widget_ref("k2:kanban@4").expect("parses")).expect("k2:kanban@4 ships");
        clean_kanban(w);
        let three = builtin_widget(&parse_widget_ref("k2:kanban@3").expect("parses")).expect("k2:kanban@3 ships");
        // The board, the manifest (no new cap) and the page are @3's (@2's).
        for f in ["manifest.json", "index.html", "md.js", "kanban.js", "picker.js"] {
            assert_eq!(file(w, f), file(three, f), "{f} is @3's");
        }
        let js = file(w, "dock.js");
        for needle in [
            // Files: chips, the hidden placeholder, pictures via data: URLs.
            "k2.thread.attachment(address, id, n, { mode: 'thumb', px: 512 })",
            "'A file (not shared with you)'",
            "'data:' + mime + ';base64,' + b64",
            "IMAGE_MIME.test(mime)",
            "el('img')",
            "k2.thread.open(current.address, target)",
            // Links, the picker, read state, catch-up, paging back.
            "k2.thread.links(address, batch)",
            "k2.thread.pickFiles(to)",
            "{ attachments: files.map(",
            "k2.thread.markRead(current.address, { seq: seq })",
            "{ afterSeq: after }",
            "{ beforeSeq: first }",
            // Every new verb asks first (an older K2 gets @3's panel).
            "k2.can('thread.attachment')",
            "k2.can('thread.links')",
            "k2.can('thread.pickFiles')",
            "k2.can('thread.markRead')",
            "k2.can('thread.read')",
            // @3's scrolling stays.
            "var STICK_PX = 32",
            "keepAnchor(anchor)",
            "known = seenNow",
        ] {
            assert!(js.contains(needle), "dock.js lost {needle}");
        }
        // Pictures never go in as markup or a blob from the frame's own code.
        for bad in ["createObjectURL", "DOMParser", ".srcdoc", "image/svg+xml' +", "setAttribute('src'"] {
            assert!(!js.contains(bad), "dock.js uses {bad}");
        }
        // The caches can't grow without end.
        for bound in ["while (thumbKeys.length > THUMBS_MAX)", "if (++linkCount > 2000)"] {
            assert!(js.contains(bound), "dock.js lost {bound}");
        }
        let css = file(w, "kanban.css");
        for rule in [".msg-files {", ".file-chip {", ".file-thumb img {", ".file-link {", ".compose-files {", ".attach-round {"] {
            assert!(css.contains(rule), "kanban.css has no {rule}");
        }
        // The rest of the look is @3's (its header comment aside).
        for line in file(three, "kanban.css").lines().skip(2) {
            assert!(css.contains(line), "kanban.css lost @3's {line}");
        }
    }

    /// k2:kanban@5 (Rosson 2026-10-24, for 0.46.4: "the Kanban should get
    /// the same update"): the board alone. The Thread is K2's built-in
    /// Conversation in the page's "Thread" column, so no Thread is redrawn
    /// here: no dock.js, no picker.js, no bubbles, no message box, and no
    /// thread:read. thread.js folds that column (`k2.layout.toggle`) and
    /// Send board posts to the agent the Conversation's header shows (the
    /// `selected` row they share). The board and its Companion look are
    /// @4's. The bare `k2:kanban` is @5.
    #[test]
    fn the_kanban_5_is_the_board_beside_k2s_own_thread() {
        assert_eq!(KANBAN_WIDGET_LATEST, "k2:kanban@5");
        assert_eq!(builtin_widget(&parse_widget_ref("k2:kanban").expect("parses")).map(|w| w.id()).as_deref(), Some(KANBAN_WIDGET_LATEST));
        let w = builtin_widget(&parse_widget_ref(KANBAN_WIDGET_LATEST).expect("parses")).expect("k2:kanban@5 ships");
        clean_kanban(w);
        let four = builtin_widget(&parse_widget_ref("k2:kanban@4").expect("parses")).expect("k2:kanban@4 still ships");
        // The board is @4's (@2's), byte for byte.
        for f in ["md.js", "kanban.js"] {
            assert_eq!(file(w, f), file(four, f), "{f} is @4's");
        }
        let names: Vec<&str> = w.files.iter().map(|(n, _)| *n).collect();
        assert_eq!(names, ["manifest.json", "index.html", "kanban.css", "md.js", "kanban.js", "thread.js"], "no dock.js or picker.js");
        // No redrawn Thread anywhere.
        let html = file(w, "index.html");
        for gone in ["threadDock", "dockBody", "dockPost", "send-round", "compose-box", "thread-ear", "picker.js", "dock.js"] {
            assert!(!html.contains(gone), "index.html still has {gone}");
        }
        for needle in [
            "id=\"btnThread\" type=\"button\" aria-pressed=\"false\"",
            "id=\"btnSend\" type=\"button\"",
            ">Send board</button>",
            "<script src=\"thread.js\"></script>",
        ] {
            assert!(html.contains(needle), "index.html lost {needle}");
        }
        let css = file(w, "kanban.css");
        for gone in [".thread-dock", ".bubble", ".dock-", ".ap-", ".thread-ear", ".file-chip", "--kb-bubble"] {
            assert!(!css.contains(gone), "kanban.css still styles the old panel ({gone})");
        }
        // The board's look is @4's: Companion light and dark, the Garden look.
        for line in file(four, "kanban.css").lines().skip(12).take_while(|l| !l.starts_with("/* ── Thread panel")) {
            if line.contains("--kb-dock") || line.contains("--kb-field") || line.contains("--kb-ear") || line.contains("--kb-bubble") {
                continue;
            }
            let line = line.replace("in card notes and Thread messages.", "in card notes.");
            assert!(css.contains(&line), "kanban.css lost @4's {line}");
        }
        let js = file(w, "thread.js");
        for needle in [
            "c.collapsible && c.label === 'Thread'",
            "k2.layout.toggle(column)",
            "k2.layout.subscribe(drawLayout)",
            "rows.find(function (r) { return r.selected })",
            "k2.thread.post(to.address, board.exportMarkdown())",
            "'Sent the board to ' + to.label",
            "k2.agents.subscribe(onRows",
            "k2.conversation.open(want)",
            "k2.save.remove(saveKey())",
            "k2.config.look === 'garden'",
            "if (!carry || shown) return",
        ] {
            assert!(js.contains(needle), "thread.js lost {needle}");
        }
        for gone in ["k2.thread.subscribe", "k2.thread.answer", "k2.thread.read", "bubble", "textContent = doc"] {
            assert!(!js.contains(gone), "thread.js redraws the Thread ({gone})");
        }
        let m: serde_json::Value = serde_json::from_str(file(w, "manifest.json")).expect("manifest is JSON");
        assert_eq!(m["caps"], serde_json::json!(["agents:read", "thread:post", "store:read", "store:write"]), "no thread:read: K2's Conversation reads the Thread");
    }

    /// What every Kanban version keeps: a clean custom widget that keeps its
    /// board in Garden data, renders notes with the bundled marked +
    /// DOMPurify, drags with pointer events, and ships none of the Roadmap
    /// board's K2-internal cards or hints.
    fn clean_kanban(w: &'static BuiltinWidget) {
        let id = w.id();
        let files: std::collections::BTreeMap<String, Vec<u8>> =
            w.files.iter().map(|(n, b)| (n.to_string(), b.as_bytes().to_vec())).collect();
        let c = crate::zen::widgets::check_files("k2", &format!("kanban-{}", w.version), &files);
        assert!(c.errors.is_empty(), "{id}: {:?}", c.errors);
        assert!(c.bundle.is_some(), "{id} bundles");
        let m = c.manifest.as_ref().expect("a manifest");
        // @5 reads no Thread itself: K2's Conversation beside it does.
        if w.version >= 5 {
            assert_eq!(m.caps, ["agents:read", "thread:post", "store:read", "store:write"]);
        } else {
            assert_eq!(m.caps, ["agents:read", "thread:read", "thread:post", "store:read", "store:write"]);
        }
        for cap in &m.caps {
            assert!(crate::zen::USER_WIDGET_CAPS.contains(&cap.as_str()), "{cap}");
        }
        assert!(!m.caps.iter().any(|c| c == "sources:read"), "reading sources would drop store:write");
        let manifest: serde_json::Value = serde_json::from_str(file(w, "manifest.json")).expect("manifest is JSON");
        assert_eq!(manifest["requires"]["libs"], serde_json::json!(["marked@18", "dompurify@3"]));
        let html = file(w, "index.html");
        for bad in [" onclick=", " onload=", " onerror=", "<iframe", "<object", "<embed", "<base", "http-equiv", "src=\"http", "href=\"http", "//cdn"] {
            assert!(!html.contains(bad), "index.html has {bad}");
        }
        // Every script it ships is loaded (@1–@4: picker.js + dock.js; @5: thread.js).
        let scripts: Vec<&str> = w.files.iter().map(|(n, _)| *n).filter(|n| n.ends_with(".js")).collect();
        assert!(scripts.starts_with(&["md.js", "kanban.js"]), "{id}: {scripts:?}");
        for s in &scripts {
            assert!(html.contains(&format!("<script src=\"{s}\"></script>")), "index.html loads {s}");
        }
        // Widget verbs, and the Garden data verbs widgets reach through
        // this computer's daemon (`garden-local`).
        let cat = crate::contract::catalog();
        let widget: Vec<String> = cat
            .verbs_exposed_to(crate::contract::Exposure::Widget)
            .iter()
            .chain(cat.verbs_exposed_to(crate::contract::Exposure::GardenLocal).iter())
            .map(|v| v.verb.clone())
            .collect();
        for name in scripts {
            let js = file(w, name);
            for bad in ["innerHTML", "outerHTML", "insertAdjacentHTML", "document.write", "fetch(", "XMLHttpRequest", "WebSocket", "EventSource", "localStorage", "sessionStorage", "indexedDB", "eval(", "new Function", "import "] {
                assert!(!js.contains(bad), "{name} uses {bad}");
            }
            // Only widget helpers the catalog exposes (plus the runtime's
            // own members and the SDK's store/save/thread helpers).
            for (i, _) in js.match_indices("k2.") {
                let call: String = js[i + 3..].chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '.').collect();
                let call = call.trim_end_matches('.');
                if call.is_empty() {
                    continue;
                }
                // `k2.widget.name.trim`, `k2.config.agent.indexOf`, `k2.connected.then`:
                // a runtime property, then plain JavaScript on its value.
                let head = call.split('.').next().unwrap_or(call);
                let ok = widget.iter().any(|v| v == call)
                    || crate::contract::gen::RUNTIME_MEMBERS.contains(&call)
                    || (["widget", "config", "connected", "motion"].contains(&head)
                        && crate::contract::gen::RUNTIME_MEMBERS.contains(&head))
                    || ["store", "store.subscribe", "store.tx", "save", "viewer", "thread", "agents", "canvas"].contains(&call);
                assert!(ok, "{name} calls k2.{call}, which isn't a widget helper");
            }
            // Generic: none of the Roadmap board's internal cards or hints.
            for bad in ["Rosson", "Roadmap", "k2::local", "Parked", "Owned elsewhere", "ranking", "0.45.4", "0.46", "K2_ADDRESS"] {
                assert!(!js.contains(bad), "{name} still has '{bad}'");
            }
        }
        let js = file(w, "kanban.js");
        // The default board: To do / Doing / Done, in Garden data, once.
        for needle in ["{ id: 'todo', name: 'To do' }", "{ id: 'doing', name: 'Doing' }", "{ id: 'done', name: 'Done' }", "metaKey()", "ifRev: 0", "k2.store.batch(", "k2.store.subscribe(prefix"] {
            assert!(js.contains(needle), "kanban.js lost {needle}");
        }
        // Pointer drag (mouse, pen, touch), never HTML5 drag and drop.
        for needle in ["'pointerdown'", "'pointermove'", "'pointerup'", "'pointercancel'", "'Escape'", "setPointerCapture", "drag-ghost"] {
            assert!(js.contains(needle), "kanban.js lost {needle}");
        }
        assert!(!js.contains("dataTransfer") && !js.contains("draggable"), "no HTML5 drag and drop");
        let css = file(w, "kanban.css");
        assert!(css.contains("@media (prefers-reduced-motion: reduce)"));
        let code: usize = w.files.iter().filter(|(f, _)| *f != "manifest.json").map(|(_, b)| b.len()).sum();
        assert!(code <= crate::zen::bundle::WARN_CODE_BYTES, "WS8: {id} is {code} bytes of code");
    }

    /// Garden v3 (contract-sources-v1 §11.4, §12.3): each data-source card
    /// is a clean custom widget that reads sources from its placement config
    /// (never posts), bundles with no findings, and pairs with presets K2 has.
    #[test]
    fn source_cards_are_clean_and_pair_with_presets() {
        let ids: Vec<String> = SOURCE_CARDS.iter().map(|(id, _)| id.to_string()).collect();
        assert_eq!(ids, ["k2:fleet-grid@3", "k2:version-spread@3", "k2:relays-dns@3", "k2:deploys@3", "k2:money-alerts@3"]);
        // Every released version of each card stays clean; @1 and @2 still
        // resolve, and the bare `k2:<card>` is @3 (the newest, 0.46.4).
        let both: Vec<(String, &[&str])> = SOURCE_CARDS
            .iter()
            .flat_map(|(id, presets)| {
                [(id.replace("@3", "@1"), *presets), (id.replace("@3", "@2"), *presets), (id.to_string(), *presets)]
            })
            .collect();
        for (id, _) in SOURCE_CARDS {
            let bare = id.trim_end_matches("@3");
            assert_eq!(builtin_widget(&parse_widget_ref(bare).expect("parses")).map(|w| w.id()).as_deref(), Some(*id), "{bare} is @3");
        }
        for (id, presets) in &both {
            let id = id.as_str();
            let w = builtin_widget(&parse_widget_ref(id).expect("parses")).unwrap_or_else(|| panic!("{id} is a built-in"));
            let files: std::collections::BTreeMap<String, Vec<u8>> =
                w.files.iter().map(|(n, b)| (n.to_string(), b.as_bytes().to_vec())).collect();
            let c = crate::zen::widgets::check_files("k2", &format!("{}-{}", w.name, w.version), &files);
            assert!(c.errors.is_empty(), "{id}: {:?}", c.errors);
            let m = c.manifest.as_ref().unwrap_or_else(|| panic!("{id} has a manifest"));
            assert_eq!(m.caps, ["sources:read"], "{id}: reads sources, never posts");
            assert_eq!(m.sources, Some(serde_json::json!("config")), "{id}");
            assert!(c.bundle.is_some(), "{id} bundles");
            for f in ["kit.js", "kit.css", "card.js"] {
                assert!(w.files.iter().any(|(n, _)| *n == f), "{id} has {f}");
            }
            let js = file(w, "card.js");
            assert!(!js.contains("innerHTML") && !js.contains("thread.") && !js.contains("compose."), "{id}: textContent only, no Thread");
            for p in presets.iter() {
                assert!(
                    crate::sources::presets_cards::NAMES.contains(p) || crate::sources::presets::BASE.contains(p),
                    "{id} pairs with unknown preset {p}"
                );
            }
        }
        for kit in [include_str!("sources-kit/kit.js"), include_str!("sources-kit-2/kit.js"), include_str!("sources-kit-3/kit.js")] {
            assert!(!kit.contains("innerHTML") && !kit.contains("fetch(") && !kit.contains("localStorage"), "the kit is sealed-frame clean");
        }
    }

    /// 0.46.4 (Rosson 2026-10-23: "not useful"): the @3 cards draw no "Ask an
    /// agent" button and never call k2.sources.ask (it answers `retired`).
    /// Apart from that, @3 is @2: the same manifest, page, style and kit.
    #[test]
    fn source_cards_at_3_have_no_ask_an_agent() {
        let kit3 = include_str!("sources-kit-3/kit.js");
        let kit2 = include_str!("sources-kit-2/kit.js");
        // Code lines only: the comments may still name what was removed.
        let code = |k: &str| -> Vec<String> {
            k.lines()
                .filter(|l| {
                    let t = l.trim_start();
                    !t.starts_with("//") && !t.starts_with('*') && !t.starts_with("/*")
                })
                .map(str::to_string)
                .collect()
        };
        for (id, _) in SOURCE_CARDS {
            let w = builtin_widget(&parse_widget_ref(id).expect("parses")).unwrap_or_else(|| panic!("{id} ships"));
            assert_eq!(w.version, 3, "{id}");
            let two = builtin_widget(&parse_widget_ref(&id.replace("@3", "@2")).expect("parses")).expect("@2 ships");
            for f in ["manifest.json", "index.html", "kit.css"] {
                assert_eq!(file(w, f), file(two, f), "{id}: {f} is @2's");
            }
            assert_eq!(file(w, "kit.js"), kit3, "{id}: kit.js is sources-kit-3");
            for js in [code(file(w, "card.js")).join("\n"), code(kit3).join("\n")] {
                assert!(!js.contains("sources.ask") && !js.contains("ctx.ask") && !js.contains("k2c-ask"), "{id}: no Ask an agent");
                assert!(!js.contains("'Ask an agent'") && !js.contains("'Ask'"), "{id}: no Ask button text");
            }
        }
        // Everything else in the kit is @2's: kit-3 adds no code line, and
        // keeps all of @2's but the ask button and ctx.ask (about 20 lines).
        let three = code(kit3);
        let two = code(kit2);
        assert!(three.iter().all(|l| two.contains(l)), "kit-3 adds no code");
        let dropped = two.iter().filter(|l| !three.contains(l)).count();
        assert!(dropped <= 25, "kit-3 keeps @2's code but the ask lines: {dropped} dropped");
    }

    /// Box usage's Garden card (0.46.4): a clean custom widget with no caps
    /// that reads only the garden-local `app.resources`, writes nothing,
    /// posts nothing, and has no buttons at all (no Ask button: Rosson
    /// 2026-10-23 retired them from K2's cards).
    #[test]
    fn the_box_disk_card_is_read_only() {
        let w = builtin_widget(&parse_widget_ref(BOX_DISK_WIDGET).expect("parses")).expect("k2:box-disk@1 ships");
        let files: std::collections::BTreeMap<String, Vec<u8>> =
            w.files.iter().map(|(n, b)| (n.to_string(), b.as_bytes().to_vec())).collect();
        let c = crate::zen::widgets::check_files("k2", "box-disk-1", &files);
        assert!(c.errors.is_empty(), "{:?}", c.errors);
        assert!(c.bundle.is_some(), "it bundles");
        let m = c.manifest.as_ref().expect("a manifest");
        assert!(m.caps.is_empty(), "no caps: {:?}", m.caps);
        let row = crate::contract::catalog().verb("app.resources").expect("the app.resources row");
        assert!(row.exposed_to(crate::contract::Exposure::GardenLocal) && row.cap.is_none() && row.effect == crate::contract::Effect::Read);
        let html = file(w, "index.html");
        let js = file(w, "card.js");
        for bad in [" onclick=", " onload=", "<button", "<form", "<iframe", "src=\"http", "href=\"http"] {
            assert!(!html.contains(bad), "index.html has {bad}");
        }
        for bad in ["innerHTML", "outerHTML", "insertAdjacentHTML", "document.write", "fetch(", "XMLHttpRequest", "WebSocket", "localStorage",
            "eval(", "new Function", "'button'", "\"button\"", "addEventListener('click'", "Ask", "sources.", "thread.", "store.", "save."]
        {
            assert!(!js.contains(bad), "card.js has {bad}");
        }
        let calls: Vec<String> = js
            .match_indices("k2.")
            .map(|(i, _)| js[i + 3..].chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '.').collect::<String>())
            .map(|s| s.trim_end_matches('.').to_string())
            .filter(|s| !s.is_empty())
            .collect();
        for call in &calls {
            let head = call.split('.').next().unwrap_or(call);
            assert!(
                call == "app.resources" || ["ready", "connected", "config"].contains(&head),
                "card.js calls k2.{call}: only app.resources, ready, connected and config"
            );
        }
        assert!(calls.iter().any(|c| c == "app.resources"), "it reads app.resources");
    }

    /// Integration (Zen v2, B1 × B3): every library a built-in widget asks
    /// for resolves against the checked-in `zen-lib.json`, and is bundled,
    /// so a built-in works offline and under air-gap with no first-use
    /// download. The Diary's `perfect-freehand@1` and `font-caveat@5` are
    /// pinned by name so a rename in the stdlib fails here, not at mount.
    #[test]
    fn builtin_widget_libraries_resolve_in_zen_lib_json() {
        use crate::zen::stdlib::{find, zen_lib_manifest, LibRef, LibSource};
        let m = zen_lib_manifest();
        for w in BUILTIN_WIDGETS {
            let manifest = w
                .files
                .iter()
                .find(|(f, _)| *f == "manifest.json")
                .map(|(_, b)| *b)
                .unwrap_or_else(|| panic!("{} has a manifest.json", w.id()));
            let manifest: serde_json::Value =
                serde_json::from_str(manifest).unwrap_or_else(|e| panic!("{}: manifest.json: {e}", w.id()));
            let Some(libs) = manifest.get("requires").and_then(|r| r.get("libs")) else { continue };
            let libs = libs.as_array().unwrap_or_else(|| panic!("{}: requires.libs is a list", w.id()));
            for l in libs {
                let r = LibRef::from_json(l).unwrap_or_else(|e| panic!("{}: {e}", w.id()));
                let e = find(m, &r).unwrap_or_else(|| panic!("{}: {l} isn't in src/shared/zen-lib.json", w.id()));
                assert_eq!(e.source, LibSource::Bundled, "{}: {l} must be bundled, not a download", w.id());
                assert!(!e.files.is_empty(), "{}: {l} resolves to a row with no files", w.id());
            }
        }
        builtin_widget(&parse_widget_ref(DIARY_WIDGET).expect("parses")).expect("k2:diary@1 ships");
        for (id, v) in [("perfect-freehand", "1"), ("font-caveat", "5"), ("pixi.js", "8"), ("tone", "15")] {
            let r = LibRef::Named { id: id.into(), version: v.into() };
            let e = find(m, &r).unwrap_or_else(|| panic!("the Diary's {id}@{v} isn't in zen-lib.json"));
            assert_eq!(e.source, LibSource::Bundled, "{id}@{v}");
        }
    }
}
