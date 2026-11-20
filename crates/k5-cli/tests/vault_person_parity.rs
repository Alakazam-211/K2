//! `k5-cli vault` for the people-only steps and the moves
//! (`families::vault::person` and `families::vault::carry`, and `import`)
//! against `cli/k2 vault` on the vault rig ([`common::vault`]): the
//! recovery key's `show-once` and `rotate`, `rollback`, `backup` and
//! `backup key`, `restore`, `import` and `carry export|import`. Each
//! confirm, each refusal, the usage lines, a body Python can't read, and
//! the typed secrets, which no child sees.

#![cfg(unix)]

mod common;

use common::parity::PASSPORT;
use common::stub::Reply;
use common::vault::{VCase, VCompare, VRig};
use std::path::{Path, PathBuf};

fn ok(body: &str) -> Reply {
    Reply::Answer(200, body.to_string())
}

/// A word for a case's command line that lives as long as the test.
fn word(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

/// One request to the daemon, this answer, this exit.
fn daemon(name: &'static str, args: &[&'static str], reply: Reply, code: i32) -> VCase {
    let mut c = VCase::new(name, args);
    c.daemon = vec![reply];
    c.daemon_requests = 1;
    c.code = code;
    c
}

/// Nothing sent: a usage line, exit 2.
fn refused(name: &'static str, args: &[&'static str]) -> VCase {
    let mut c = VCase::new(name, args);
    c.code = 2;
    c
}

