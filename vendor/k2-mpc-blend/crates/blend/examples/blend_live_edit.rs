//! Edits a BLEND file through the pack's live tier, as a host does: test tooling for the
//! headless round trip's live write-backs (`PROVENANCE.md`).
//!
//!     cargo run --example blend_live_edit -- IN OUT VERB...
//!
//! VERB is one live edit verb, in the words the live rows use: `object place ADDR --loc X,Y,Z`
//! (`--rot`, `--scale`), `object visibility ADDR --viewport off` (`--select`, `--render`;
//! `on` or `off`), `object rename ADDR --name NAME --break-links`. ADDR is a live address
//! (`@obj:N`) or `@name:NAME`, the live scene's object of that name.
//!
//! IN opens through the kit's session with this pack, exactly as a host opens a file: a session
//! manager over a doc store and a blob store in a fresh temporary folder, a host granting
//! everyone everything. An agent reads the scene, runs the verb with that read's version as its
//! base, and the kit writes the doc back to the file (the pack's write-back), which is then
//! copied to OUT. Prints `LIVE ok <format> <bytes>` (exit 0). A file the live tier declines
//! prints `LIVE FAIL <code>: <why>` (`live_version`), a verb it refuses the same with the
//! refusal's code (`blend_driven`), anything else `LIVE FAIL <step>: <why>`; each exits 1 and
//! writes no OUT.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use k2_mpc_base::blob::{BlobConfig, BlobStore};
use k2_mpc_base::doc::Neutral;
use k2_mpc_base::elem::Value;
use k2_mpc_base::session::{
    AuditEvent, Config, Host, Pack, Participant, ParticipantKind, Rights, SessionManager, Stay,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_blend::live::{BlendLive, OBJS, OBJ_PROPS, SCENE};

/// The host: everyone may do everything; one fixed clock.
struct Grant;

impl Host for Grant {
    fn rights(&self, _who: &Participant, _key: &DocKey) -> Rights {
        Rights::full()
    }
    fn now_ms(&self) -> u64 {
        1_793_111_400_000
    }
    fn on_event(&self, _ev: &AuditEvent) {}
}

/// The file's name in the session's folder.
const NAME: &str = "scene.blend";

/// The scene map's `version` (`5.03`) in a live doc's roots.
fn version_of(roots: &[(String, Neutral)]) -> Option<String> {
    roots.iter().find_map(|(r, n)| match n {
        Neutral::Map(m) if r == SCENE => {
            m.get("version").and_then(Value::as_str).map(str::to_owned)
        }
        _ => None,
    })
}

/// The live address (`@obj:N`) of the object named `name` in a live doc's roots.
fn address_named(roots: &[(String, Neutral)], name: &str) -> Option<String> {
    let props = roots.iter().find_map(|(r, n)| match n {
        Neutral::Map(m) if r == OBJ_PROPS => Some(m),
        _ => None,
    })?;
    roots.iter().find_map(|(r, n)| match n {
        Neutral::List(l) if r == OBJS => l.iter().find_map(|(k, _)| {
            let k = k.as_str();
            let named = props.get(&format!("{k}\u{1f}name")).and_then(Value::as_str);
            (named == Some(name)).then(|| format!("@obj:{}", &k[1..]))
        }),
        _ => None,
    })
}

/// `input` opened live, `argv` run by an agent and written back: `(format, the file's bytes)`,
/// or why not (`<code>: <why>`), on one line.
pub fn live_edit(input: &[u8], argv: &[String]) -> Result<(String, Vec<u8>), String> {
    // the live tier's own decline, as a host's open gets it
    if let Err(d) = BlendLive::default().decode(input) {
        return Err(format!("{}: {}", d.code, d.message));
    }
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "blend-live-edit-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = fs::remove_dir_all(&dir);
    let out = in_session(&dir, input, argv);
    let _ = fs::remove_dir_all(&dir);
    out.map_err(|e| e.replace('\n', " | "))
}

fn in_session(dir: &Path, input: &[u8], argv: &[String]) -> Result<(String, Vec<u8>), String> {
    let ws = dir.join("ws");
    fs::create_dir_all(&ws).map_err(|e| format!("folder: {e}"))?;
    let ws: PathBuf = fs::canonicalize(ws).map_err(|e| format!("folder: {e}"))?;
    let host = Arc::new(Grant);
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).map_err(|e| format!("store: {e}"))?);
    let h = host.clone();
    let blobs = Arc::new(
        BlobStore::open(
            store.clone(),
            dir.join("blobs"),
            BlobConfig::default(),
            Arc::new(move || h.now_ms()),
        )
        .map_err(|e| format!("blob store: {e}"))?,
    );
    let mgr = SessionManager::new(store, host, Config::default()).with_blob_store(blobs);
    let path = ws.join(NAME);
    fs::write(&path, input).map_err(|e| format!("file: {e}"))?;
    let key = DocKey {
        space: "ws".into(),
        path: NAME.into(),
    };
    let s = mgr
        .open_file(Arc::new(BlendLive::default()), &key, &path, &ws)
        .map_err(|e| format!("open: {e}"))?;
    let agent = s
        .join(
            Participant::new("agent", "agent", "#123456", ParticipantKind::Agent, "cli"),
            Stay::Held,
        )
        .map_err(|e| format!("join: {e}"))?
        .conn;
    let list: Vec<String> = ["scene", "list", "--limit", "1"]
        .iter()
        .map(|a| (*a).to_owned())
        .collect();
    let base = s
        .run_tool(agent, OBJS, &list, b"")
        .map_err(|e| format!("read: {e}"))?
        .version;
    let roots = s.read(agent).map_err(|e| format!("read: {e}"))?.content;
    let version =
        version_of(&roots).ok_or_else(|| "read: the scene map has no version".to_owned())?;
    let mut args = Vec::with_capacity(argv.len() + 2);
    for a in argv {
        match a.strip_prefix("@name:") {
            Some(name) => args.push(
                address_named(&roots, name)
                    .ok_or_else(|| format!("blend_not_found: no live object named {name:?}"))?,
            ),
            None => args.push(a.clone()),
        }
    }
    args.push("--base".into());
    args.push(base.to_string());
    let o = s
        .run_tool(agent, OBJS, &args, b"")
        .map_err(|e| format!("verb: {e}"))?;
    let text = String::from_utf8_lossy(&o.stdout).into_owned();
    if o.exit != 0 {
        let first = text.lines().next().unwrap_or("");
        return Err(match first.strip_prefix("refused: ") {
            Some(r) => r.to_owned(),
            None => format!("verb: exit {}: {text}", o.exit),
        });
    }
    let flushed = s.flush(agent).map_err(|e| format!("write-back: {e}"))?;
    if !flushed.written {
        return Err("write-back: nothing written".into());
    }
    let bytes = fs::read(&path).map_err(|e| format!("file: {e}"))?;
    Ok((version, bytes))
}

/// Prints the failure line and exits 1.
fn fail(why: String) -> ! {
    println!("LIVE FAIL {why}");
    std::process::exit(1)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 3 {
        fail("usage: blend_live_edit IN OUT VERB...".into());
    }
    let input = fs::read(&args[0]).unwrap_or_else(|e| fail(format!("input: {e}")));
    match live_edit(&input, &args[2..]) {
        Ok((version, bytes)) => {
            if let Err(e) = fs::write(&args[1], &bytes) {
                fail(format!("output: {e}"));
            }
            println!("LIVE ok {version} {}", bytes.len());
        }
        Err(why) => fail(why),
    }
}
