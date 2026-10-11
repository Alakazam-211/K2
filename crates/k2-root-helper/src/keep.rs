//! `keep` namespace (§7.5, protocol 4): the vault keeper. Holds only the
//! vault KEK (32 random bytes) and a rollback counter, per daemon user, in
//! `/var/lib/k2-root-helper/keep/<daemon user>/` (dirs 0700, files 0600,
//! root-owned, temp + fsync + rename, never through a link). The daemon
//! user comes from the caller check, never from argv or env.
//!
//! Rotation is two-phase (RH20): `kek stage` makes and prints `kek.next`;
//! the daemon re-wraps and verifies the VDK; then `kek commit` (kek →
//! kek.prev, kek.next → kek) or `kek abort`.

use std::ffi::OsStr;

use k2_root_proto::paths::KEEP_DIR;
use k2_root_proto::PROTOCOL;

use crate::ctx::{Ctx, Fail, HResult};
use crate::fsx::{Dir, Kind};

const KEK: &str = "kek";
const NEXT: &str = "kek.next";
const PREV: &str = "kek.prev";
const GEN: &str = "gen";
const KEK_LEN: usize = 32;

fn user_ok(u: &str) -> bool {
    let b = u.as_bytes();
    !b.is_empty()
        && b.len() <= 32
        && (b[0].is_ascii_lowercase() || b[0] == b'_')
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_' || *c == b'-')
}

/// Create `name` 0700 root under `d` if missing; refuse a link or a file.
fn ensure_private_dir(ctx: &Ctx, d: &Dir, name: &OsStr) -> HResult<Dir> {
    match d.kind(name).map_err(|e| Fail::io_err("stat", e))? {
        Kind::Missing => d.mkdir(name, 0o700).map_err(|e| Fail::io_err("mkdir", e))?,
        Kind::Dir => {}
        _ => return Err(Fail::state("keep refused: store path is not a directory")),
    }
    let sub = d.sub(name).map_err(|e| Fail::io_err("keep refused", e))?;
    if let Some((u, g)) = ctx.root_owner() {
        sub.fchown(u, g).map_err(|e| Fail::io_err("chown", e))?;
    }
    sub.fchmod(0o700).map_err(|e| Fail::io_err("chmod", e))?;
    Ok(sub)
}

/// The store dir for `user`; created on demand only when `create`.
fn store(ctx: &Ctx, user: &str, create: bool) -> HResult<Option<Dir>> {
    if !user_ok(user) {
        return Err(Fail::state("keep refused: daemon user"));
    }
    let (parent, name) = crate::mail::split(KEEP_DIR);
    let (gp, state_name) = crate::mail::split(parent);
    let base = ctx.dir(gp).map_err(|e| Fail::io_err("open /var/lib", e))?;
    let state = if create {
        ensure_private_dir(ctx, &base, OsStr::new(state_name))?
    } else {
        match base
            .kind(OsStr::new(state_name))
            .map_err(|e| Fail::io_err("stat", e))?
        {
            Kind::Missing => return Ok(None),
            Kind::Dir => base
                .sub(OsStr::new(state_name))
                .map_err(|e| Fail::io_err("keep refused", e))?,
            _ => return Err(Fail::state("keep refused: state path is not a directory")),
        }
    };
    let keep = if create {
        ensure_private_dir(ctx, &state, OsStr::new(name))?
    } else {
        match state
            .kind(OsStr::new(name))
            .map_err(|e| Fail::io_err("stat", e))?
        {
            Kind::Missing => return Ok(None),
            Kind::Dir => state
                .sub(OsStr::new(name))
                .map_err(|e| Fail::io_err("keep refused", e))?,
            _ => return Err(Fail::state("keep refused: store path is not a directory")),
        }
    };
    if create {
        return Ok(Some(ensure_private_dir(ctx, &keep, OsStr::new(user))?));
    }
    match keep
        .kind(OsStr::new(user))
        .map_err(|e| Fail::io_err("stat", e))?
    {
        Kind::Missing => Ok(None),
        Kind::Dir => Ok(Some(
            keep.sub(OsStr::new(user))
                .map_err(|e| Fail::io_err("keep refused", e))?,
        )),
        _ => Err(Fail::state("keep refused: store path is not a directory")),
    }
}

fn present(d: &Option<Dir>, name: &str) -> HResult<bool> {
    match d {
        None => Ok(false),
        Some(d) => match d
            .kind(OsStr::new(name))
            .map_err(|e| Fail::io_err("stat", e))?
        {
            Kind::Missing => Ok(false),
            Kind::File => Ok(true),
            _ => Err(Fail::state(
                "keep refused: store entry is not a regular file",
            )),
        },
    }
}

fn read_kek(d: &Dir, name: &str) -> HResult<Vec<u8>> {
    let b = d
        .read_file(OsStr::new(name), 64)
        .map_err(|e| Fail::io_err("read kek", e))?
        .ok_or_else(|| Fail::state("keep: no kek"))?;
    if b.len() != KEK_LEN {
        return Err(Fail::state("keep: kek corrupt"));
    }
    Ok(b)
}

