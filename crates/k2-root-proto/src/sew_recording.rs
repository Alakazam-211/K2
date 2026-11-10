//! Sew recording on K2 (0.46.11, K-a): the recorder and transcriber
//! sidecars and the speech model, installed through the root helper's
//! `engine sew recording …` verbs (protocol 15, `namespaces.sewrecording`).
//!
//! **Trust.** The helper stages a blob only when its sha256 is this build's
//! pin: the release's `sew-recorder` or `sew-transcriber` tarball (and its
//! binary member), or the speech model K2 pins for the release
//! ([`asr_models`], from Sew's `release.json` `models[]`). Anything else is
//! refused before it is written, and a model that isn't the pinned one is
//! a hard failure: no install, no other model, no retry (the Kessel
//! Steward's condition R28: models are Sew release assets, MIT and
//! Apache-2.0 sources only). Sew checks the tarballs and the model against
//! its own pins again.
//!
//! **Argv.** The helper's argv is enum names only (`engine sew recording
//! stage|install|status|plan|upgrade|uninstall`). The model name reaches the
//! helper as JSON on stdin ([`RecordingRequest`]); paths are the helper's
//! own fixed staging files. Sew's argv is built in ONE place, [`sew_argv`],
//! so Sew's final 0.9.0 flags change one function.
//!
//! **Sew's verbs** (Sew 0.9.0's final argv, its release note of 2026-11-10,
//! checked against the 0.9.0 binary's own `--help`): `sew recording plan
//! --model <name>` (read-only; writes nothing), `sew recording install
//! --recorder-tarball <f> --transcriber-tarball <f> --model-file <f> --model
//! <name> --no-start`, then `sew recording enable` (it sets transcribe too);
//! `status`; `upgrade --recorder-tarball <f> --transcriber-tarball <f>
//! --when-idle` (after `sew upgrade`, only where recording is installed);
//! `uninstall -y` (it stops the units itself). K2 never runs `sew recording
//! disable`: K2's disable flips the org policy only. Every one takes the
//! global `--i-am-k2`, as Sew's final argv names it (Sew refuses the
//! guarded ones without it on a box marked K2-managed, exit 3 `k2_managed`;
//! on the read-only ones it changes nothing); the helper adds `--json` to
//! every Sew run.

use serde::{Deserialize, Serialize};

use crate::sew::{checksum_is_placeholder, hex64, pinned, SewArtifact, SewRelease, SewSidecar};

// ── fixed paths ─────────────────────────────────────────────────────────

/// The staged, pinned sidecar tarballs and model (root 0600 under the
/// helper's 0700 staging dir; removed after a successful install).
pub const SEW_STAGE_RECORDER_TARBALL: &str = "/var/lib/k2-root-helper/sew/sew-recorder.tar.gz";
pub const SEW_STAGE_TRANSCRIBER_TARBALL: &str = "/var/lib/k2-root-helper/sew/sew-transcriber.tar.gz";
/// Sew copies the model to `/usr/local/share/sew/asr/<its own file name>`,
/// so the staged name doesn't matter.
pub const SEW_STAGE_ASR_MODEL: &str = "/var/lib/k2-root-helper/sew/asr-model.gguf";

/// The binary member of each sidecar tarball.
pub const RECORDER_MEMBER: &str = "sew-recorder";
pub const TRANSCRIBER_MEMBER: &str = "sew-transcriber";

pub const UNIT_RECORDER: &str = "sew-recorder.service";
pub const UNIT_TRANSCRIBER: &str = "sew-transcriber.service";

/// What Sew's install writes beside each sidecar (`{version, line,
/// tarball_sha256, binary_sha256}`). Read by `engine sew recording status`
/// until Sew's own status names the versions (Sew 2026-11-10).
pub const RECORDER_VERSION_JSON: &str = "/usr/local/lib/sew/recorder/version.json";
pub const TRANSCRIBER_VERSION_JSON: &str = "/usr/local/lib/sew/transcriber/version.json";
pub const MAX_VERSION_JSON: usize = 4096;

