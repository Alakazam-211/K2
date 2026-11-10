fn main() {
    // Windows only (the setting is ignored elsewhere): Tauri links the VC++
    // runtime statically by default. The test build also links k2-core (a
    // dev-dependency), and a C dependency built in debug can link the DEBUG
    // dynamic CRT (msvcrtd). Both at once give the test binary two C runtimes
    // and two heaps; it died before main (0xc0000374 / 0xc0000005,
    // ucrtbased.dll), so no desktop test ran on Windows. llama-cpp-sys-2 did
    // that and is gone (the local LLM was removed); other CMake-built debug
    // dependencies can do the same, and no Windows run has shown otherwise,
    // so the switch stays. A debug build script (dev and test builds) uses
    // the dynamic runtime; a release one (the installer, which has no
    // k2-core) keeps the static runtime, as before.
    let mut attributes = tauri_build::Attributes::new()
        .windows_attributes(tauri_build::WindowsAttributes::new().static_vc_runtime(!cfg!(debug_assertions)));
    // `sew-voice:*` exists only when the plugin is linked, and a capability that
    // names an unknown permission fails the build. So its entry sits in its own
    // folder, read only with the `native-voice` feature.
    if std::env::var_os("CARGO_FEATURE_NATIVE_VOICE").is_some() {
        println!("cargo:rerun-if-changed=capabilities");
        println!("cargo:rerun-if-changed=capabilities-native-voice");
        attributes = attributes.capabilities_path_pattern("./capabilities*/**/*");
    }
    if let Err(error) = tauri_build::try_build(attributes) {
        println!("{error:#}");
        std::process::exit(1);
    }
}
