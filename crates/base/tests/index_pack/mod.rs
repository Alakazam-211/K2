//! A small pack for the structure-index tests: `IDX` files, one line per node, so a test
//! writes a file's tree by hand. It stands in for a real file type's reader (the kit knows
//! none).
//!
//! ```text
//! IDX
//! #source fmt-2
//! #attr seed=7
//! key|kind|name|parent|vis|text|content|note
//! ```
//!
//! `key`, `text`, `content` and `note` are `-` for none; `parent` is a line index or `-`;
//! `vis` is 0 or 1. Any other shape is `Unreadable`.
#![allow(dead_code)]

use std::io::Read;

use k2_mpc_base::index::{
    cut_text, Digest, DraftNode, IndexDraft, IndexLimits, IndexRefused, IndexSpec, IndexVerbs,
    INDEX_TEXT_MAX,
};
use k2_mpc_base::session::{AgentWrites, BlobPack};

pub const VERBS: IndexVerbs =
    IndexVerbs::new("part list", "part show", "part diff", "part blame", None);

/// The test pack. `stop_after`: read only that many bytes (the pack stops early).
#[derive(Clone)]
pub struct IxPack {
    pub pack: &'static str,
    pub format: u32,
    pub refuse_agents: bool,
    pub stop_after: Option<u64>,
    /// Put an unkeyed root node before the file's own (a reader whose output changed).
    pub extra_root: bool,
}

impl Default for IxPack {
    fn default() -> Self {
        Self {
            pack: "idx",
            format: 1,
            refuse_agents: false,
            stop_after: None,
            extra_root: false,
        }
    }
}

impl BlobPack for IxPack {
    fn index_spec(&self) -> Option<IndexSpec> {
        Some(IndexSpec::new(self.pack, self.format, &VERBS))
    }

    fn index(
        &self,
        src: &mut dyn Read,
        _size: u64,
        limits: &IndexLimits,
    ) -> Result<IndexDraft, IndexRefused> {
        let mut bytes = Vec::new();
        let read = match self.stop_after {
            Some(n) => src.take(n).read_to_end(&mut bytes),
            None => src.read_to_end(&mut bytes),
        };
        read.map_err(IndexRefused::Read)?;
        let mut draft = parse(&bytes, limits)?;
        if self.extra_root {
            for n in &mut draft.nodes {
                n.parent = n.parent.map(|p| p + 1);
            }
            draft
                .nodes
                .insert(0, DraftNode::new("extra", "Extra", Digest::of(b"extra")));
        }
        Ok(draft)
    }

    fn agent_writes(&self) -> AgentWrites {
        if self.refuse_agents {
            AgentWrites::Refused
        } else {
            AgentWrites::Allowed
        }
    }
}

fn parse(bytes: &[u8], limits: &IndexLimits) -> Result<IndexDraft, IndexRefused> {
    let bad = |why: &str| IndexRefused::Unreadable {
        why: why.to_owned(),
    };
    let text = std::str::from_utf8(bytes).map_err(|_| bad("not text"))?;
    let mut lines = text.lines();
    if lines.next() != Some("IDX") {
        return Err(bad("no IDX header"));
    }
    let mut draft = IndexDraft::new("fmt-1");
    for line in lines {
        if let Some(s) = line.strip_prefix("#source ") {
            draft.source = s.to_owned();
            continue;
        }
        if let Some(a) = line.strip_prefix("#attr ") {
            let (k, v) = a.split_once('=').ok_or_else(|| bad("attr"))?;
            draft.attrs.insert(k.to_owned(), v.to_owned());
            continue;
        }
        let f: Vec<&str> = line.split('|').collect();
        if f.len() < 7 {
            return Err(bad("short line"));
        }
        let opt = |s: &str| (s != "-").then(|| s.to_owned());
        let mut n = DraftNode::new(f[1], f[2], Digest::of(line.as_bytes()));
        n.key = opt(f[0]);
        n.parent = if f[3] == "-" {
            None
        } else {
            Some(f[3].parse().map_err(|_| bad("parent"))?)
        };
        n.visible = f[4] == "1";
        if let Some(t) = opt(f[5]) {
            let (kept, cut) = cut_text(&t, limits.max_text_bytes.min(INDEX_TEXT_MAX));
            if cut {
                n.flags.insert("text_truncated".to_owned());
            }
            n.text = Some(kept.to_owned());
        }
        n.content = opt(f[6]).map(|c| Digest::of(c.as_bytes()));
        if let Some(note) = f.get(7).copied().and_then(opt) {
            n.attrs.insert("note".to_owned(), note.to_owned());
            n.weak.insert("placed".to_owned(), note.to_owned());
        }
        draft.nodes.push(n);
    }
    Ok(draft)
}

/// An `IDX` file from node lines.
pub fn file(lines: &[&str]) -> Vec<u8> {
    let mut s = String::from("IDX\n");
    for l in lines {
        s.push_str(l);
        s.push('\n');
    }
    s.into_bytes()
}