/// Each sidecar's licence files, from its own pinned tarball (as
/// `sew-media`'s go under `/usr/share/doc/sew-media`).
pub const SEW_RECORDER_DOC_DIR: &str = "/usr/share/doc/sew-recorder";
pub const SEW_TRANSCRIBER_DOC_DIR: &str = "/usr/share/doc/sew-transcriber";

// ── the rules K2 checks before asking Sew (docs/recording.md) ──────────

/// Recording is refused below 8 GiB of RAM.
pub const MIN_RAM_BYTES: u64 = 8 * 1024 * 1024 * 1024;
/// The recorder's `MemoryMax` (Sew's placeholder until measured).
pub const RECORDER_MEMORY_MAX_MIB: u64 = 512;
/// On a `shared` (K2) box, K2's own SQL sidecar may take this share of RAM
/// (the fence of Sew's fit rule).
pub const SHARED_FENCE_PCT: u64 = 40;
/// Kept free beyond every unit's `MemoryMax`.
pub const FIT_SLACK_MIB: u64 = 512;
/// The largest speech model the helper stages (Sew's `base-q8_0` is about
/// 83 MB; `small-q8_0` doesn't fit a shared 8 GiB box).
pub const MAX_ASR_MODEL: usize = 512 * 1024 * 1024;

/// Sew's fit rule on a `shared` box: every Sew unit's `MemoryMax` at once
/// must fit in RAM − the 40% fence − 512 MiB. `units_mib` = sew-db + sew +
/// sew-media; the recorder and the model are added here. `(fits, sum,
/// budget)`, all MiB.
pub fn shared_fit(ram_mib: u64, units_mib: u64, model_mib: u64) -> (bool, u64, u64) {
    let budget = ram_mib.saturating_sub(ram_mib * SHARED_FENCE_PCT / 100).saturating_sub(FIT_SLACK_MIB);
    let sum = units_mib + RECORDER_MEMORY_MAX_MIB + model_mib;
    (sum <= budget, sum, budget)
}

// ── the speech model pin ────────────────────────────────────────────────

/// One speech model of one Sew release (`release.json` `models[]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SewAsrModel {
    /// The Sew release that ships it (its core version).
    pub sew_version: &'static str,
    /// Sew's model name (`--model <name>`).
    pub name: &'static str,
    pub backend: &'static str,
    /// The release asset's file name.
    pub file: &'static str,
    pub sha256: &'static str,
    pub size: u64,
    /// The transcriber's `MemoryMax` with this model (Sew's pin
    /// `memory_max_mib`; 0 = not measured, never installable).
    pub memory_max_mib: u64,
}

impl SewAsrModel {
    fn usable(&self) -> bool {
        hex64(self.sha256) && self.size > 0 && self.size <= MAX_ASR_MODEL as u64 && self.memory_max_mib > 0
    }
}

/// The model K2 installs, per Sew release: `base-q8_0` (candle), the one
/// that fits a shared 8 GiB box (Sew 2026-11-10). Never `auto`: K2 stages
/// the file before Sew picks.
///
/// **Placeholder** until Sew publishes the model as a release asset
/// (pending the Kessel Steward's licence check) and fills in its measured
/// memory: the stage step refuses cleanly ([`MODEL_UNPINNED`]). On pin day,
/// copy the row from that release's `release.json` `models[]`.
pub const SEW_ASR_MODELS: &[SewAsrModel] = &[SewAsrModel {
    sew_version: "0.9.0",
    name: "base-q8_0",
    backend: "candle",
    file: "sew-asr-base-q8_0.gguf",
    sha256: "PLACEHOLDER-sew-0.9.0-models-not-published-yet",
    size: 0,
    memory_max_mib: 0,
}];

