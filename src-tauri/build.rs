fn main() {
    // Windows only (the setting is ignored elsewhere): Tauri links the VC++
    // runtime statically by default. The test build also links k2-core (a
    // dev-dependency), whose llama-cpp-sys-2 build script links the DEBUG
    // dynamic CRT (msvcrtd) whenever it is itself a debug build. Both at once
    // give the test binary two C runtimes and two heaps, and it died before
    // main (0xc0000374 / 0xc0000005, ucrtbased.dll), so no desktop test ran on
    // Windows. A debug build script (dev and test builds) uses the dynamic
    // runtime; a release one (the installer, which has no k2-core) keeps the
    // static runtime, as before.
    let attributes = tauri_build::Attributes::new()
        .windows_attributes(tauri_build::WindowsAttributes::new().static_vc_runtime(!cfg!(debug_assertions)));
    if let Err(error) = tauri_build::try_build(attributes) {
        println!("{error:#}");
        std::process::exit(1);
    }
}