/// A person's verb: the vault socket answers, stdin as given.
fn person(
    name: &'static str,
    args: &[&'static str],
    replies: Vec<Reply>,
    stdin: &[u8],
    code: i32,
) -> VCase {
    let mut c = VCase::new(name, args);
    c.vault_requests = replies.len();
    c.vault = Some(replies);
    c.stdin = stdin.to_vec();
    c.code = code;
    c
}

fn traceback(mut c: VCase) -> VCase {
    c.compare = VCompare::Traceback;
    c
}

fn secrets(mut c: VCase, s: &[&'static str]) -> VCase {
    c.secrets = s.to_vec();
    c
}

fn methods(m: &str) -> Reply {
    ok(&format!(
        r#"{{"methods":["{m}"],"confirmText":"a person's step"}}"#
    ))
}

const PHRASE: &str = "alpha bravo charlie delta echo foxtrot golf hotel india juliet kilo lima mike november oscar papa quebec romeo sierra tango uniform victor whiskey xray";

fn dir(rig: &VRig, name: &str) -> PathBuf {
    let d = rig.path().join(name);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn text(p: &Path) -> &'static str {
    word(p.to_str().unwrap().to_string())
}

#[test]
fn recovery_show_once_and_rotate() {
    let rig = VRig::new();
    let shown = ok(&format!(r#"{{"id":"rk-1","phrase":"{PHRASE}"}}"#));
    let words = b"word-three-x\nword-eleven-y\nword-twenty-z\n";
    let mut in_k2 = refused("show-once in a K2 terminal", &["recovery", "show-once"]);
    in_k2.env = vec![("K2_HOOK_TOKEN", PASSPORT.to_string())];
    in_k2.code = 3;
    for c in [
        secrets(
            person(
                "show-once, a key confirmed before",
                // covers: vault recovery show-once
                &["recovery", "show-once"],
                vec![ok(r#"{"recovery":"confirmed"}"#), shown.clone(), ok("{}")],
                words,
                0,
            ),
            &["word-three-x", "word-eleven-y", "word-twenty-z"],
        ),
        secrets(
            person(
                "show-once, the first key: this computer's check",
                &["recovery", "show-once"],
                vec![
                    ok(r#"{"recovery":"none"}"#),
                    methods("local-auth"),
                    ok(r#"{"token":"tk-rs-1"}"#),
                    shown.clone(),
                    ok("{}"),
                ],
                words,
                0,
            ),
            &["tk-rs-1", "word-three-x"],
        ),
        secrets(
            person(
                "rotate: the vault password",
                // covers: vault recovery rotate
                &["recovery", "rotate"],
                vec![
                    ok(r#"{"recovery":"confirmed"}"#),
                    methods("vault-password"),
                    ok(r#"{"token":"tk-rr-2"}"#),
                    shown.clone(),
                    ok("{}"),
                ],
                b"pw-rotate-9\nw3\nw11\nw20\n",
                0,
            ),
            &["pw-rotate-9", "tk-rr-2"],
        ),
        person(
            "rotate: no words typed back",
            &["recovery", "rotate"],
            vec![
                ok(r#"{"recovery":"confirmed"}"#),
                methods("terminal"),
                ok(r#"{"token":"t"}"#),
                shown.clone(),
                Reply::Answer(
                    400,
                    r#"{"error":{"code":"words_mismatch","hint":"type them again"}}"#.into(),
                ),
            ],
            b"",
            2,
        ),
        traceback(person(
            "show-once: a short phrase",
            &["recovery", "show-once"],
            vec![
                ok(r#"{"recovery":"confirmed"}"#),
                ok(r#"{"id":"r","phrase":"a b c d e"}"#),
                ok("{}"),
            ],
            b"a\nb\nc\n",
            0,
        )),
        traceback(person(
            "show-once: no id, a status Python can't read",
            &["recovery", "show-once"],
            vec![
                ok("not json"),
                methods("local-auth"),
                ok(r#"{"token":"t"}"#),
                ok(r#"{"phrase":"x"}"#),
                ok("{}"),
            ],
            b"",
            0,
        )),
        person(
            "show-once: the vault is closed",
            &["recovery", "show-once"],
            vec![Reply::Answer(
                423,
                r#"{"error":{"code":"vault_locked","hint":"open it"}}"#.into(),
            )],
            b"",
            1,
        ),
        refused("rotate takes no flags", &["recovery", "rotate", "--json"]),
        in_k2,
    ] {
        rig.check(&c);
    }
}

#[test]
fn rollback() {
    let rig = VRig::new();
    let shown = ok(r#"{"secret":{"id":"sec-7"},"placements":[{"target":"file","version":3}]}"#);
    let placed =
        ok(r#"{"secret":{"id":"sec-8"},"placements":[{"target":"systemd_cred","version":2}]}"#);
    for c in [
        secrets(
            person(
                "rollback",
                // covers: vault rollback
                &["rollback", "API", "--to", "2", "--scope", "server"],
                vec![
                    shown.clone(),
                    methods("vault-password"),
                    ok(r#"{"token":"tk-rb-1"}"#),
                    ok("{}"),
                ],
                b"pw-rollback-1\n",
                0,
            ),
            &["pw-rollback-1", "tk-rb-1"],
        ),
        person(
            "rollback: --to as Python's int() reads it",
            &["rollback", "API", "--to", " 0_2", "--accept-agent-value"],
            vec![shown.clone(), methods("local-auth"), ok(r#"{"token":"t"}"#), ok("{}")],
            b"",
            0,
        ),
        secrets(
            person(
                "rollback: root units load it, the plan's confirm",
                &["rollback", "API", "--to", "1", "--accept-agent-value"],
                vec![
                    placed,
                    methods("vault-password"),
                    ok(r#"{"token":"tk-plan-3"}"#),
                    ok("{}"),
                ],
                b"pw-plan-3\n",
                0,
            ),
            &["pw-plan-3", "tk-plan-3"],
        ),
        traceback(person(
            "rollback: a version Python can't compare",
            &["rollback", "API", "--to", "1"],
            vec![
                ok(r#"{"secret":{"id":"s"},"placements":[{"target":"systemd_cred","version":"2"}]}"#),
                methods("local-auth"),
                ok(r#"{"token":"t"}"#),
                ok("{}"),
            ],
            b"",
            0,
        )),
        person(
            "rollback: no such version",
            &["rollback", "API", "--to", "9"],
            vec![
                shown.clone(),
                methods("local-auth"),
                ok(r#"{"token":"t"}"#),
                Reply::Answer(
                    404,
                    r#"{"error":{"code":"not_found","hint":"no v9"}}"#.into(),
                ),
            ],
            b"",
            4,
        ),
        person(
            "rollback: the secret isn't there",
            &["rollback", "API", "--to", "1"],
            vec![Reply::Answer(
                404,
                r#"{"error":{"code":"not_found","hint":"no API"}}"#.into(),
            )],
            b"",
            4,
        ),
        person(
            "rollback: confirm cancelled",
            &["rollback", "API", "--to", "1"],
            vec![shown.clone(), ok(r#"{"methods":[]}"#)],
            b"",
            3,
        ),
        refused("rollback with no --to", &["rollback", "API"]),
        refused("rollback with no name", &["rollback", "--to", "1"]),
        refused("rollback takes no --json", &["rollback", "API", "--to", "1", "--json"]),
    ] {
        rig.check(&c);
    }
}

#[test]
fn backup_and_its_key() {
    let rig = VRig::new();
    let abs = text(&dir(&rig, "bk"));
    std::fs::create_dir_all(rig.proj.join("rel")).unwrap();
    let wrote = ok(r#"{"path":"/b/vault-backup.age","note":"keep it apart"}"#);
    for c in [
        person(
            "backup, a key confirmed before",
            // covers: vault backup
            &["backup", abs],
            vec![ok(r#"{"recovery":"confirmed"}"#), wrote.clone()],
            b"",
            0,
        ),
        person(
            "backup, a folder by its relative name, the first time",
            &["backup", "rel"],
            vec![
                ok(r#"{"recovery":"none"}"#),
                methods("local-auth"),
                ok(r#"{"token":"t"}"#),
                wrote.clone(),
            ],
            b"",
            0,
        ),
        traceback(person(
            "backup: an answer with no path",
            &["backup", abs],
            vec![ok(r#"{"recovery":"confirmed"}"#), ok(r#"{"note":"n"}"#)],
            b"",
            1,
        )),
        person(
            "backup: not a folder",
            &["backup", "/nowhere/at/all"],
            vec![],
            b"",
            2,
        ),
        refused("backup with no folder", &["backup"]),
        refused("backup takes no flags", &["backup", abs, "--json"]),
        secrets(
            person(
                "backup key add",
                // covers: vault backup key
                &["backup", "key", "age1example"],
                vec![
                    ok(r#"{"recovery":"confirmed"}"#),
                    methods("vault-password"),
                    ok(r#"{"token":"tk-bk-1"}"#),
                    ok(r#"{"fingerprint":"fp-new","replaced":"fp-old"}"#),
                ],
                b"pw-bk-1\n",
                0,
            ),
            &["pw-bk-1", "tk-bk-1"],
        ),
        person(
            "backup key add, no recovery key yet",
            &["backup", "key", "age1example"],
            vec![
                ok(r#"{"recovery":"none"}"#),
                methods("local-auth"),
                ok(r#"{"token":"t"}"#),
                ok(r#"{"fingerprint":"fp-new"}"#),
            ],
            b"",
            0,
        ),
        person(
            "backup key remove",
            &["backup", "key", "remove"],
            vec![
                ok(r#"{"holders":[{"kind":"box","fingerprint":"b"},{"kind":"backup","fingerprint":"fp-1"}]}"#),
                methods("local-auth"),
                ok(r#"{"token":"t"}"#),
                ok(r#"{"removed":"fp-1","note":"old backups still open with it"}"#),
            ],
            b"",
            0,
        ),
        person(
            "backup key remove, none there",
            &["backup", "key", "remove"],
            vec![ok(r#"{"holders":[{"kind":"box"}]}"#)],
            b"",
            1,
        ),
        refused("backup key with no key", &["backup", "key"]),
    ] {
        rig.check(&c);
    }
}

#[test]
fn restore() {
    let rig = VRig::new();
    let from = dir(&rig, "from");
    std::fs::write(from.join("vault.db"), b"db").unwrap();
    let from = text(&from);
    let empty = text(&dir(&rig, "empty"));
    let words: &[u8] = b"  alpha bravo charlie-secret delta\n\n";
    for c in [
        secrets(
            person(
                "restore --dry-run, the recovery words",
                // covers: vault restore
                &["restore", "--from", from, "--dry-run"],
                vec![ok(r#"{"dryRun":true,"vaultId":"v-1","secrets":3}"#)],
                words,
                0,
            ),
            &["charlie-secret"],
        ),
        secrets(
            person(
                "restore --replace: a confirm first",
                &["restore", "--from", from, "--replace"],
                vec![
                    methods("local-auth"),
                    ok(r#"{"token":"tk-rs-9"}"#),
                    ok(r#"{"vaultId":"v-1","secrets":2,"previous":"/k/vault.db.prev"}"#),
                ],
                words,
                0,
            ),
            &["charlie-secret", "tk-rs-9"],
        ),
        secrets(
            person(
                "restore with the backup key",
                &["restore", "--from", from, "--holder", "backup"],
                vec![ok(r#"{"vaultId":"v-2","secrets":1,"previous":"p"}"#)],
                b"AGE-SECRET-KEY-1SECRETBYTES",
                0,
            ),
            &["AGE-SECRET-KEY-1SECRETBYTES"],
        ),
        person(
            "restore: a wrong key",
            &["restore", "--from", from, "--holder", "backup"],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"bad_key","hint":"not this vault's"}}"#.into(),
            )],
            b"AGE-SECRET-KEY-1X\n",
            3,
        ),
        traceback(person(
            "restore: a count Python can't print",
            &["restore", "--from", from, "--dry-run"],
            vec![ok(r#"{"dryRun":true,"vaultId":"v","secrets":"three"}"#)],
            b"w\n",
            1,
        )),
        person(
            "restore: nothing on stdin",
            &["restore", "--from", from],
            vec![],
            b"",
            2,
        ),
        person(
            "restore: no vault.db there",
            &["restore", "--from", empty],
            vec![],
            b"w\n",
            2,
        ),
        person(
            "restore: not a folder",
            &["restore", "--from", "/nowhere/at/all"],
            vec![],
            b"w\n",
            2,
        ),
        refused("restore with no --from", &["restore"]),
        refused("restore takes no --json", &["restore", "--from", from, "--json"]),
    ] {
        rig.check(&c);
    }
}

#[test]
fn import_an_env_file() {
    let rig = VRig::new();
    let moved = ok(r#"{"moved":["A","B"],"envK2":"/p/.env.k2","originalRemoved":false}"#);
    for c in [
        daemon(
            "import",
            // covers: vault import
            &["import", ".env", "--scope", "ws:acme", "--delivery", "env"],
            moved.clone(),
            0,
        ),
        daemon(
            "import --remove-original",
            &["import", "sub/.env", "--remove-original"],
            ok(r#"{"moved":[],"envK2":"/p/sub/.env.k2","originalRemoved":true}"#),
            0,
        ),
        // covers: vault import --json
        daemon("import --json", &["import", ".env", "--json"], moved.clone(), 0),
        traceback(daemon(
            "import: names Python can't join",
            &["import", ".env"],
            ok(r#"{"moved":[1]}"#),
            1,
        )),
        daemon(
            "import: refused",
            &["import", "/etc/.env"],
            Reply::Answer(
                400,
                r#"{"error":{"code":"bad_path","hint":"not a file"}}"#.into(),
            ),
            2,
        ),
        refused("import with no file", &["import"]),
        refused("import takes no --ttl", &["import", ".env", "--ttl", "2h"]),
    ] {
        rig.check(&c);
    }
}

#[test]
fn carry_export_and_import() {
    let rig = VRig::new();
    let out = rig.proj.join("carry.age");
    std::fs::write(rig.proj.join("there.age"), b"x").unwrap();
    std::fs::write(rig.proj.join("in.age"), b"sealed bytes \x00\xff").unwrap();
    let export = |name: &'static str, reply: Reply, code: i32| {
        let mut c = daemon(
            name,
            &[
                "carry",
                "export",
                "--recipient",
                "age1target",
                "--target",
                "vault-2",
                "--workspace",
                "w1",
                "--workspace",
                "w2",
                "--server",
                "s1",
                "--target-workspace",
                "tw",
                "--own-box-move",
                "--out",
                "carry.age",
            ],
            reply,
            code,
        );
        c.files = vec![out.clone()];
        c
    };
    let dry = ok(r#"{"dryRun":true,"sourceVaultId":"v1","sourceKeyFingerprint":"k1","ownBoxMove":true,"madeForWorkspaceId":"w9","secrets":[{"scope":"ws:a","name":"A","hosts":["a.com","b.com"],"targetWorkspaceId":"w9","targetWorkspace":"acme"},{"scope":"server","name":"B","hosts":[],"skip":"name taken"},{"scope":"server","name":"C","hosts":["c"]}]}"#);
    for c in [
        // covers: vault carry export
        export(
            "carry export",
            ok(r#"{"carryB64":"c2VhbGVkIGJ5dGVz","count":2,"targetFingerprint":"fp","secrets":["A","B"]}"#),
            0,
        ),
        traceback(export(
            "carry export: bytes Python can't decode",
            ok(r#"{"carryB64":"abc","count":1,"targetFingerprint":"fp","secrets":[]}"#),
            1,
        )),
        export(
            "carry export: refused",
            Reply::Answer(
                403,
                r#"{"error":{"code":"owner_only","hint":"a person moves a vault"}}"#.into(),
            ),
            3,
        ),
        refused(
            "carry export: the file is there",
            &[
                "carry",
                "export",
                "--recipient",
                "r",
                "--target",
                "t",
                "--out",
                "there.age",
            ],
        ),
        refused(
            "carry export with no --out",
            &["carry", "export", "--recipient", "r", "--target", "t"],
        ),
        refused(
            "carry export takes no --json",
            &["carry", "export", "--json"],
        ),
        daemon(
            "carry import, applied",
            // covers: vault carry import
            &["carry", "import", "in.age", "--workspace", "w", "--apply"],
            ok(r#"{"imported":["A","B"],"skipped":["C: name taken"]}"#),
            0,
        ),
        daemon("carry import, a dry run", &["carry", "import", "in.age"], dry.clone(), 0),
        // covers: vault carry import --json
        daemon(
            "carry import --json",
            &["carry", "import", "in.age", "--json"],
            dry.clone(),
            0,
        ),
        traceback(daemon(
            "carry import: an answer with no list",
            &["carry", "import", "in.age"],
            ok(r#"{"imported":[]}"#),
            1,
        )),
        refused("carry import, no such file", &["carry", "import", "nope.age"]),
        refused(
            "carry import, two workspaces",
            &["carry", "import", "in.age", "--workspace", "a", "--workspace", "b"],
        ),
        refused("carry, no verb", &["carry"]),
        refused("carry, an unknown verb", &["carry", "nope", "--json"]),
    ] {
        rig.check(&c);
    }
}