/// K2's fixture model for the `test-signer` fixture release 0.0.2
/// (`testdata/sew/sew-asr-test-q8_0.gguf`, never a real model; made by
/// `testdata/sew/make-recording-fixtures.py`).
pub const TEST_ASR_MODELS: &[SewAsrModel] = &[SewAsrModel {
    sew_version: "0.0.2",
    name: "test-q8_0",
    backend: "candle",
    file: "sew-asr-test-q8_0.gguf",
    sha256: "4ff03f6b28aec08b02957c8b8692db4aa50f4d3c61e1cca61aa659ad9e816330",
    size: 3200,
    memory_max_mib: 640,
}];

/// The model table this build uses (fixtures under `test-signer`).
pub fn asr_models() -> &'static [SewAsrModel] {
    if cfg!(feature = "test-signer") {
        TEST_ASR_MODELS
    } else {
        SEW_ASR_MODELS
    }
}

/// The code a missing or placeholder model pin answers.
pub const MODEL_UNPINNED: &str = "recording_model_unpinned";

/// The model K2 installs with the pinned Sew release.
pub fn pinned_asr_model() -> Result<&'static SewAsrModel, String> {
    model_in(asr_models(), pinned().version)
}

/// The model `models` pins for Sew `version` (pure).
pub fn model_in(models: &'static [SewAsrModel], version: &str) -> Result<&'static SewAsrModel, String> {
    let m = models.iter().find(|m| m.sew_version == version).ok_or_else(|| {
        format!("{MODEL_UNPINNED}: this K2 build pins no speech model for Sew {version} (recording needs Sew 0.9.0 or later)")
    })?;
    if !m.usable() {
        return Err(format!(
            "{MODEL_UNPINNED}: this K2 build has no speech model pin for Sew {version} yet (Sew hasn't published {} with its sha256 and measured memory)",
            m.file
        ));
    }
    Ok(m)
}

// ── the sidecar pins ────────────────────────────────────────────────────

/// Which sidecar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sidecar {
    Recorder,
    Transcriber,
}

impl Sidecar {
    pub const BOTH: [Sidecar; 2] = [Sidecar::Recorder, Sidecar::Transcriber];

    pub fn member(self) -> &'static str {
        match self {
            Sidecar::Recorder => RECORDER_MEMBER,
            Sidecar::Transcriber => TRANSCRIBER_MEMBER,
        }
    }
    pub fn staged(self) -> &'static str {
        match self {
            Sidecar::Recorder => SEW_STAGE_RECORDER_TARBALL,
            Sidecar::Transcriber => SEW_STAGE_TRANSCRIBER_TARBALL,
        }
    }
    pub fn doc_dir(self) -> &'static str {
        match self {
            Sidecar::Recorder => SEW_RECORDER_DOC_DIR,
            Sidecar::Transcriber => SEW_TRANSCRIBER_DOC_DIR,
        }
    }
    pub fn version_json(self) -> &'static str {
        match self {
            Sidecar::Recorder => RECORDER_VERSION_JSON,
            Sidecar::Transcriber => TRANSCRIBER_VERSION_JSON,
        }
    }
    /// `recorder` / `transcriber`.
    pub fn key(self) -> &'static str {
        match self {
            Sidecar::Recorder => "recorder",
            Sidecar::Transcriber => "transcriber",
        }
    }
}

/// The pinned (version, artifact) of a sidecar for `arch`.
pub fn pinned_sidecar(which: Sidecar, arch: &str) -> Result<(&'static str, &'static SewArtifact), String> {
    sidecar_in(pinned(), which, arch)
}

/// [`pinned_sidecar`] of any release (pure).
pub fn sidecar_in(rel: &'static SewRelease, which: Sidecar, arch: &str) -> Result<(&'static str, &'static SewArtifact), String> {
    let side: &'static SewSidecar = match which {
        Sidecar::Recorder => rel.recorder.as_ref(),
        Sidecar::Transcriber => rel.transcriber.as_ref(),
    }
    .ok_or_else(|| format!("this build pins no {} for Sew {}", which.member(), rel.version))?;
    let a = side
        .artifacts
        .iter()
        .find(|a| a.arch == arch)
        .ok_or_else(|| format!("unsupported CPU architecture '{arch}' for {}", which.member()))?;
    if checksum_is_placeholder(a) {
        return Err(format!("refusing to install: this build has no {} pin (placeholder checksum)", which.member()));
    }
    Ok((side.version, a))
}

