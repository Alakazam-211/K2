//! The coverage matrix: every rule, the capabilities it needs, and its status on each
//! engine.
//!
//! - A rule runs on an engine when the engine has every capability it needs
//!   ([`Engine::caps`]). Its status there is **pass**, or **deviation** when it is in that
//!   engine's `KNOWN_DEVIATIONS` (the rule then asserts the deviation still happens).
//! - Where it doesn't run, the cell must say why: **awaiting** ([`AWAITING`], a reason and a
//!   target) or **never** ([`NEVER`], a reason). A silent gap is a failure.
//! - **A rule that runs on no engine fails the suite unless it is listed in [`AWAITING`]**
//!   ([`Matrix::problems`]), so "every (new) rule exists" ([`REQUIRED`]) is checked, not
//!   claimed.
//!
//! The suite's tests check that each engine's tests run exactly its capabilities' rules,
//! print the matrix ([`Matrix::render`]) and keep `docs/CONFORMANCE.md` equal to it.

use crate::model::ModelText;
use crate::yrs::YrsText;
use crate::{Deviation, TextReplica};

use Cap::*;

/// What a rule needs from an engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Cap {
    /// [`TextReplica`]: every engine.
    Text,
    /// [`crate::caps::Events`].
    Events,
    /// [`crate::caps::Blame`].
    Blame,
    /// [`crate::caps::Anchors`].
    Anchors,
    /// [`crate::caps::Undo`] (doc level).
    Undo,
    /// [`crate::caps::Versions`] (doc level).
    Versions,
    /// [`crate::caps::HistoryPurge`] (doc level).
    HistoryPurge,
    /// [`crate::caps::Erasure`] (doc level): the kit's mapping from agent IDs to people.
    Erasure,
    /// [`crate::caps::Branches`].
    Branches,
    /// [`crate::caps::BranchAt`].
    BranchAt,
    /// [`crate::caps::Purge`].
    Purge,
    /// [`crate::caps::BlameAt`].
    BlameAt,
    /// Decoding untrusted update bytes: the native decoder's (G2).
    Wire,
    /// The element IDs a span of the live text covers, tombstones between included.
    SpanIds,
}

impl Cap {
    /// Every capability.
    pub const ALL: [Cap; 14] = [
        Text,
        Events,
        Blame,
        Anchors,
        Undo,
        Versions,
        HistoryPurge,
        Erasure,
        Branches,
        BranchAt,
        Purge,
        BlameAt,
        Wire,
        SpanIds,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Cap::Text => "Text",
            Cap::Events => "Events",
            Cap::Blame => "Blame",
            Cap::Anchors => "Anchors",
            Cap::Undo => "Undo",
            Cap::Versions => "Versions",
            Cap::HistoryPurge => "HistoryPurge",
            Cap::Erasure => "Erasure",
            Cap::Branches => "Branches",
            Cap::BranchAt => "BranchAt",
            Cap::Purge => "Purge",
            Cap::BlameAt => "BlameAt",
            Cap::Wire => "Wire",
            Cap::SpanIds => "SpanIds",
        }
    }

    /// The rules this capability's module implements, or `None` for a capability that is
    /// only declared so far.
    pub fn implemented(self) -> Option<&'static [&'static str]> {
        match self {
            Cap::Text => Some(crate::text::RULES),
            Cap::Events => Some(crate::events::RULES),
            Cap::Blame => Some(crate::blame::RULES),
            Cap::Anchors => Some(crate::anchors::RULES),
            Cap::Undo => Some(crate::undo::RULES),
            Cap::Versions => Some(crate::versions::RULES),
            Cap::HistoryPurge => Some(crate::erasure::HISTORY_RULES),
            Cap::Erasure => Some(crate::erasure::PARTICIPANT_RULES),
            Cap::Branches => Some(crate::branches::RULES),
            Cap::BranchAt => Some(crate::branches::AT_RULES),
            Cap::Purge => Some(crate::purge::RULES),
            Cap::BlameAt => Some(crate::blame::AT_RULES),
            Cap::SpanIds => Some(crate::span::RULES),
            Cap::Wire => None,
        }
    }
}

