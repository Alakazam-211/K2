//! The ID table (K-ID-13): every ID or address kind a verb takes or prints, as data, with its
//! durability class, its scope and the verbs that accept it.
//!
//! An address is **(scope, id)**: `r17` names a row of one table, `v12` a version of one
//! doc. Each kind has a class ([`IdClass`], P1): durable, file-local, a unique name,
//! positional, session or a change stamp. Anchors bind to the most durable ID an object has;
//! weaker ones are hints for re-matching.
//!
//! - [`KIT`]: the kit's own kinds (text lines and `L:C` positions, grid keys, `#N`, `LN`,
//!   header names, `--where`, versions, changes, participants, claims).
//! - [`crate::session::Pack::ids`]: a pack's own kinds (Markdown's headings, docx's
//!   `w14:paraId`...), in the same form.
//! - [`markdown`] renders a table; [`kit_markdown`] is the kit's `docs/IDS.md` (a test holds
//!   the committed file to it); [`check`] checks a table against a verb registry.
//! - `churn` (feature `test-support`): the ID-churn judge (K-COD-10), a harness a pack's
//!   tests run over its corpus.
//!
//! Plain data, K2-agnostic: no host, route or CLI name appears here.

#[cfg(feature = "test-support")]
pub mod churn;

use std::collections::BTreeSet;
use std::fmt;

/// How long an ID keeps naming the same object (P1). Declared most durable first: an anchor
/// binds to the smallest class an object has ([`IdClass::anchors`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum IdClass {
    /// Survives saves, outside edits and an engine migration; never reused in its scope
    /// (grid keys, versions, docx `w16cid:durableId`).
    Durable,
    /// Unique within one file as written; another producer may renumber it on save, so
    /// across saves it is a hint (docx `w14:paraId`, IDML `Self`).
    FileLocal,
    /// A name or a value: it addresses only while exactly one object has it; two or more
    /// are refused with the candidates, never "the first" (header names, `--where`).
    UniqueName,
    /// An index, a line or an offset: it moves when anything before it changes. Resolve it
    /// against the version that was read (`--if-version`).
    Positional,
    /// Lives only while the session is in memory; never stored (claim ids).
    Session,
    /// A change stamp: it says that something changed, and never addresses an object
    /// (docx `w14:textId`). Accepted by no verb.
    Stamp,
}

impl IdClass {
    /// Every class, most durable first.
    pub const ALL: &'static [IdClass] = &[
        IdClass::Durable,
        IdClass::FileLocal,
        IdClass::UniqueName,
        IdClass::Positional,
        IdClass::Session,
        IdClass::Stamp,
    ];

    /// The class's name in `docs/IDS.md`.
    pub fn word(self) -> &'static str {
        match self {
            IdClass::Durable => "durable",
            IdClass::FileLocal => "file-local",
            IdClass::UniqueName => "unique-name",
            IdClass::Positional => "positional",
            IdClass::Session => "session",
            IdClass::Stamp => "stamp",
        }
    }

    /// What the class promises, one sentence.
    pub fn meaning(self) -> &'static str {
        match self {
            IdClass::Durable => {
                "Survives saves, outside edits and an engine migration; never reused in its scope."
            }
            IdClass::FileLocal => {
                "Unique within one file as written; another producer may renumber it on save, \
                 so across saves it is a hint."
            }
            IdClass::UniqueName => {
                "A name or a value: it addresses only while exactly one object has it; two or \
                 more are refused with the candidates, never \"the first\"."
            }
            IdClass::Positional => {
                "An index, a line or an offset: it moves when anything before it changes. \
                 Resolve it against the version you read (`--if-version`)."
            }
            IdClass::Session => {
                "Lives only while the session is in memory; never stored, never an anchor."
            }
            IdClass::Stamp => {
                "A change stamp: it says that something changed and never addresses an object; \
                 no verb accepts it."
            }
        }
    }

    /// Whether an anchor may bind to it (a session ID or a stamp never holds one).
    pub fn anchors(self) -> bool {
        !matches!(self, IdClass::Session | IdClass::Stamp)
    }
}

impl fmt::Display for IdClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.word())
    }
}

/// One ID or address kind: a row of an ID table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdKind {
    /// The kind's name: lowercase words joined by `-` (`row-key`, `para-id`). Unique in its
    /// table.
    pub kind: &'static str,
    /// How a caller writes or sees one (`r17`, `--lines 3,7`).
    pub example: &'static str,
    pub class: IdClass,
    /// What it is unique in (`a grid doc's rows`): the address is (scope, id).
    pub scope: &'static str,
    /// The verbs that accept it, by their registry names ([`crate::parity::VerbSpec::name`],
    /// a pack verb's [`crate::tools::PackVerb::name`]). Empty for a kind a pack only reads.
    pub accepted_by: &'static [&'static str],
    /// One line: how it is minted, printed or refused.
    pub note: &'static str,
}