/// What a staged blob is, by its sha256: one of the pinned sidecar
/// tarballs or the pinned model. `None`: not this build's pin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blob {
    Sidecar(Sidecar),
    Model,
}

pub fn classify(sha256_hex: &str, arch: &str) -> Option<Blob> {
    classify_in(pinned(), asr_models(), sha256_hex, arch)
}

/// [`classify`] against one release and model table (pure).
pub fn classify_in(rel: &'static SewRelease, models: &'static [SewAsrModel], sha256_hex: &str, arch: &str) -> Option<Blob> {
    for s in Sidecar::BOTH {
        if sidecar_in(rel, s, arch).is_ok_and(|(_, a)| a.tarball_sha256.eq_ignore_ascii_case(sha256_hex)) {
            return Some(Blob::Sidecar(s));
        }
    }
    model_in(models, rel.version).ok().filter(|m| m.sha256.eq_ignore_ascii_case(sha256_hex)).map(|_| Blob::Model)
}

// ── stdin input ─────────────────────────────────────────────────────────

/// `engine sew recording install|plan`'s stdin: the model Sew installs. It
/// must be the pinned model (the only one the helper stages).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordingRequest {
    pub model: String,
}

impl RecordingRequest {
    pub fn validate(&self) -> Result<&'static SewAsrModel, String> {
        self.validate_against(pinned_asr_model()?)
    }

    /// Against a given pinned model (pure).
    pub fn validate_against(&self, m: &'static SewAsrModel) -> Result<&'static SewAsrModel, String> {
        if self.model != m.name {
            return Err(format!("model refused: {:?} is not the speech model this build pins ({})", self.model, m.name));
        }
        Ok(m)
    }
}

// ── Sew's argv (the ONE place) ──────────────────────────────────────────

/// The global flag a K2-managed Sew wants on its guarded verbs.
pub const I_AM_K2: &str = "--i-am-k2";

/// One Sew step the recording verbs run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SewStep<'a> {
    Install { recorder: &'a str, transcriber: &'a str, model_file: &'a str, model: &'a str },
    Enable,
    Status,
    Plan { model: &'a str },
    Upgrade { recorder: &'a str, transcriber: &'a str },
    Uninstall,
}

/// Sew's argv for a step (the helper appends `--json`).
pub fn sew_argv(step: SewStep) -> Vec<String> {
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    match step {
        SewStep::Install { recorder, transcriber, model_file, model } => s(&[
            "recording",
            "install",
            "--recorder-tarball",
            recorder,
            "--transcriber-tarball",
            transcriber,
            "--model-file",
            model_file,
            "--model",
            model,
            "--no-start",
            I_AM_K2,
        ]),
        SewStep::Enable => s(&["recording", "enable", I_AM_K2]),
        SewStep::Status => s(&["recording", "status", I_AM_K2]),
        SewStep::Plan { model } => s(&["recording", "plan", "--model", model, I_AM_K2]),
        SewStep::Upgrade { recorder, transcriber } => {
            s(&["recording", "upgrade", "--recorder-tarball", recorder, "--transcriber-tarball", transcriber, "--when-idle", I_AM_K2])
        }
        SewStep::Uninstall => s(&["recording", "uninstall", "-y", I_AM_K2]),
    }
}

/// One sidecar's `version.json` as Sew wrote it (other keys ignored).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SidecarVersion {
    pub version: String,
    #[serde(default)]
    pub tarball_sha256: Option<String>,
    #[serde(default)]
    pub binary_sha256: Option<String>,
}