fn read_gen(d: &Option<Dir>) -> HResult<u64> {
    let Some(d) = d else { return Ok(0) };
    match d
        .read_file(OsStr::new(GEN), 32)
        .map_err(|e| Fail::io_err("read gen", e))?
    {
        None => Ok(0),
        Some(b) => std::str::from_utf8(&b)
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .ok_or_else(|| Fail::state("keep: gen corrupt")),
    }
}

fn new_kek(ctx: &Ctx) -> HResult<Vec<u8>> {
    let mut k = vec![0u8; KEK_LEN];
    ctx.rand
        .fill(&mut k)
        .map_err(|e| Fail::io_err("getrandom", e))?;
    if k.iter().all(|&b| b == 0) {
        return Err(Fail::io("getrandom returned zeros"));
    }
    Ok(k)
}

fn write(ctx: &Ctx, d: &Dir, name: &str, bytes: &[u8]) -> HResult<()> {
    d.write_atomic(OsStr::new(name), bytes, 0o600, ctx.root_owner())
        .map_err(|e| Fail::io_err("keep write", e))
}

pub fn status(ctx: &Ctx, user: Option<&str>) -> HResult<serde_json::Value> {
    let d = match user {
        Some(u) => store(ctx, u, false)?,
        None => None,
    };
    let word = |b: bool| if b { "present" } else { "absent" };
    Ok(serde_json::json!({
        "protocol": PROTOCOL,
        "kek": word(present(&d, KEK)?),
        "next": word(present(&d, NEXT)?),
        "gen": read_gen(&d)?,
        "sealed": "none",
    }))
}

/// Generate the KEK; refuse when one exists. Prints nothing.
pub fn kek_init(ctx: &Ctx, user: &str) -> HResult<()> {
    let d = store(ctx, user, true)?.ok_or_else(|| Fail::io("keep: store not created"))?;
    if d.kind(OsStr::new(KEK))
        .map_err(|e| Fail::io_err("stat", e))?
        != Kind::Missing
    {
        return Err(Fail::state("keep: a kek already exists"));
    }
    let k = new_kek(ctx)?;
    write(ctx, &d, KEK, &k)
}

/// The 32-byte KEK on stdout.
pub fn kek_get(ctx: &Ctx, user: &str) -> HResult<()> {
    let d = store(ctx, user, false)?.ok_or_else(|| Fail::state("keep: no kek"))?;
    let k = read_kek(&d, KEK)?;
    ctx.print(&k);
    Ok(())
}

/// Rotation phase 1: make `kek.next` (replacing an abandoned one) and print it.
pub fn kek_stage(ctx: &Ctx, user: &str) -> HResult<()> {
    let d = store(ctx, user, false)?.ok_or_else(|| Fail::state("keep: no kek"))?;
    read_kek(&d, KEK)?;
    let k = new_kek(ctx)?;
    write(ctx, &d, NEXT, &k)?;
    ctx.print(&k);
    Ok(())
}

/// Rotation phase 2: kek → kek.prev, kek.next → kek.
pub fn kek_commit(ctx: &Ctx, user: &str) -> HResult<()> {
    let d = store(ctx, user, false)?.ok_or_else(|| Fail::state("keep: no kek"))?;
    read_kek(&d, KEK)?;
    read_kek(&d, NEXT).map_err(|_| Fail::state("keep: nothing staged"))?;
    if d.kind(OsStr::new(PREV))
        .map_err(|e| Fail::io_err("stat", e))?
        == Kind::File
    {
        d.unlink(OsStr::new(PREV))
            .map_err(|e| Fail::io_err("keep", e))?;
    }
    d.rename(OsStr::new(KEK), OsStr::new(PREV))
        .map_err(|e| Fail::io_err("keep", e))?;
    d.rename(OsStr::new(NEXT), OsStr::new(KEK))
        .map_err(|e| Fail::io_err("keep", e))?;
    d.fsync().map_err(|e| Fail::io_err("keep", e))
}

/// Drop `kek.next`.
pub fn kek_abort(ctx: &Ctx, user: &str) -> HResult<()> {
    let Some(d) = store(ctx, user, false)? else {
        return Ok(());
    };
    match d
        .kind(OsStr::new(NEXT))
        .map_err(|e| Fail::io_err("stat", e))?
    {
        Kind::Missing => Ok(()),
        Kind::File => d
            .unlink(OsStr::new(NEXT))
            .map_err(|e| Fail::io_err("keep", e)),
        _ => Err(Fail::state(
            "keep refused: store entry is not a regular file",
        )),
    }
}

pub fn gen_get(ctx: &Ctx, user: Option<&str>) -> HResult<serde_json::Value> {
    let d = match user {
        Some(u) => store(ctx, u, false)?,
        None => None,
    };
    Ok(serde_json::json!({ "gen": read_gen(&d)? }))
}

pub fn gen_bump(ctx: &Ctx, user: &str) -> HResult<serde_json::Value> {
    let d = store(ctx, user, true)?;
    let next = read_gen(&d)?
        .checked_add(1)
        .ok_or_else(|| Fail::state("keep: gen overflow"))?;
    let d = d.ok_or_else(|| Fail::io("keep: store not created"))?;
    write(ctx, &d, GEN, next.to_string().as_bytes())?;
    Ok(serde_json::json!({ "gen": next }))
}
