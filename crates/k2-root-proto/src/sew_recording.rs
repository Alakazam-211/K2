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
/// helper's 0700 staging dir; removed after a successful install). The
/// model's licence files are staged beside it ([`staged_licence`]).
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
/// The speech model's licence texts and NOTICE (separate release assets,
/// not inside a tarball), written before Sew installs the model, so the
/// model is never on the box without its attribution; removed with the
/// recorder.
pub const SEW_ASR_DOC_DIR: &str = "/usr/share/doc/sew-asr";
/// The largest licence file the helper stages (Sew's are about 11 KB).
pub const MAX_ASR_LICENCE: usize = 256 * 1024;

/// Where the helper stages one pinned licence file: beside the staged
/// model, under its own pinned file name (from [`SewAsrModel::licences`],
/// never from input).
pub fn staged_licence(file: &str) -> String {
    format!("/var/lib/k2-root-helper/sew/{file}")
}

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

/// One licence file of a speech model (a release asset beside the model:
/// `sew-<v>-models.json` `license_files[]` and `notice`, hashes from
/// `SHA256SUMS-models`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SewAsrLicence {
    pub file: &'static str,
    pub sha256: &'static str,
    pub size: u64,
}

impl SewAsrLicence {
    fn usable(&self) -> bool {
        hex64(self.sha256)
            && self.size > 0
            && self.size <= MAX_ASR_LICENCE as u64
            && !self.file.is_empty()
            && self.file.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
            && !self.file.starts_with('.')
    }
}

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
    /// Its licence texts and NOTICE, staged and installed with it. A model
    /// without them is never installable.
    pub licences: &'static [SewAsrLicence],
}

impl SewAsrModel {
    fn usable(&self) -> bool {
        hex64(self.sha256)
            && self.size > 0
            && self.size <= MAX_ASR_MODEL as u64
            && self.memory_max_mib > 0
            && !self.licences.is_empty()
            && self.licences.iter().all(SewAsrLicence::usable)
    }

    /// The pinned licence file with this sha256.
    pub fn licence_by_sha(&self, sha256_hex: &str) -> Option<&'static SewAsrLicence> {
        self.licences.iter().find(|l| hex64(l.sha256) && l.sha256.eq_ignore_ascii_case(sha256_hex))
    }
}