/// A `version.json`, strictly: at most [`MAX_VERSION_JSON`] bytes, a plain
/// `x.y.z` version. Root's file, read as untrusted input.
pub fn parse_sidecar_version(bytes: &[u8]) -> Option<SidecarVersion> {
    if bytes.len() > MAX_VERSION_JSON {
        return None;
    }
    let v: SidecarVersion = serde_json::from_slice(bytes).ok()?;
    crate::version::Semver::parse(&v.version)?;
    Some(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_release() -> &'static SewRelease {
        crate::sew::TEST_RELEASES.last().expect("fixture releases")
    }

    #[test]
    fn the_real_model_pin_is_a_placeholder_so_the_stage_step_refuses() {
        let m = SEW_ASR_MODELS.iter().find(|m| m.sew_version == "0.9.0").expect("a 0.9.0 row");
        assert_eq!((m.name, m.backend), ("base-q8_0", "candle"), "Sew: base-q8_0 for a shared 8 GiB box, never auto");
        assert!(SEW_ASR_MODELS.iter().all(|m| m.name != "auto"));
        let e = model_in(SEW_ASR_MODELS, "0.9.0").unwrap_err();
        assert!(e.starts_with(MODEL_UNPINNED) && e.contains("sew-asr-base-q8_0.gguf"), "{e}");
        // The real pin is 0.9.0 and its release.json has no models[] yet
        // (Sew 2026-11-10: the model waits on a licence point), so its row
        // stays the placeholder and every model stage refuses.
        let pin = crate::sew::SEW_RELEASES.last().expect("real pins");
        assert_eq!(pin.version, "0.9.0");
        let e = model_in(SEW_ASR_MODELS, pin.version).unwrap_err();
        assert!(e.starts_with(MODEL_UNPINNED) && e.contains("hasn't published"), "{e}");
        // An older Sew has no row at all.
        let e = model_in(SEW_ASR_MODELS, "0.8.3").unwrap_err();
        assert!(e.starts_with(MODEL_UNPINNED) && e.contains("recording needs Sew 0.9.0"), "{e}");
    }

    #[test]
    fn the_fixture_release_pins_both_sidecars_and_a_model() {
        let rel = fixture_release();
        let (v, r) = sidecar_in(rel, Sidecar::Recorder, "x86_64").expect("recorder");
        let (_, t) = sidecar_in(rel, Sidecar::Transcriber, "x86_64").expect("transcriber");
        assert_eq!(v, "0.0.2");
        let c = |sha: &str| classify_in(rel, TEST_ASR_MODELS, sha, "x86_64");
        assert_eq!(c(r.tarball_sha256), Some(Blob::Sidecar(Sidecar::Recorder)));
        assert_eq!(c(&t.tarball_sha256.to_uppercase()), Some(Blob::Sidecar(Sidecar::Transcriber)));
        let m = model_in(TEST_ASR_MODELS, rel.version).expect("model");
        assert_eq!(c(m.sha256), Some(Blob::Model));
        assert_eq!(c(&"0".repeat(64)), None, "anything else is no pin");
        assert_eq!(c(r.binary_sha256), None, "a bare binary is not a tarball");
        assert!(sidecar_in(rel, Sidecar::Recorder, "riscv64").is_err());
        // The real pin's sidecars are known; its model isn't, so no blob is one.
        let real = crate::sew::SEW_RELEASES.last().expect("real pins");
        let (_, rr) = sidecar_in(real, Sidecar::Recorder, "x86_64").expect("the real pin pins the recorder");
        assert_eq!(classify_in(real, SEW_ASR_MODELS, rr.tarball_sha256, "x86_64"), Some(Blob::Sidecar(Sidecar::Recorder)));
    }

    /// The fixture files match the fixture pins byte for byte.
    #[test]
    fn the_fixture_files_hash_to_the_fixture_pins() {
        use sha2::Digest;
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/sew");
        let sha = |f: &str| -> String {
            let b = std::fs::read(dir.join(f)).unwrap_or_else(|e| panic!("{f}: {e}"));
            sha2::Sha256::digest(&b).iter().map(|x| format!("{x:02x}")).collect()
        };
        let rel = fixture_release();
        for s in Sidecar::BOTH {
            let f = format!("{}-0.0.2-x86_64-unknown-linux-gnu.tar.gz", s.member());
            assert_eq!(sha(&f), sidecar_in(rel, s, "x86_64").expect("pinned").1.tarball_sha256, "{f}");
        }
        let m = &TEST_ASR_MODELS[0];
        assert_eq!(sha(m.file), m.sha256);
        assert_eq!(std::fs::metadata(dir.join(m.file)).expect("model").len(), m.size);
    }

    #[test]
    fn the_request_names_only_the_pinned_model() {
        assert!(serde_json::from_str::<RecordingRequest>(r#"{"model":"x","path":"/etc/shadow"}"#).is_err(), "unknown keys refused");
        let m = &TEST_ASR_MODELS[0];
        assert!(RecordingRequest { model: "test-q8_0".into() }.validate_against(m).is_ok());
        for bad in ["auto", "small-q8_0", "", "../x"] {
            assert!(RecordingRequest { model: bad.into() }.validate_against(m).is_err(), "{bad}");
        }
    }

    #[test]
    fn sews_argv_carries_i_am_k2_on_every_guarded_verb_and_never_auto() {
        let inst = sew_argv(SewStep::Install { recorder: "/r.tgz", transcriber: "/t.tgz", model_file: "/m.gguf", model: "base-q8_0" });
        assert_eq!(
            inst,
            [
                "recording",
                "install",
                "--recorder-tarball",
                "/r.tgz",
                "--transcriber-tarball",
                "/t.tgz",
                "--model-file",
                "/m.gguf",
                "--model",
                "base-q8_0",
                "--no-start",
                "--i-am-k2"
            ]
        );
        for step in [SewStep::Enable, SewStep::Uninstall, SewStep::Upgrade { recorder: "/r", transcriber: "/t" }] {
            assert!(sew_argv(step).iter().any(|a| a == I_AM_K2), "{step:?}");
        }
        assert_eq!(sew_argv(SewStep::Uninstall), ["recording", "uninstall", "-y", "--i-am-k2"]);
        // Sew 0.9.0's final argv: the read-only verbs carry it too.
        assert_eq!(sew_argv(SewStep::Status), ["recording", "status", "--i-am-k2"]);
        assert_eq!(sew_argv(SewStep::Plan { model: "base-q8_0" }), ["recording", "plan", "--model", "base-q8_0", "--i-am-k2"]);
        assert_eq!(
            sew_argv(SewStep::Upgrade { recorder: "/r", transcriber: "/t" }),
            ["recording", "upgrade", "--recorder-tarball", "/r", "--transcriber-tarball", "/t", "--when-idle", "--i-am-k2"]
        );
        assert_eq!(sew_argv(SewStep::Enable), ["recording", "enable", "--i-am-k2"]);
        assert!(!inst.iter().any(|a| a == "auto"));
    }

    #[test]
    fn the_fit_rule_matches_sews_table() {
        // docs/recording.md: shared 8 GiB, voice's units 3,200 MiB.
        let (fits, sum, budget) = shared_fit(8192, 1216 + 1216 + 768, 692);
        assert_eq!((sum, budget), (4404, 8192 - 3276 - 512));
        assert!(fits, "base-q8_0 at ≤ 692 MiB fits");
        assert!(!shared_fit(8192, 3200, 1075).0, "small-q8_0 does not");
    }

    #[test]
    fn sidecar_versions_are_read_strictly() {
        let ok = br#"{"version":"0.9.0","line":"0.9","tarball_sha256":"aa","binary_sha256":"bb"}"#;
        assert_eq!(parse_sidecar_version(ok).expect("parses").version, "0.9.0");
        assert!(parse_sidecar_version(br#"{"version":"0.9.0-rc1"}"#).is_none());
        assert!(parse_sidecar_version(&vec![b' '; MAX_VERSION_JSON + 1]).is_none());
        assert!(parse_sidecar_version(b"not json").is_none());
    }
}
