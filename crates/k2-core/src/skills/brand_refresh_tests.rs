//! A brand change alone refreshes the generated text of every tier.
//!
//! Each test writes a tier's file with its real writer, puts in its place
//! the same file as another brand's build of the same version would have
//! written it (another command and product in the text, that brand's key
//! in the stamp), runs the writer again, and checks the file is this
//! build's again. The other brand's command and product are separate
//! words, unrelated to each other, as a partner's are.

use std::fs;
use std::path::{Path, PathBuf};

use crate::brand::{CLI_NAME, PRODUCT_NAME};
use crate::skills::version::{brand_key, brand_names_key, parse_skill, wrap_managed_skill_for};

/// The other brand's command and product: never this build's.
fn other_cli() -> &'static str {
    if CLI_NAME == "zq" {
        "zr"
    } else {
        "zq"
    }
}

fn other_product() -> &'static str {
    if PRODUCT_NAME == "Globex Desk" {
        "Initech Hub"
    } else {
        "Globex Desk"
    }
}

fn other_key() -> String {
    brand_names_key(other_product(), other_cli(), &[])
}

/// `text` as the other brand writes it: its command where this build's
/// is typed, its product where this build's is named.
fn other_brand_text(text: &str) -> String {
    text.replace(&format!("{CLI_NAME} "), &format!("{} ", other_cli()))
        .replace(PRODUCT_NAME, other_product())
}

/// A managed skill file as the other brand's build of the same version
/// writes it.
fn as_other_brand(file: &str) -> String {
    let p = parse_skill(file);
    let region = p
        .managed_region
        .as_deref()
        .expect("the file has its markers");
    let extras = (!p.extra_frontmatter.is_empty()).then_some(p.extra_frontmatter.as_str());
    wrap_managed_skill_for(
        p.k2_skill.as_deref().expect("the file names its type"),
        p.skill_version.expect("the file names its version"),
        &other_brand_text(region),
        extras,
        &other_key(),
    )
}