/// One rule.
#[derive(Debug)]
pub struct Rule {
    pub id: &'static str,
    /// The requirement(s) it checks.
    pub req: &'static str,
    /// Every capability it needs; the first names the module that implements it.
    pub needs: &'static [Cap],
}

const fn r(id: &'static str, req: &'static str, needs: &'static [Cap]) -> Rule {
    Rule { id, req, needs }
}

/// Every rule, implemented or declared.
pub const RULES: &[Rule] = &[
    r("text.converge", "E-CORE-1, E-TEXT-2", &[Text]),
    r("text.no_interleave_forward", "E-TEXT-3", &[Text]),
    r("text.no_interleave_backward", "E-TEXT-1, K-CT-5", &[Text]),
    r(
        "text.no_interleave_backward_multi",
        "E-TEXT-1, K-CT-5(a)",
        &[Text],
    ),
    r("text.keep_concurrent_insert", "E-TEXT-6", &[Text]),
    r("text.delete_once", "E-TEXT-7", &[Text]),
    r("text.edit_atomic", "E-CORE-10", &[Text]),
    r("text.yata_separating_case", "E-TEXT-1, K-CT-5(b)", &[Text]),
    r("text.yata_paper_a19", "K-CT-5(e)", &[Text]),
    r("text.scalar_safe", "E-TEXT-9", &[Text]),
    r("text.edit_atomic_split", "K-CT-3", &[Events]),
    r("core.idempotent", "E-CORE-2", &[Events]),
    r("core.causal", "E-CORE-3", &[Events]),
    r("core.no_equivocation", "E-CORE-4", &[Events]),
    r(
        "text.blame_char",
        "E-TEXT-11, undo spec U18a, U23",
        &[Blame],
    ),
    r("text.cursor_stable", "E-TEXT-10", &[Anchors]),
    r("hist.undo_atomic", "E-HIST-6, K-CT-4", &[Undo]),
    r("hist.undo_partial", "K-HIST-13, K-CT-4", &[Undo]),
    r("hist.redo", "E-HIST-6, K-HIST-2", &[Undo]),
    r(
        "hist.undo_own_only",
        "E-HIST-5, undo spec U10, U10a (by root, v1.8), U18a, U23",
        &[Undo, Blame],
    ),
    r("hist.version_stable", "E-HIST-1", &[Versions]),
    r("hist.restore_forward", "E-HIST-2, K-HIST-8", &[Versions]),
    r(
        "hist.restore_minimal",
        "K-HIST-8, undo spec D1",
        &[Versions],
    ),
    r("hist.blame_grain", "E-HIST-4", &[BlameAt]),
    r("hist.branch_merge", "E-HIST-8", &[Branches]),
    r("hist.merge_any_base", "E-HIST-9", &[BranchAt]),
    r("sec.duplicate_ranges", "E-SEC-6", &[Wire]),
    r("sec.base_too_old", "E-SEC-12", &[Wire]),
    r(
        "sec.client_provenance_refused",
        "undo spec U23 (v1.8)",
        &[Wire],
    ),
    r(
        "erase.purge",
        "E-ERASE-1, K-PURGE-5",
        &[Purge, Blame, Anchors],
    ),
    r("erase.participant", "E-ERASE-5, K-PURGE-7", &[Erasure]),
    r("erase.no_readmit", "E-ERASE-6", &[Purge]),
    r("erase.versions", "E-ERASE-7, K-PURGE-4", &[HistoryPurge]),
    r("erase.span_includes_tombstones", "E-ERASE-1", &[SpanIds]),
];

/// The requirements' v1 rules marked "(new)" (slice 1's inventory, 17), plus
/// `hist.undo_atomic`, `hist.undo_partial` and `hist.redo` (K-CT-4, E-HIST-6): each must be
/// in [`RULES`].
pub const REQUIRED: &[&str] = &[
    "core.idempotent",
    "core.causal",
    "core.no_equivocation",
    "text.scalar_safe",
    "text.cursor_stable",
    "text.blame_char",
    "hist.version_stable",
    "hist.restore_forward",
    "hist.blame_grain",
    "hist.branch_merge",
    "hist.merge_any_base",
    "sec.duplicate_ranges",
    "sec.base_too_old",
    "erase.purge",
    "erase.participant",
    "erase.no_readmit",
    "erase.versions",
    "hist.undo_atomic",
    "hist.undo_partial",
    "hist.redo",
];

