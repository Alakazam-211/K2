//! What the forge says about a release: the tag's commit and tree, and the
//! Rust channel the commit pins. The signer gathers it itself, with its own
//! read-only token, through `/usr/bin/curl` with the system trust store
//! (k2sign's `gh_api`): nothing from the caller's checkout or environment
//! counts. Tests use [`Forge`] fakes.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use k2_node_proto::signer_policy::{SignerPolicy, SourceKind};
use serde_json::Value;

use crate::ctx::{self, Ctx, Scratch};
use crate::{refused, Fail, R};

/// A compare between two commits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Compare {
    pub status: String,
    pub ahead_by: u64,
    pub files: Vec<String>,
}

pub trait Forge {
    /// The commit a tag points at (annotated tags peeled); a refusal when
    /// there is no such tag.
    fn tag_commit(&self, repo: &str, tag: &str) -> R<String>;
    /// `(tree, parents)` of a commit.
    fn commit(&self, repo: &str, sha: &str) -> R<(String, Vec<String>)>;
    fn compare(&self, repo: &str, base: &str, head: &str) -> R<Compare>;
    /// A file's bytes at a commit; `None` when it doesn't exist.
    fn file_at(&self, repo: &str, path: &str, rev: &str) -> R<Option<Vec<u8>>>;
}

/// What the forge vouches for about `<tag_prefix><version>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseSource {
    /// The tag's commit (T).
    pub tag_commit: String,
    pub tree: String,
    /// The approved base commit (B).
    pub base: String,
    /// The `rust-toolchain.toml` channel at T, when a build needs `{rust_pin}`.
    pub rust_pin: Option<String>,
}

fn is_sha(s: &str) -> bool {
    s.len() == 40 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Bind a version to commits (policy `[source] kind = "github_tag"`).
///
/// The tag must be the approved base commit B itself. `release_files`
/// (project `k2` only: k2sign's `release_commit_files`, release.sh's
/// version bump) also allows T = B + one commit that changes only those
/// files, with B on `main`, exactly as k2sign checks it.
pub fn resolve_github(
    forge: &dyn Forge,
    policy: &SignerPolicy,
    version: &str,
    base: &str,
    release_files: Option<&[String]>,
    need_rust_pin: bool,
) -> R<ReleaseSource> {
    if policy.source.kind != SourceKind::GithubTag {
        return Err(Fail::Internal("resolve_github on a non-github policy".into()));
    }
    let repo = policy.source.repo.as_deref().ok_or_else(|| Fail::Refused("policy: no source.repo".into()))?;
    let tag = format!("{}{version}", policy.source.tag_prefix.as_deref().unwrap_or("v"));
    let t = forge.tag_commit(repo, &tag)?;
    if !is_sha(&t) {
        return refused(format!("tag {tag} on GitHub does not point at a commit"));
    }
    let (tree, parents) = forge.commit(repo, &t)?;
    if !is_sha(&tree) {
        return refused(format!("GitHub says {tag}'s tree is {tree:?}"));
    }
    if t != base {
        let Some(files) = release_files else {
            return refused(format!(
                "tag {tag} is commit {}, not the approved commit {} (the approval names the tagged commit)",
                &t[..12],
                &base[..12.min(base.len())]
            ));
        };
        if parents != [base.to_string()] {
            return refused(format!(
                "release commit is not base+bump ({tag} is {}, parents {}; the approved base is {})",
                &t[..12],
                parents.iter().map(|p| &p[..12.min(p.len())]).collect::<Vec<_>>().join(","),
                &base[..12.min(base.len())]
            ));
        }
        let cmp = forge.compare(repo, base, &t)?;
        let bad: Vec<&String> = cmp.files.iter().filter(|f| !files.contains(f)).collect();
        if !bad.is_empty() {
            return refused(format!(
                "release commit changes non-release files: {}",
                bad.iter().take(5).map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
            ));
        }
        if cmp.status != "ahead" || cmp.ahead_by != 1 || cmp.files.len() >= 300 {
            return refused(format!("release commit is not base+bump (compare says {}, ahead by {})", cmp.status, cmp.ahead_by));
        }
    }
    if release_files.is_some() {
        let on_main = forge.compare(repo, base, "main")?;
        if on_main.status != "ahead" && on_main.status != "identical" {
            return refused(format!("base commit is not on GitHub main ({}; compare says {})", &base[..12.min(base.len())], on_main.status));
        }
    }
    let rust_pin = if need_rust_pin { Some(rust_pin_at(forge, repo, &t, &tag)?) } else { None };
    Ok(ReleaseSource { tag_commit: t, tree, base: base.to_string(), rust_pin })
}

/// The one `channel = "…"` in `rust-toolchain.toml` at `rev`.
pub fn rust_pin_at(forge: &dyn Forge, repo: &str, rev: &str, what: &str) -> R<String> {
    let Some(bytes) = forge.file_at(repo, "rust-toolchain.toml", rev)? else {
        return refused(format!("{what} has no rust-toolchain.toml on GitHub"));
    };
    let text = String::from_utf8(bytes).map_err(|_| Fail::Refused(format!("rust-toolchain.toml at {what} is not UTF-8")))?;
    let pins = toolchain_channels(&text);
    if pins.len() != 1 {
        return refused(format!("rust-toolchain.toml at {what} pins {} channels, want exactly 1", pins.len()));
    }
    Ok(pins[0].clone())
}

/// k2sign's `TOOLCHAIN_CHANNEL_RE`: `channel = "<[0-9A-Za-z][0-9A-Za-z.-]{0,63}>"` lines.
pub fn toolchain_channels(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let l = line.trim();
        let Some(rest) = l.strip_prefix("channel") else { continue };
        let Some(rest) = rest.trim_start().strip_prefix('=') else { continue };
        let v = rest.trim();
        let Some(v) = v.strip_prefix('"').and_then(|v| v.strip_suffix('"')) else { continue };
        if !v.is_empty()
            && v.len() <= 64
            && v.as_bytes()[0].is_ascii_alphanumeric()
            && v.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
        {
            out.push(v.to_string());
        }
    }
    out
}

