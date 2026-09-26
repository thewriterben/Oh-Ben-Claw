use std::path::PathBuf;

fn main() {
    embuild::espidf::sysenv::output();

    // The PSRAM cfgs camera.rs and main.rs check. esp-idf-sys emits them (every
    // true kconfig bool becomes `esp_idf_<name>`), `sysenv::output` passes them
    // on; declaring them keeps `unexpected_cfgs` quiet on builds where they are
    // absent.
    println!(
        "cargo:rustc-check-cfg=cfg(esp_idf_spiram, esp_idf_spiram_mode_oct, esp_idf_spiram_mode_quad)"
    );

    refuse_a_shared_target_dir();
}

/// This build script is shared by `obc-esp32-s3` (the live node's build) and
/// `obc-esp32-s3-camera` (which adds the esp32-camera component). esp-idf-sys's
/// build output is keyed by esp-idf-sys's own features, not by which crate is the
/// root, and its build script does not re-run when a manifest's metadata changes.
/// So two crates in one target dir share one esp-idf build: on 2026-09-16 a build
/// on `main` silently destroyed the bring-up branch's camera bindings that way,
/// and the reverse would put the camera component into a default build.
///
/// The first crate to build in a target dir (per target and profile) claims it;
/// the other is refused with the fix. `cargo clean` releases it. It catches the
/// crossing when the other crate's build script first runs there, which is the
/// first time that crate is built in that dir; it is a backstop for
/// build_camera.ps1's separate CARGO_TARGET_DIR, not a replacement for it.
fn refuse_a_shared_target_dir() {
    let Some(out) = std::env::var_os("OUT_DIR").map(PathBuf::from) else {
        return;
    };
    // OUT_DIR is <target-dir>/<triple>/<profile>/build/<pkg>-<hash>/out.
    let mut up = out.ancestors();
    let (Some(_), Some(_), Some(build), Some(profile_dir)) =
        (up.next(), up.next(), up.next(), up.next())
    else {
        return;
    };
    if build.file_name().map_or(true, |n| n != "build") {
        return; // an unfamiliar layout: say nothing rather than guess
    }
    let me = std::env::var("CARGO_PKG_NAME").unwrap_or_default();
    let marker = profile_dir.join("obc-firmware-crate");
    match std::fs::read_to_string(&marker) {
        Ok(owner) if owner.trim() != me => panic!(
            "\n\n  {} was built here by `{}`; this is `{me}`.\n\
             \n  The two crates would share one esp-idf build, and one of them would\n  \
             get the other's camera component (or lose its own). Use a separate\n  \
             CARGO_TARGET_DIR per crate -- scripts/build_camera.ps1 uses C:\\ec-cam --\n  \
             or `cargo clean` this one.\n",
            profile_dir.display(),
            owner.trim(),
        ),
        Ok(_) => {}
        Err(_) => {
            let _ = std::fs::write(&marker, &me);
        }
    }
}