/// An engine, as the matrix sees it.
#[derive(Debug)]
pub struct Engine {
    pub name: &'static str,
    /// The capabilities its adapters implement (the suite checks its tests run exactly
    /// these capabilities' rules).
    pub caps: &'static [Cap],
    pub deviations: &'static [Deviation],
}

/// The kit's own [`k2_mpc_base::Doc`] (Yrs), and its sessions for the doc-level
/// capabilities (undo, versions, history purge, erasure).
pub const KIT: Engine = Engine {
    name: YrsText::ENGINE,
    caps: &[
        Text,
        Events,
        Blame,
        Anchors,
        Undo,
        Versions,
        HistoryPurge,
        Erasure,
        Branches,
        Purge,
    ],
    deviations: YrsText::KNOWN_DEVIATIONS,
};

/// The reference model mpce-model (v0.4.1).
pub const MODEL: Engine = Engine {
    name: ModelText::ENGINE,
    caps: &[
        Text,
        Events,
        Blame,
        Anchors,
        Versions,
        HistoryPurge,
        Branches,
        BranchAt,
        Purge,
        BlameAt,
        SpanIds,
    ],
    deviations: ModelText::KNOWN_DEVIATIONS,
};

pub const ENGINES: &[Engine] = &[KIT, MODEL];

/// A rule that doesn't run on an engine yet: why, and when it will.
#[derive(Debug)]
pub struct Awaiting {
    pub rule: &'static str,
    pub engine: &'static str,
    pub reason: &'static str,
    pub target: &'static str,
}

/// A rule that will never run on an engine, and why.
#[derive(Debug)]
pub struct Never {
    pub rule: &'static str,
    pub engine: &'static str,
    pub reason: &'static str,
}

const K: &str = KIT.name;
const M: &str = MODEL.name;

const fn aw(
    rule: &'static str,
    engine: &'static str,
    reason: &'static str,
    target: &'static str,
) -> Awaiting {
    Awaiting {
        rule,
        engine,
        reason,
        target,
    }
}

const fn nv(rule: &'static str, engine: &'static str, reason: &'static str) -> Never {
    Never {
        rule,
        engine,
        reason,
    }
}

const WIRE: &str = "decoding untrusted update bytes is the native decoder's; this suite feeds \
                    engines only bytes they made";
const WIRE_NEVER: &str = "the reference model has no wire format (native decoder / client \
                          protocol)";
const UNDO_NEVER: &str = "undo is the kit's, over a session (undo spec); the model has no sessions";
/// `sec.client_provenance_refused` on the kit: why it holds there without a wire test.
const PROVENANCE_KIT: &str = concat!(
    "holds by construction in the kit: provenance (undo spec U23) is stamped only by the ",
    "session's own undo, redo and revert (`Session::apply_local` names the elements a step ",
    "puts back and writes their `provenance` rows in the change's own transaction, ",
    "`store::history::put_changes`); no public API takes a root, and a client update ",
    "(`Doc::check_client_update` / `apply_client_update`) carries engine content only and ",
    "never reaches the provenance table or the session's map. Refusing a root a client ",
    "sends on the wire is the native decoder's"
);
/// Every (rule, engine) that doesn't run yet.
pub const AWAITING: &[Awaiting] = &[
    aw("sec.duplicate_ranges", K, WIRE, "native decoder, G2"),
    aw("sec.base_too_old", K, WIRE, "native decoder, G2"),
    aw(
        "sec.client_provenance_refused",
        K,
        PROVENANCE_KIT,
        "native decoder, G2 (the engine's)",
    ),
];