// ── GitHub through /usr/bin/curl ───────────────────────────────────────

/// The real forge: GitHub's REST API with the signer's own read-only token.
pub struct GitHub<'a> {
    pub ctx: &'a Ctx,
    pub api: String,
    /// The token file (`/var/k2sign/gh/token` for project `k2`, shared with
    /// k2sign; `/var/k2sign/k2-signer/projects/<p>/gh-token` for a client).
    /// `None`: public repos only.
    pub token_file: Option<PathBuf>,
}

impl<'a> GitHub<'a> {
    pub fn for_project(ctx: &'a Ctx, project: &str) -> R<Self> {
        let token_file = if project == "k2" {
            ctx.var().join("gh").join("token")
        } else {
            ctx.own_dir().join("projects").join(project).join("gh-token")
        };
        Ok(Self { ctx, api: ctx.github_api()?, token_file: token_file.is_file().then_some(token_file) })
    }

    fn get(&self, path: &str, missing: Option<&str>) -> R<Value> {
        let scratch = Scratch::new(self.ctx)?;
        let hdr = scratch.0.join("headers");
        let mut h = String::from("Accept: application/vnd.github+json\nX-GitHub-Api-Version: 2022-11-28\nUser-Agent: k2-signer\n");
        if let Some(tf) = &self.token_file {
            if ctx::mode_of(tf)? != 0o600 {
                return refused(format!("{} must be mode 600", tf.display()));
            }
            let t = std::fs::read_to_string(tf)?;
            let t = t.trim();
            if t.is_empty() || !t.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
                return refused("the signer's saved GitHub token is malformed (set it again)");
            }
            // In a 0600 file, never on argv.
            h = format!("Authorization: Bearer {t}\n{h}");
        }
        ctx::write_file(self.ctx, &hdr, h.as_bytes(), 0o600)?;
        let proto = if self.api.starts_with("http://") { "=http" } else { "=https" };
        let out = Command::new("/usr/bin/curl")
            .args(["-sS", "--fail-with-body", "--proto", proto, "--max-time", "60", "--max-filesize", "20000000", "-H"])
            .arg(format!("@{}", hdr.display()))
            .arg(format!("{}{path}", self.api))
            .env_clear()
            .env("PATH", ctx::SYS_PATH)
            .env("LANG", "C")
            .current_dir("/")
            .stdin(Stdio::null())
            .output()
            .map_err(|e| Fail::Refused(format!("can't run /usr/bin/curl: {e}")))?;
        if !out.status.success() {
            if let Some(m) = missing {
                if String::from_utf8_lossy(&out.stdout).contains("\"Not Found\"") {
                    return refused(m.to_string());
                }
            }
            let err = if out.stderr.is_empty() { &out.stdout } else { &out.stderr };
            let line = String::from_utf8_lossy(err).lines().next().unwrap_or_default().chars().take(200).collect::<String>();
            return refused(format!("GitHub API {path} failed: {line}"));
        }
        serde_json::from_slice(&out.stdout).map_err(|_| Fail::Refused(format!("GitHub API {path} did not return JSON")))
    }
}

fn s<'v>(v: &'v Value, k: &str) -> &'v str {
    v.get(k).and_then(Value::as_str).unwrap_or_default()
}