// ---------- the kit's own kinds ----------

/// The kit's text edit verbs that take `--if-version`.
const TEXT_EDITS: [&str; 7] = [
    "insert", "replace", "delete", "sed -i", "patch", "apply", "append",
];

/// The grid verbs that name a row with `--row R`, `R,C` or a place (`--after R`).
const ROW_VERBS: [&str; 8] = [
    "cell get",
    "cell set",
    "cells set",
    "cells clear",
    "row insert",
    "row move",
    "row delete",
    "row restore",
];

/// The grid verbs that name a column with `--col C`, `R,C`, `--cols`, a place or `--by`.
const COL_VERBS: [&str; 11] = [
    "cell get",
    "cell set",
    "cells set",
    "cells clear",
    "row list",
    "col insert",
    "col move",
    "col delete",
    "col restore",
    "col rename",
    "sort",
];

const fn cat<const A: usize, const B: usize, const N: usize>(
    a: [&'static str; A],
    b: [&'static str; B],
) -> [&'static str; N] {
    assert!(A + B == N, "cat: N must be A + B");
    let mut out = [""; N];
    let mut i = 0;
    while i < A {
        out[i] = a[i];
        i += 1;
    }
    let mut j = 0;
    while j < B {
        out[A + j] = b[j];
        j += 1;
    }
    out
}

const ROW_KEY_VERBS: [&str; 11] = cat(ROW_VERBS, ["claim", "cursor", "blame --cell"]);
const COL_KEY_VERBS: [&str; 14] = cat(COL_VERBS, ["claim", "cursor", "blame --cell"]);
const WHERE_VERBS: [&str; 4] = ["cell get", "cell set", "row move", "row delete"];
/// `--where COL=VALUE` names its column in these too (`cell get` / `cell set` are in both).
const COL_NAME_VERBS: [&str; 13] = cat(COL_VERBS, ["row move", "row delete"]);
/// The grid verbs that change the doc (each takes `--if-version`).
const GRID_EDITS: [&str; 14] = [
    "cell set",
    "cells set",
    "cells clear",
    "row insert",
    "row move",
    "row delete",
    "row restore",
    "col insert",
    "col move",
    "col delete",
    "col restore",
    "col rename",
    "sort",
    "header",
];
const VERSION_VERBS: [&str; 27] = cat(
    cat::<7, 14, 21>(TEXT_EDITS, GRID_EDITS),
    [
        "write",
        "diff --version",
        "show --version",
        "export --version",
        "restore",
        "version --name",
    ],
);

/// Every ID and address kind the kit's verbs take or print (`docs/IDS.md`). Sources:
/// `tools/mod.rs` (the text verbs' flags), `table/verbs.rs` (grid addressing, §1.7),
/// `elem.rs` (keys), `session/position.rs` (`line:col`), `parity/registry.rs` (the verbs).
pub const KIT: &[IdKind] = &[
    IdKind {
        kind: "line",
        example: "--lines 3,7",
        class: IdClass::Positional,
        scope: "a doc's lines (a grid: its rendered file)",
        accepted_by: &[
            "view", "insert", "replace", "delete", "sed", "sed -i", "claim", "blame",
        ],
        note: "1-based: `--lines a,b`, `--line N`, `--after N`, `-L a,b`, sed's own addresses; \
               moves when a line above it is added or removed",
    },
    IdKind {
        kind: "line-col",
        example: "--at 12:5",
        class: IdClass::Positional,
        scope: "a text root's text",
        accepted_by: &["insert", "replace", "delete", "cursor"],
        note: "`L:C` or `L:C-L:C`: 1-based lines, columns in Unicode scalar values (UTF-8 \
               bytes inside the kit); positions the kit stores move with every change",
    },
    IdKind {
        kind: "exact-text",
        example: "--old 'TODO: x'",
        class: IdClass::UniqueName,
        scope: "a doc's text",
        accepted_by: &["replace"],
        note: "must match exactly once (unless `--all`); none or several apply nothing and \
               exit 1 with each match's lines",
    },
    IdKind {
        kind: "row-key",
        example: "@k:r17 | r17 | key:r17",
        class: IdClass::Durable,
        scope: "a grid doc's rows",
        accepted_by: &ROW_KEY_VERBS,
        note: "minted once, never reused; follows moves and sorts; kept through outside saves \
               (the aligner re-matches rows, and a tie never pairs) and an engine migration; \
               printed by `row insert` and `row list`; a deleted row is named by key only; `@k:` is \
               the canonical spelling, `key:` the same, and a bare word is the key",
    },
    IdKind {
        kind: "col-key",
        example: "@k:c3 | c3 | key:c3",
        class: IdClass::Durable,
        scope: "a grid doc's columns",
        accepted_by: &COL_KEY_VERBS,
        note: "as a row key; a bare word that is a known column key is the key, `@k:` (canonical) \
               or `key:` forces it; printed by `col insert` and `col list`",
    },
    IdKind {
        kind: "row-nth",
        example: "#3",
        class: IdClass::Positional,
        scope: "a grid doc's live rows, in order",
        accepted_by: &ROW_VERBS,
        note: "the Nth live row (`--row #N`, `--at #N`, `--to #N`); past the end exits 4",
    },
    IdKind {
        kind: "col-nth",
        example: "#2",
        class: IdClass::Positional,
        scope: "a grid doc's live columns, in order",
        accepted_by: &COL_VERBS,
        note: "the Nth live column; past the end exits 4",
    },
    IdKind {
        kind: "rendered-line",
        example: "L5",
        class: IdClass::Positional,
        scope: "a grid doc's rendered file",
        accepted_by: &ROW_VERBS,
        note: "the row rendered line N belongs to; the header's or a preamble's line exits 4",
    },
    IdKind {
        kind: "col-name",
        example: "email | name:email",
        class: IdClass::UniqueName,
        scope: "a grid doc's header names (header on)",
        accepted_by: &COL_NAME_VERBS,
        note: "a bare word that is no column key, or `name:`; two live columns with the name \
               exit 2 listing their keys; with the header off, exit 4",
    },
    IdKind {
        kind: "where",
        example: "--where sku=A7",
        class: IdClass::UniqueName,
        scope: "a grid doc's live rows, by one column's value",
        accepted_by: &WHERE_VERBS,
        note: "the one live row whose cell is the value; several exit 2 listing their keys, \
               none exit 4 (`row list --where` filters instead)",
    },
    IdKind {
        kind: "version",
        example: "v1842 | 1842",
        class: IdClass::Durable,
        scope: "a doc's history",
        accepted_by: &VERSION_VERBS,
        note: "a log position, printed by every read; `--if-version`, `--base`, `--version`, \
               `--to`; retention may drop an unnamed stored version (a base then exits 8)",
    },
    IdKind {
        kind: "change",
        example: "revert 41",
        class: IdClass::Durable,
        scope: "a doc's change log",
        accepted_by: &["revert", "approve", "decline"],
        note: "a change's log position, as `log` prints it (a blob doc: its version)",
    },
    IdKind {
        kind: "participant",
        example: "--by p2",
        class: IdClass::Durable,
        scope: "the host's participants",
        accepted_by: &[
            "follow",
            "undo",
            "undo --partial",
            "log",
            "log --pending",
            "approvals",
        ],
        note: "host-given (the kit never mints or renames one): `follow <pid>`, `--for`, \
               `--by`, `--on-behalf-of`",
    },
    IdKind {
        kind: "claim-id",
        example: "claim-12 | 12",
        class: IdClass::Session,
        scope: "the process",
        accepted_by: &["claim --renew", "claim --release"],
        note: "printed by `claim`; unique within the process, so another doc's or an ended \
               claim's id is simply not live",
    },
    IdKind {
        kind: "annotation",
        example: "ann-12 | 12",
        class: IdClass::Durable,
        scope: "a doc's comment threads",
        accepted_by: &[
            "reply",
            "resolve",
            "resolve --reopen",
            "comment --delete",
            "reanchor",
            "comments --log",
        ],
        note: "printed by `comment` and `comments`; per doc, never reused (a purged thread keeps \
               its id); a comment within it is `--n N`, 0 the first",
    },
];

// ---------- rendering and checks ----------

fn code_list(xs: &[&str]) -> String {
    if xs.is_empty() {
        return "none".to_owned();
    }
    xs.iter()
        .map(|x| format!("`{x}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// An example cell: each `|`-separated form in code (`|` would end a table cell).
fn example_cell(e: &str) -> String {
    e.split(" | ")
        .map(|x| format!("`{x}`"))
        .collect::<Vec<_>>()
        .join(" or ")
}

/// An ID table as Markdown: one row per kind (kind, example, class, scope, verbs, note).
/// What a pack's `docs/IDS.md` is generated from, under its own prose.
pub fn markdown(rows: &[IdKind]) -> String {
    let mut o = String::from(
        "| Kind | Example | Class | Scope | Accepted by | Note |\n|---|---|---|---|---|---|\n",
    );
    for r in rows {
        o.push_str(&format!(
            "| `{}` | {} | {} | {} | {} | {} |\n",
            r.kind,
            example_cell(r.example),
            r.class.word(),
            r.scope,
            code_list(r.accepted_by),
            r.note
        ));
    }
    o
}

/// The classes as a Markdown table (P1), most durable first.
pub fn classes_markdown() -> String {
    let mut o = String::from("| Class | Meaning |\n|---|---|\n");
    for c in IdClass::ALL {
        o.push_str(&format!("| {} | {} |\n", c.word(), c.meaning()));
    }
    o
}

/// The kit's `docs/IDS.md`, generated (a test holds the committed file to this).
pub fn kit_markdown() -> String {
    let mut o = String::from(
        "# IDs and addresses\n\n\
         <!-- Generated from crates/base/src/ids/mod.rs by `ids::kit_markdown()`; don't edit \
         by hand. Regenerate: MPC_BLESS=1 cargo test -p k2-mpc-base --test ids. -->\n\n\
         Every ID or address kind the kit's verbs take or print, as data (`ids::KIT`, \
         K-ID-13). An address is **(scope, id)**: an ID means something only inside its \
         scope (`r17` is a row of one table, `v12` a version of one doc).\n\n\
         ## Classes\n\n\
         Each kind has one class (P1), listed most durable first:\n\n",
    );
    o.push_str(&classes_markdown());
    o.push_str(
        "\nAnchors bind to the most durable ID an object has; weaker ones are hints for \
         re-matching (its native ID, then kind, name and parent path, then position), and a \
         weak or failed match changes the anchor's state, never where it points. A cell \
         `R,C` is a row address and a column address, as durable as the weaker of the two.\n\n\
         ## The kit's kinds\n\n",
    );
    o.push_str(&markdown(KIT));
    o.push_str(
        "\n## Packs\n\n\
         A pack declares the kinds its own codec and verbs read (`Pack::ids`, default none) \
         in the same form, and generates its own `docs/IDS.md` from them with `ids::markdown`; \
         `ids::check` holds the rows to its verb registry. A pack lists only its own kinds: \
         the kit's apply to every pack of their tier.\n\n\
         **The ID-churn judge** (K-COD-10; `ids::churn::judge`, feature `test-support`): over \
         a pack's corpus, a no-op save (decode, encode, read back) must give the file's bytes \
         back and keep every ID, and each one-unit edit (a verb's edits written back, or an \
         outside save read back) must keep the ID of every object it doesn't touch. It is a \
         test harness: a write that would renumber an ID is not refused at run time yet.\n",
    );
    o
}

/// Why an ID table is wrong: one line per problem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableProblems(pub Vec<String>);

impl fmt::Display for TableProblems {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ID table: {}", self.0.join("; "))
    }
}

impl std::error::Error for TableProblems {}

/// Check an ID table: each kind named once in lowercase words joined by `-`; an example, a
/// scope and a note; every verb in `accepted_by` known to `verb_exists` (a registry's names)
/// and listed once; a stamp accepted by no verb. A pack's test runs it with
/// `|v| Registry::for_pack(&pack)?.get(v).is_some()`.
pub fn check(rows: &[IdKind], verb_exists: impl Fn(&str) -> bool) -> Result<(), TableProblems> {
    let mut bad = Vec::new();
    let mut kinds = BTreeSet::new();
    for r in rows {
        let k = r.kind;
        let word_ok = |w: &str| {
            !w.is_empty()
                && w.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        };
        if !k.split('-').all(word_ok) {
            bad.push(format!("kind {k:?}: lowercase words joined by '-'"));
        }
        if !kinds.insert(k) {
            bad.push(format!("kind {k:?} is listed twice"));
        }
        for (what, v) in [("example", r.example), ("scope", r.scope), ("note", r.note)] {
            if v.trim().is_empty() {
                bad.push(format!("kind {k:?}: no {what}"));
            }
        }
        let mut verbs = BTreeSet::new();
        for v in r.accepted_by {
            if !verbs.insert(*v) {
                bad.push(format!("kind {k:?}: verb {v:?} is listed twice"));
            }
            if !verb_exists(v) {
                bad.push(format!("kind {k:?}: verb {v:?} is not in the registry"));
            }
        }
        if r.class == IdClass::Stamp && !r.accepted_by.is_empty() {
            bad.push(format!(
                "kind {k:?}: a stamp addresses nothing, so no verb accepts it"
            ));
        }
    }
    if bad.is_empty() {
        Ok(())
    } else {
        Err(TableProblems(bad))
    }
}