/// Every (rule, engine) that will never run.
pub const NEVER: &[Never] = &[
    nv(
        "erase.span_includes_tombstones",
        K,
        concat!(
            "Yrs has no element-level purge event: the kit's purge deletes the span and ",
            "re-encodes the doc with garbage collection on (K-PURGE-11), so no tombstone keeps ",
            "content, inside the span or outside it (erase.purge, erase.versions)"
        ),
    ),
    nv("sec.duplicate_ranges", M, WIRE_NEVER),
    nv("sec.base_too_old", M, WIRE_NEVER),
    nv("sec.client_provenance_refused", M, WIRE_NEVER),
    nv(
        "erase.participant",
        M,
        "the kit's mapping from agent IDs to people, not the model's",
    ),
    nv("hist.undo_atomic", M, UNDO_NEVER),
    nv("hist.undo_partial", M, UNDO_NEVER),
    nv("hist.redo", M, UNDO_NEVER),
    nv("hist.undo_own_only", M, UNDO_NEVER),
    nv(
        "hist.blame_grain",
        K,
        concat!(
            "blame at a version needs that version's items with their IDs. ",
            "Yrs keeps no engine state of a past version: garbage collection is on ",
            "(K-PURGE-11), so deleted items keep no content, and the kit's versions are ",
            "engine-neutral text (K-HIST-5) with no attribution. The kit blames the live doc ",
            "(text.blame_char; erase.purge checks a purge moves no other character's blame)"
        ),
    ),
    nv(
        "hist.merge_any_base",
        K,
        concat!(
            "a branch from a past version needs that version's engine state. ",
            "Yrs keeps none: garbage collection is on (K-PURGE-11), and the kit's versions are ",
            "engine-neutral text (K-HIST-5) over a log that is compacted and, at a purge, ",
            "replaced. The kit has no branch verb (K-BR-6 is the native engine's, G2a); a ",
            "branch from the current state runs (hist.branch_merge)"
        ),
    ),
];

/// The undo spec's trace test, which lives in its own crate.
pub const UNDO_TRACES: &str =
    "`crates/spec-undo` (`docs/SPEC-UNDO.md` §8): seeded traces replayed \
     through the oracle `spec_undo::model` and the kit's sessions on Yrs, compared after every \
     step (every root's text, the outcome, the conflict regions as an exact set, every pid's \
     stack depths); 256 traces in the normal run, every sequential case class hit; a large \
     run (10^4 hits per class) is `#[ignore]`d, for the compute nodes.";

/// The parts of the undo spec's test plan (§8) that come later, and when.
pub const LATER: &[(&str, &str)] = &[
    (
        "The **concurrency class** of the trace test: concurrent thin-client batches \
         integrated between undo steps, the oracle consuming the daemon's applied hunks; \
         equal wherever the merged texts agree, other differences classified by the Def. 4 \
         checker; Ctrl+Z with unacked ops (U21).",
        "G3",
    ),
    (
        "The trace test's **native leg** (oracle vs the kit on the native engine), and the \
         **positions PT**: each engine's reported hunk positions against `k2e-model`'s \
         element positions on the alphabet `{a, \\n}`, with a text-diffing mutant adapter \
         that must fail (N4, E-HIST-5).",
        "native engine, G2a / G3",
    ),
    (
        "Person grouping (U3, the daemon-side (g) split) and its case class; \
         `hist.undo_group_person`, `hist.ctrl_z_unacked`.",
        "native tab, G3",
    ),
];

/// A rule by id, in [`RULES`].
pub fn rule(id: &str) -> Option<&'static Rule> {
    RULES.iter().find(|r| r.id == id)
}

/// A rule's status on an engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Pass,
    /// A pinned deviation, with its reason.
    Deviation(&'static str),
    Awaiting {
        reason: &'static str,
        target: &'static str,
    },
    Never(&'static str),
    /// Doesn't run and isn't listed: always a failure.
    Unlisted,
}

/// Does `rule` run on `e`?
pub fn runs(rule: &Rule, e: &Engine) -> bool {
    rule.needs.iter().all(|c| e.caps.contains(c))
}

/// A matrix: the rules, the engines, and the lists that explain every gap.
pub struct Matrix<'a> {
    pub rules: &'a [Rule],
    pub engines: &'a [Engine],
    pub awaiting: &'a [Awaiting],
    pub never: &'a [Never],
    /// Rule ids that must be in `rules`.
    pub required: &'a [&'a str],
}