impl Forge for GitHub<'_> {
    fn tag_commit(&self, repo: &str, tag: &str) -> R<String> {
        let r = self.get(&format!("/repos/{repo}/git/ref/tags/{tag}"), Some(&format!("no tag {tag} on GitHub yet")))?;
        let mut obj = r.get("object").cloned().unwrap_or(Value::Null);
        if s(&obj, "type") == "tag" {
            let sha = s(&obj, "sha").to_string();
            if !is_sha(&sha) {
                return refused(format!("tag {tag}: a bad annotated tag object"));
            }
            obj = self.get(&format!("/repos/{repo}/git/tags/{sha}"), None)?.get("object").cloned().unwrap_or(Value::Null);
        }
        if s(&obj, "type") != "commit" || !is_sha(s(&obj, "sha")) {
            return refused(format!("tag {tag} on GitHub does not point at a commit"));
        }
        Ok(s(&obj, "sha").to_string())
    }

    fn commit(&self, repo: &str, sha: &str) -> R<(String, Vec<String>)> {
        let c = self.get(&format!("/repos/{repo}/commits/{sha}"), None)?;
        let tree = c.get("commit").and_then(|x| x.get("tree")).map(|t| s(t, "sha").to_string()).unwrap_or_default();
        let parents = c
            .get("parents")
            .and_then(Value::as_array)
            .map(|a| a.iter().map(|p| s(p, "sha").to_string()).collect())
            .unwrap_or_default();
        Ok((tree, parents))
    }

    fn compare(&self, repo: &str, base: &str, head: &str) -> R<Compare> {
        let c = self.get(&format!("/repos/{repo}/compare/{base}...{head}"), None)?;
        Ok(Compare {
            status: s(&c, "status").to_string(),
            ahead_by: c.get("ahead_by").and_then(Value::as_u64).unwrap_or(0),
            files: c
                .get("files")
                .and_then(Value::as_array)
                .map(|a| a.iter().map(|f| s(f, "filename").to_string()).collect())
                .unwrap_or_default(),
        })
    }

    fn file_at(&self, repo: &str, path: &str, rev: &str) -> R<Option<Vec<u8>>> {
        let missing = format!("{path} not at {rev}");
        let j = match self.get(&format!("/repos/{repo}/contents/{path}?ref={rev}"), Some(&missing)) {
            Ok(j) => j,
            Err(Fail::Refused(m)) if m == missing => return Ok(None),
            Err(e) => return Err(e),
        };
        if s(&j, "encoding") != "base64" {
            return refused(format!("GitHub's {path} at {rev} is not a base64 file"));
        }
        let squeezed: String = s(&j, "content").split_whitespace().collect();
        Ok(Some(k2_node_proto::crypto::unb64(&squeezed).map_err(|e| Fail::Refused(format!("{path}: {e}")))?))
    }
}

/// A forge for tests and rehearsals: canned answers.
#[derive(Debug, Default, Clone)]
pub struct FakeForge {
    pub tags: BTreeMap<String, String>,
    pub commits: BTreeMap<String, (String, Vec<String>)>,
    pub compares: BTreeMap<(String, String), Compare>,
    pub files: BTreeMap<(String, String), Vec<u8>>,
}

impl Forge for FakeForge {
    fn tag_commit(&self, _repo: &str, tag: &str) -> R<String> {
        self.tags.get(tag).cloned().ok_or_else(|| Fail::Refused(format!("no tag {tag} on GitHub yet")))
    }
    fn commit(&self, _repo: &str, sha: &str) -> R<(String, Vec<String>)> {
        self.commits.get(sha).cloned().ok_or_else(|| Fail::Refused(format!("GitHub API: no commit {sha}")))
    }
    fn compare(&self, _repo: &str, base: &str, head: &str) -> R<Compare> {
        self.compares
            .get(&(base.to_string(), head.to_string()))
            .cloned()
            .ok_or_else(|| Fail::Refused(format!("GitHub API: no compare {base}...{head}")))
    }
    fn file_at(&self, _repo: &str, path: &str, rev: &str) -> R<Option<Vec<u8>>> {
        Ok(self.files.get(&(path.to_string(), rev.to_string())).cloned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toolchain_channels_reads_k2sign_lines() {
        assert_eq!(toolchain_channels("[toolchain]\nchannel = \"1.99.1\"\ncomponents = [\"rustfmt\"]\n"), vec!["1.99.1"]);
        assert_eq!(toolchain_channels("channel=\"stable\"\n  channel = \"1.98.0\"\n").len(), 2);
        assert!(toolchain_channels("channel = 1.99\n").is_empty());
        assert!(toolchain_channels("channel = \"-x\"\n").is_empty());
    }
}