/// The model K2 installs, per Sew release: `base-q8_0` (candle), the only
/// model past Sew's quality gate and the one for a shared 8 GiB box (Sew
/// 2026-11-10). Never `auto`: K2 stages the file before Sew picks.
///
/// Sew 0.9.0's: release assets added to the same v0.9.0 release after the
/// Kessel Steward's licence check (Apache-2.0; quantized by Alakazam Labs
/// from openai/whisper-base@e37978b9, whose weights are MIT). The row is
/// `sew-0.9.0-models.json` (sha256 `d4ffbf22…53cd`) `models[0]`, each hash
/// equal to `SHA256SUMS-models` (sha256 `9ae56512…9159`), checked against
/// the downloaded files (vendored as `testdata/sew/sew-0.9.0.SHA256SUMS-models`,
/// held by a test). Sew checks the model against the pin built into `sew`
/// (`asr_pin_sha256` `6d67c041…b472`) again. A sha256 mismatch is a hard
/// failure (the Steward's R28): no install, no other model, no retry.
pub const SEW_ASR_MODELS: &[SewAsrModel] = &[SewAsrModel {
    sew_version: "0.9.0",
    name: "base-q8_0",
    backend: "candle",
    file: "sew-asr-base-q8_0.gguf",
    sha256: "19221e8b84f5ffa4e5589557792302257e445cec8f0c147515049ad86e52937c",
    size: 82_578_976,
    memory_max_mib: 704,
    licences: &[
        SewAsrLicence {
            file: "asr-license-Apache-2.0.txt",
            sha256: "cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30",
            size: 11_358,
        },
        SewAsrLicence {
            file: "asr-license-openai-whisper-MIT.txt",
            sha256: "b5d65a59060e68c4ff940e1eddfa6f94b2d68fdf58ed7f4dd57721c997e35e9d",
            size: 1_063,
        },
        SewAsrLicence {
            file: "sew-asr-models-NOTICE.txt",
            sha256: "04716eeb41bc4c8e7d98707100ce248aa6cbd6cec1c3643ec912b87e99972b7f",
            size: 955,
        },
    ],
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
    licences: &[SewAsrLicence {
        file: "sew-asr-test-LICENSE.txt",
        sha256: "925a8e505c54df2f25afca649e33a3cc4460380423c74c15fd6fe2d3473b8991",
        size: 76,
    }],
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
/// tarballs, the pinned model or one of its licence files. `None`: not
/// this build's pin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blob {
    Sidecar(Sidecar),
    Model,
    ModelLicence(&'static SewAsrLicence),
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
    let m = model_in(models, rel.version).ok()?;
    if m.sha256.eq_ignore_ascii_case(sha256_hex) {
        return Some(Blob::Model);
    }
    m.licence_by_sha(sha256_hex).map(Blob::ModelLicence)
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

    /// The real pin's model: Sew 0.9.0's base-q8_0, exactly as its release
    /// assets publish it (the vendored `SHA256SUMS-models`, both ways for
    /// the model and its licence files); an older Sew has no row.
    #[test]
    fn the_real_model_pin_is_sews_0_9_0_base_q8_0_and_equals_its_sums() {
        let pin = crate::sew::SEW_RELEASES.last().expect("real pins");
        assert_eq!(pin.version, "0.9.0");
        let m = model_in(SEW_ASR_MODELS, pin.version).expect("the 0.9.0 model is pinned");
        assert_eq!((m.name, m.backend, m.file), ("base-q8_0", "candle", "sew-asr-base-q8_0.gguf"), "never auto");
        assert!(SEW_ASR_MODELS.iter().all(|m| m.name != "auto"));
        assert_eq!((m.sha256, m.size, m.memory_max_mib), ("19221e8b84f5ffa4e5589557792302257e445cec8f0c147515049ad86e52937c", 82_578_976, 704));
        let sums = include_str!("../testdata/sew/sew-0.9.0.SHA256SUMS-models");
        assert_eq!(
            {
                use sha2::Digest;
                sha2::Sha256::digest(sums.as_bytes()).iter().map(|x| format!("{x:02x}")).collect::<String>()
            },
            "9ae56512521e96c2898b2c0020b083b7c8d19d1251d63b7c411de52b06b79159",
            "the vendored SHA256SUMS-models is the published one"
        );
        let mut want: Vec<String> = vec![format!("{}  {}", m.sha256, m.file)];
        want.extend(m.licences.iter().map(|l| format!("{}  {}", l.sha256, l.file)));
        want.sort();
        let mut published: Vec<String> =
            sums.lines().filter(|l| l.ends_with(".gguf") || l.contains("license") || l.contains("NOTICE")).map(str::to_string).collect();
        published.sort();
        assert_eq!(want, published, "the model and its licence files, both ways");
        assert_eq!(m.licences.len(), 3, "two licence texts and the NOTICE");
        // A wrong file is no blob of the pin: the stage refuses it before Sew.
        let wrong = "0".repeat(64);
        assert_eq!(classify_in(pin, SEW_ASR_MODELS, &wrong, "x86_64"), None);
        assert_eq!(classify_in(pin, SEW_ASR_MODELS, m.sha256, "x86_64"), Some(Blob::Model));
        for l in m.licences {
            assert_eq!(classify_in(pin, SEW_ASR_MODELS, l.sha256, "aarch64"), Some(Blob::ModelLicence(l)));
            assert!(l.usable(), "{l:?}");
        }
        // An older Sew has no row at all.
        let e = model_in(SEW_ASR_MODELS, "0.8.3").unwrap_err();
        assert!(e.starts_with(MODEL_UNPINNED) && e.contains("recording needs Sew 0.9.0"), "{e}");
    }

    /// A model row without its licence files is never installable.
    #[test]
    fn a_model_without_its_licences_is_unpinned() {
        static BARE: [SewAsrModel; 1] = [SewAsrModel { licences: &[], ..SEW_ASR_MODELS[0] }];
        let e = model_in(&BARE, "0.9.0").unwrap_err();
        assert!(e.starts_with(MODEL_UNPINNED), "{e}");
        static ODD: [SewAsrLicence; 1] = [SewAsrLicence { file: "../x", sha256: "cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30", size: 1 }];
        static BAD_NAME: [SewAsrModel; 1] = [SewAsrModel { licences: &ODD, ..SEW_ASR_MODELS[0] }];
        assert!(model_in(&BAD_NAME, "0.9.0").is_err(), "a licence file name is a plain name");
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
        assert_eq!(c(m.licences[0].sha256), Some(Blob::ModelLicence(&m.licences[0])));
        assert_eq!(c(&"0".repeat(64)), None, "anything else is no pin");
        assert_eq!(c(r.binary_sha256), None, "a bare binary is not a tarball");
        assert!(sidecar_in(rel, Sidecar::Recorder, "riscv64").is_err());
        // The real pin's sidecars are known too.
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
        for l in m.licences {
            assert_eq!(sha(l.file), l.sha256, "{}", l.file);
            assert_eq!(std::fs::metadata(dir.join(l.file)).expect("licence").len(), l.size);
        }
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

    /// The staged licence paths sit beside the staged model, by pinned name.
    #[test]
    fn licences_stage_beside_the_model() {
        let dir = SEW_STAGE_ASR_MODEL.rsplit_once('/').expect("dir").0;
        for l in SEW_ASR_MODELS[0].licences.iter().chain(TEST_ASR_MODELS[0].licences) {
            assert_eq!(staged_licence(l.file), format!("{dir}/{}", l.file));
        }
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