/// The suite's matrix.
pub const CURRENT: Matrix<'static> = Matrix {
    rules: RULES,
    engines: ENGINES,
    awaiting: AWAITING,
    never: NEVER,
    required: REQUIRED,
};

impl Matrix<'_> {
    fn rule(&self, id: &str) -> Option<&Rule> {
        self.rules.iter().find(|r| r.id == id)
    }

    fn engine(&self, name: &str) -> Option<&Engine> {
        self.engines.iter().find(|e| e.name == name)
    }

    pub fn status(&self, rule: &Rule, e: &Engine) -> Status {
        if runs(rule, e) {
            return match e.deviations.iter().find(|(id, _)| *id == rule.id) {
                Some((_, why)) => Status::Deviation(why),
                None => Status::Pass,
            };
        }
        if let Some(a) = self
            .awaiting
            .iter()
            .find(|a| a.rule == rule.id && a.engine == e.name)
        {
            return Status::Awaiting {
                reason: a.reason,
                target: a.target,
            };
        }
        if let Some(n) = self
            .never
            .iter()
            .find(|n| n.rule == rule.id && n.engine == e.name)
        {
            return Status::Never(n.reason);
        }
        Status::Unlisted
    }

    /// Everything wrong with the matrix (empty when it holds).
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (i, a) in self.rules.iter().enumerate() {
            if self.rules[..i].iter().any(|b| b.id == a.id) {
                out.push(format!("{}: listed twice", a.id));
            }
            let Some(first) = a.needs.first() else {
                out.push(format!("{}: needs no capability", a.id));
                continue;
            };
            // an implemented capability's module and this table agree
            if let Some(ids) = first.implemented() {
                if !ids.contains(&a.id) {
                    out.push(format!(
                        "{}: listed under {}, whose module doesn't implement it",
                        a.id,
                        first.name()
                    ));
                }
            }
            let runs_somewhere = self.engines.iter().any(|e| runs(a, e));
            if !runs_somewhere && !self.awaiting.iter().any(|w| w.rule == a.id) {
                out.push(format!(
                    "{}: runs on no engine and is not in AWAITING (give a reason and a target)",
                    a.id
                ));
            }
            for e in self.engines {
                if self.status(a, e) == Status::Unlisted {
                    out.push(format!(
                        "{} on {}: doesn't run and is in neither AWAITING nor NEVER",
                        a.id, e.name
                    ));
                }
            }
        }
        for c in Cap::ALL {
            for id in c.implemented().unwrap_or_default() {
                match self.rule(id) {
                    Some(r) if r.needs.first() == Some(&c) => {}
                    _ => out.push(format!(
                        "{id}: implemented by {} but not listed under it",
                        c.name()
                    )),
                }
            }
        }
        for e in self.engines {
            if !e.caps.contains(&Text) {
                out.push(format!("{}: every engine has Text", e.name));
            }
            for c in e.caps {
                if c.implemented().is_none() {
                    out.push(format!(
                        "{}: claims {}, which has no rules module",
                        e.name,
                        c.name()
                    ));
                }
            }
            for (id, why) in e.deviations {
                match self.rule(id) {
                    None => out.push(format!("{}: deviation {id} names no rule", e.name)),
                    Some(r) if !runs(r, e) => out.push(format!(
                        "{}: deviation {id} on a rule it doesn't run",
                        e.name
                    )),
                    _ if why.is_empty() => {
                        out.push(format!("{}: deviation {id} has no reason", e.name))
                    }
                    _ => {}
                }
            }
        }
        for a in self.awaiting {
            match (self.rule(a.rule), self.engine(a.engine)) {
                (Some(r), Some(e)) => {
                    if runs(r, e) {
                        out.push(format!(
                            "{} on {}: runs now; remove its AWAITING entry",
                            a.rule, a.engine
                        ));
                    }
                    if a.reason.is_empty() || a.target.is_empty() {
                        out.push(format!(
                            "{} on {}: AWAITING needs a reason and a target",
                            a.rule, a.engine
                        ));
                    }
                }
                _ => out.push(format!("AWAITING names an unknown rule or engine: {a:?}")),
            }
            if self
                .never
                .iter()
                .any(|n| n.rule == a.rule && n.engine == a.engine)
            {
                out.push(format!(
                    "{} on {}: both AWAITING and NEVER",
                    a.rule, a.engine
                ));
            }
        }
        for n in self.never {
            match (self.rule(n.rule), self.engine(n.engine)) {
                (Some(r), Some(e)) if runs(r, e) => out.push(format!(
                    "{} on {}: runs, but is listed as NEVER",
                    n.rule, n.engine
                )),
                (Some(_), Some(_)) if n.reason.is_empty() => {
                    out.push(format!("{} on {}: NEVER needs a reason", n.rule, n.engine))
                }
                (Some(_), Some(_)) => {}
                _ => out.push(format!("NEVER names an unknown rule or engine: {n:?}")),
            }
        }
        for id in self.required {
            if self.rule(id).is_none() {
                out.push(format!(
                    "{id}: a required (new) rule missing from the rules"
                ));
            }
        }
        out
    }

    /// The matrix as Markdown: the table, then the engines, the deviations, the AWAITING
    /// list and the NEVER list, each with its reason.
    pub fn render(&self) -> String {
        let mut s = String::new();
        s.push_str("# Conformance coverage\n\n");
        s.push_str(
            "Generated from `crates/conformance/src/matrix.rs`; a test keeps this file equal \
             to it (regenerate with `CONFORMANCE_BLESS=1 cargo test -p k2-mpc-conformance \
             --test matrix`).\n\n",
        );
        s.push_str(
            "A rule runs on every engine that has the capabilities it needs. A rule that \
             runs on no engine fails the suite unless it is listed as awaiting, with a \
             reason and a target. A pinned deviation is asserted to still happen.\n\n",
        );
        s.push_str("| Rule | Requirement | Needs |");
        for e in self.engines {
            s.push_str(&format!(" {} |", e.name));
        }
        s.push_str("\n|---|---|---|");
        for _ in self.engines {
            s.push_str("---|");
        }
        s.push('\n');
        for r in self.rules {
            let needs: Vec<&str> = r.needs.iter().map(|c| c.name()).collect();
            s.push_str(&format!(
                "| `{}` | {} | {} |",
                r.id,
                r.req,
                needs.join(" + ")
            ));
            for e in self.engines {
                s.push_str(&format!(" {} |", cell(&self.status(r, e))));
            }
            s.push('\n');
        }
        s.push_str("\n## Engines\n\n");
        for e in self.engines {
            let caps: Vec<&str> = e.caps.iter().map(|c| c.name()).collect();
            s.push_str(&format!("- **{}**: {}.\n", e.name, caps.join(", ")));
        }
        s.push_str("\n## Pinned deviations\n\n");
        for e in self.engines {
            for (id, why) in e.deviations {
                s.push_str(&format!("- `{id}` on {}: {why}\n", e.name));
            }
        }
        s.push_str("\n## Awaiting\n\n");
        for a in self.awaiting {
            s.push_str(&format!(
                "- `{}` on {}: {}. Target: {}.\n",
                a.rule, a.engine, a.reason, a.target
            ));
        }
        s.push_str("\n## Never\n\n");
        for n in self.never {
            s.push_str(&format!("- `{}` on {}: {}.\n", n.rule, n.engine, n.reason));
        }
        s.push_str("\n## Undo spec trace test\n\n");
        s.push_str(UNDO_TRACES);
        s.push_str("\n\nLater (not in this suite yet):\n\n");
        for (what, when) in LATER {
            s.push_str(&format!("- {what} Target: {when}.\n"));
        }
        s
    }
}

fn cell(s: &Status) -> String {
    match s {
        Status::Pass => "pass".into(),
        Status::Deviation(_) => "**deviation** (pinned)".into(),
        Status::Awaiting { target, .. } => format!("awaiting: {target}"),
        Status::Never(_) => "never".into(),
        Status::Unlisted => "**UNLISTED**".into(),
    }
}