fn scratch_project(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "k2-brand-refresh-{tag}-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    fs::create_dir_all(dir.join(".k2/skills")).expect("make the scratch workspace");
    dir
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// The other brand's file at `path` is rewritten by `write` to this
/// build's: the managed text, its checksum, the version and the stamp.
fn assert_refreshes(path: &Path, write: impl Fn()) {
    write();
    let fresh = read(path);
    let other = as_other_brand(&fresh);
    assert_ne!(other, fresh, "the other brand's file differs");
    fs::write(path, &other).expect("write the other brand's file");
    write();
    let after = read(path);
    let (a, f) = (parse_skill(&after), parse_skill(&fresh));
    assert_eq!(a.managed_region, f.managed_region, "{}", path.display());
    assert_eq!(a.skill_checksum, f.skill_checksum, "{}", path.display());
    assert_eq!(a.skill_version, f.skill_version, "{}", path.display());
    assert_eq!(a.brand_key(), brand_key(), "{}", path.display());
    assert!(
        !path.with_extension("md.proposed").exists(),
        "an unedited file is rewritten, not proposed: {}",
        path.display()
    );
}

fn agent_tier(agent_type: &str, tag: &str) {
    let proj = scratch_project(tag);
    let path = proj.to_str().expect("a UTF-8 path");
    let name = format!("{tag}-agent");
    let skill = crate::workspace_dot_dir(path)
        .join("skills")
        .join(format!("k2so-{name}"))
        .join("SKILL.md");
    assert_refreshes(&skill, || {
        crate::skills::writer::write_agent_skill_file(path, &name, agent_type)
    });
    fs::remove_dir_all(&proj).ok();
}

#[test]
fn manager_tier_refreshes_on_a_brand_change() {
    agent_tier("manager", "manager");
}

#[test]
fn k2_agent_tier_refreshes_on_a_brand_change() {
    agent_tier("k2", "planner");
}

#[test]
fn custom_agent_tier_refreshes_on_a_brand_change() {
    agent_tier("custom", "custom");
}

#[test]
fn template_tier_refreshes_on_a_brand_change() {
    agent_tier("agent-template", "template");
}

/// The k2-cli skill (SKILL_VERSION_WORKSPACE) and the canonical-agents
/// skill, both from the workspace regen.
#[test]
fn workspace_and_canonical_agent_tiers_refresh_on_a_brand_change() {
    let proj = scratch_project("workspace");
    let path = proj.to_str().expect("a UTF-8 path");
    let skills = crate::workspace_dot_dir(path).join("skills");
    for dir in ["k2-cli", "k2-canonical-agents"] {
        assert_refreshes(&skills.join(dir).join("SKILL.md"), || {
            crate::workspace::skill_regen::write_workspace_skill_file(path)
        });
    }
    fs::remove_dir_all(&proj).ok();
}

#[test]
fn opt_in_skills_refresh_on_a_brand_change() {
    use crate::skills::content::{write_opt_in_skill, OptInSkill};
    let proj = scratch_project("optin");
    let path = proj.to_str().expect("a UTF-8 path");
    for skill in [
        OptInSkill::WorkspaceManager,
        OptInSkill::K2Agent,
        OptInSkill::K2CanonicalAgent,
    ] {
        let file = write_opt_in_skill(path, skill);
        assert_refreshes(&file, || {
            write_opt_in_skill(path, skill);
        });
    }
    fs::remove_dir_all(&proj).ok();
}

#[test]
fn permissions_tier_refreshes_on_a_brand_change() {
    let proj = scratch_project("permissions");
    let dir = crate::workspace_dot_dir(proj.to_str().expect("a UTF-8 path"))
        .join("skills")
        .join(crate::permission::SKILL_DIR);
    assert_refreshes(&dir.join("SKILL.md"), || {
        crate::workspace::skill_regen::write_permissions_skill(&dir)
    });
    fs::remove_dir_all(&proj).ok();
}

#[test]
fn zen_tier_refreshes_on_a_brand_change() {
    let proj = scratch_project("zen");
    let dot = crate::workspace_dot_dir(proj.to_str().expect("a UTF-8 path"));
    assert_refreshes(&dot.join("skills/k2-zen/SKILL.md"), || {
        assert!(crate::workspace::skill_regen::ensure_zen_skill(&dot, true));
    });
    fs::remove_dir_all(&proj).ok();
}

#[test]
fn chat_tier_refreshes_on_a_brand_change() {
    use crate::sew_chat::teach::{ensure_chat_skill, ChatGates};
    let proj = scratch_project("chat");
    let dot = crate::workspace_dot_dir(proj.to_str().expect("a UTF-8 path"));
    let gates = ChatGates {
        sew_on: true,
        has_access: true,
        manage: true,
    };
    let path = dot.join("skills/k2-chat/SKILL.md");
    assert_refreshes(&path, || {
        ensure_chat_skill(&dot, gates).expect("write the chat skill")
    });
    // The chat skill is written whole: after the refresh it is this
    // build's file, byte for byte.
    let fresh = read(&path);
    fs::write(&path, as_other_brand(&fresh)).expect("write the other brand's file");
    ensure_chat_skill(&dot, gates).expect("write the chat skill");
    assert_eq!(read(&path), fresh);
    fs::remove_dir_all(&proj).ok();
}

/// `.k2/AGENTS.md` is keyed on its text, the compose banner included
/// (without its time), so another brand's compose is rewritten.
#[test]
fn agents_md_refreshes_on_a_brand_change() {
    use crate::workspace::skill_regen::{
        agents_md_refresh_key, compose_agents_md_public, recompose_agents_md,
        write_workspace_skill_file,
    };
    let proj = scratch_project("agents-md");
    let path = proj.to_str().expect("a UTF-8 path");
    write_workspace_skill_file(path);
    let canonical = crate::workspace_dot_dir(path).join("AGENTS.md");
    let fresh = read(&canonical);
    assert!(
        fresh.contains(&format!("`{CLI_NAME} agent context regen`")),
        "the banner names the command: {fresh}"
    );

    // The whole compose as the other brand wrote it.
    fs::write(&canonical, other_brand_text(&fresh)).expect("write the other brand's compose");
    recompose_agents_md(path);
    assert_eq!(
        agents_md_refresh_key(&read(&canonical)),
        agents_md_refresh_key(&compose_agents_md_public(path))
    );

    // Only the banner's command differs: still rewritten.
    let banner_only = fresh.replacen(
        &format!("`{CLI_NAME} agent context regen`"),
        &format!("`{} agent context regen`", other_cli()),
        1,
    );
    assert_ne!(banner_only, fresh);
    fs::write(&canonical, &banner_only).expect("write the other banner");
    recompose_agents_md(path);
    assert!(
        read(&canonical).contains(&format!("`{CLI_NAME} agent context regen`")),
        "the banner is this build's again"
    );

    // Nothing new: the file is left alone (its compose time too).
    let settled = read(&canonical);
    recompose_agents_md(path);
    assert_eq!(read(&canonical), settled);
    fs::remove_dir_all(&proj).ok();
}
