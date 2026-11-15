//! Placement for the tools k2-node installs itself from a pinned download
//! (k2-node 0.46.13: `android-ndk@27`, `cargo-ndk@4`): Linux x86_64 only,
//! reported in the offer's tools under their own key, the NDK by its
//! revision (`android-ndk = 27.3.13750724`), cargo-ndk by its version.

use k2_core::compute::proto::frames::Offer;
use k2_core::compute::proto::needs;

use super::NeedState;

/// A native tool's state on the node behind `offer`, and the version found
/// when ready: unsupported off Linux x86_64 (a Mac, arm64 Linux), ready
/// when the pinned build is there (however the version is spelled), else
/// to install.
pub(super) fn need_state(offer: &Offer, tool: &str, version: &str) -> (NeedState, Option<String>) {
    let label = |k: &str| offer.labels.get(k).map(String::as_str);
    let (os, arch) = needs::NATIVE_TOOLS_HOST;
    if label("os").is_some_and(|o| o != os) || label("arch").is_some_and(|a| a != arch) {
        return (NeedState::UnsupportedOnOs, None);
    }
    let want = if tool == needs::ANDROID_NDK {
        needs::ndk_pin(version).map(|p| p.revision.to_string())
    } else {
        needs::cargo_ndk_matches(version).then(|| needs::CARGO_NDK_VERSION.to_string())
    };
    let have = offer.tools.get(tool).map(|v| v.split(',').map(str::trim).any(|x| Some(x) == want.as_deref())).unwrap_or(false);
    match want.filter(|_| have) {
        Some(v) => (NeedState::Ready, Some(v)),
        None => (NeedState::Install, None),
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::cand;
    use super::super::{need_states, tools_ready, Cand, NeedState};

    fn node(name: &str, os: &str, arch: &str, tools: &[(&str, &str)]) -> Cand {
        let mut c = cand(name, 32, 32, 0, 4);
        c.offer.labels.insert("os".into(), os.into());
        c.offer.labels.insert("arch".into(), arch.into());
        for (k, v) in tools {
            c.offer.tools.insert(k.to_string(), v.to_string());
        }
        c
    }

    const NDK: &[(&str, &str)] = &[("android-ndk", "27.3.13750724"), ("cargo-ndk", "4.1.2")];

    #[test]
    fn android_ndk_is_ready_by_revision_and_linux_x86_64_only() {
        let toks: Vec<String> = ["android-ndk@27", "ndk@r27d", "android-ndk@27.3.13750724", "cargo-ndk@4"].map(String::from).to_vec();
        let linux = node("a-linux", "linux", "x86_64", NDK);
        let states = need_states(&linux.offer, &toks);
        assert!(states.iter().all(|s| s.state == NeedState::Ready), "{states:?}");
        assert_eq!(states[0].have.as_deref(), Some("27.3.13750724"));
        assert!(tools_ready(&states), "a node with them cached counts as tools ready (placement prefers it)");
        let bare = node("b-bare", "linux", "x86_64", &[]);
        assert!(need_states(&bare.offer, &toks).iter().all(|s| s.state == NeedState::Install));
        let other = node("c-r26", "linux", "x86_64", &[("android-ndk", "26.3.11579264")]);
        assert_eq!(need_states(&other.offer, &toks[..1])[0].state, NeedState::Install, "another revision isn't the pin");
        for c in [node("d-mac", "macos", "arm64", NDK), node("e-arm", "linux", "aarch64", NDK)] {
            assert_eq!(need_states(&c.offer, &toks[..1])[0].state, NeedState::UnsupportedOnOs, "{:?}", c.offer.labels);
        }
    }
}
