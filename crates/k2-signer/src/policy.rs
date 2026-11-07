//! A project's policy: `/usr/local/etc/k2-signer/projects/<project>.toml`,
//! root-owned, hashed into `pins.sha256`, approved at the signer by a person
//! (`k2-signer-admin policy-approve`). **Never read from the repo at sign
//! time** (PRD §4.5): the repo's `.k2/signer.toml` is only a proposal.
//!
//! The format is frozen in `k2_node_proto::signer_policy` (C-1). The signer
//! adds its own rules on top ([`signer_rules`]), all narrowing:
//! - every asset signed with `check = "artifact"` must be in its build's
//!   `dual` list: a file is signed only when an independent verify build
//!   reproduced it byte for byte (k2sign's "one node alone must never get a
//!   file signed");
//! - `check = "inner_binary"` (Q11) is refused until C8 builds it;
//! - every `[keys.<slot>]` public key parses as a minisign key.

use std::path::PathBuf;

use k2_node_proto::signer_policy::{AssetCheck, SignerPolicy};

use crate::ctx::{self, Ctx};
use crate::{refused, Fail, R};

/// Largest policy file.
pub const MAX_POLICY_BYTES: u64 = 256 * 1024;

pub fn policy_path(ctx: &Ctx, project: &str) -> R<PathBuf> {
    check_project_name(project)?;
    Ok(ctx.projects_dir().join(format!("{project}.toml")))
}

pub fn check_project_name(project: &str) -> R<()> {
    if !k2_node_proto::ci::is_name(project) {
        return Err(Fail::Usage(format!("bad project name {project:?} ([a-z0-9._-], 1-63)")));
    }
    Ok(())
}

/// Parse and check a policy text (the frozen rules plus the signer's).
pub fn parse(text: &str, project: &str) -> R<SignerPolicy> {
    let p = SignerPolicy::parse(text).map_err(Fail::Refused)?;
    if p.project != project {
        return refused(format!("the policy says project {:?}, its file name says {project:?}", p.project));
    }
    signer_rules(&p)?;
    Ok(p)
}

/// The signer's own rules on top of the frozen schema (all narrowing).
pub fn signer_rules(p: &SignerPolicy) -> R<()> {
    for (name, a) in &p.assets {
        let b = &p.build[&a.build];
        match a.check {
            AssetCheck::Artifact => {
                if !b.dual.contains(&a.artifact) {
                    return refused(format!(
                        "policy: asset {name} ({}) is not in build {}'s dual list: a file is signed only when an \
                         independent verify build reproduced it byte for byte",
                        a.artifact, a.build
                    ));
                }
            }
            AssetCheck::InnerBinary => {
                return refused(format!("policy: asset {name}: check = \"inner_binary\" is not built yet (C8, Q11)"));
            }
        }
    }
    for (slot, k) in &p.keys {
        crate::minisign::PublicKey::parse(&k.public).map_err(|e| Fail::Refused(format!("policy: keys.{slot}.public: {e}")))?;
    }
    Ok(())
}

/// The installed policy for `project` (root-owned in production).
pub fn load(ctx: &Ctx, project: &str) -> R<SignerPolicy> {
    let path = policy_path(ctx, project)?;
    if !path.is_file() {
        return refused(format!(
            "no policy for project {project} (a person approves one: sudo /usr/local/libexec/k2-signer/k2-signer-admin policy-approve {project} <sha256> < policy.toml)"
        ));
    }
    let raw = ctx::read_trusted(ctx, &path, MAX_POLICY_BYTES)?;
    let text = String::from_utf8(raw).map_err(|_| Fail::Refused(format!("{}: not UTF-8", path.display())))?;
    parse(&text, project)
}

/// A line diff of two policy texts (for the person who approves a change).
pub fn diff(old: &str, new: &str) -> String {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    // Longest common subsequence: policies are small (≤ 256 KiB, a few hundred lines).
    let (n, m) = (a.len(), b.len());
    let mut l = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            l[i][j] = if a[i] == b[j] { l[i + 1][j + 1] + 1 } else { l[i + 1][j].max(l[i][j + 1]) };
        }
    }
    let (mut i, mut j, mut out) = (0, 0, String::new());
    while i < n || j < m {
        if i < n && j < m && a[i] == b[j] {
            out.push_str(&format!("  {}\n", a[i]));
            i += 1;
            j += 1;
        } else if j < m && (i == n || l[i][j + 1] >= l[i + 1][j]) {
            out.push_str(&format!("+ {}\n", b[j]));
            j += 1;
        } else {
            out.push_str(&format!("- {}\n", a[i]));
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_marks_added_and_removed_lines() {
        let d = diff("a\nb\nc\n", "a\nc\nd\n");
        assert_eq!(d, "  a\n- b\n  c\n+ d\n");
    }
}
