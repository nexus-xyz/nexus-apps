//! Points the binary at CEF's files for development runs: `cef-dll-sys` downloads CEF's binary distribution under
//! `CEF_PATH` (`.cargo/config.toml`), and the binary needs `libcef` and its resources from there at run time. This
//! finds the folder that holds `libcef`, adds it to the binary's library path (rpath) and tells `src/web` where it is
//! (`NEXUS_CEF_DIR`). A shipped build copies CEF next to the binary instead, and finds it there through `$ORIGIN` on
//! Linux and the DLL search order on Windows (`packaging/`). On macOS CEF is a framework the app loads at run time from
//! its bundle (`src/web/mac.rs`), so there is nothing to link or point at here.
//!
//! It also names the Nexus host this build installs on a server (`NEXUS_HOST_VERSION`, `src/machines.rs`): this app's
//! own version and the git sha, as nexus-host's own build.rs makes them. The release builds the host with the same
//! `NEXUS_DESKTOP_BUILD_VERSION` and publishes it in this app's feed.
//!
//! And it stages the logos the app embeds with `held!` (`held_assets`): a mark that is not in the tree, as in the
//! open-source repo while Legal reviews third-party logos and Nexus's own marks, builds with a generic stand-in.

// git runs at build time, not in the app: the console rule is not for it.
#![allow(clippy::disallowed_methods)]

// `factory_version`: the git short sha every Nexus binary carries. It brings `Path` and `PathBuf`.
include!("../../build/version.rs");

fn main() {
    host_version();
    held_assets();
    println!("cargo::rerun-if-env-changed=CEF_PATH");
    // A shipped build's `libcef.so` sits beside the binary. `$ORIGIN` comes first, so a package never loads a
    // development copy.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        println!("cargo::rustc-link-arg-bins=-Wl,-rpath,$ORIGIN");
    }
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        return;
    }
    let Some(root) = std::env::var_os("CEF_PATH").map(PathBuf::from) else {
        return;
    };
    let library = if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        "libcef.dll"
    } else {
        "libcef.so"
    };
    let Some(dir) = find(&root, library, 4) else {
        println!(
            "cargo::warning=no {library} under {}; web views will not start",
            root.display()
        );
        return;
    };
    // `libcef.so` sits in `Release/`, its resources (`*.pak`, `icudtl.dat`, `locales/`) beside it or in
    // `Resources/`, depending on the distribution.
    let resources = [dir.join("../Resources"), dir.clone()]
        .into_iter()
        .find(|path| path.join("resources.pak").exists() || path.join("icudtl.dat").exists())
        .unwrap_or_else(|| dir.clone());
    let resources = resources.canonicalize().unwrap_or(resources);
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        println!("cargo::rustc-link-arg-bins=-Wl,-rpath,{}", dir.display());
    }
    println!("cargo::rustc-env=NEXUS_CEF_DIR={}", dir.display());
    println!("cargo::rustc-env=NEXUS_CEF_RESOURCES={}", resources.display());
    println!("cargo::rerun-if-changed={}", dir.display());
}

/// The first folder under `root`, `depth` levels down at most, that holds `file`.
fn find(root: &Path, file: &str, depth: usize) -> Option<PathBuf> {
    if root.join(file).exists() {
        return root.canonicalize().ok();
    }
    if depth == 0 {
        return None;
    }
    let mut entries: Vec<PathBuf> = std::fs::read_dir(root)
        .ok()?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.is_dir())
        .collect();
    entries.sort();
    entries.into_iter().find_map(|path| find(&path, file, depth - 1))
}

/// `NEXUS_HOST_VERSION`: this build's version plus the git short sha, as nexus-host's build.rs makes it. A server's host
/// says this version in its hello. The version is `NEXUS_DESKTOP_BUILD_VERSION`, which the release sets for the app and
/// its host alike (`0.11.0-internal.42` on main), else `version` in Cargo.toml.
fn host_version() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default());
    let (_, factory) = factory_version(&manifest.join("../../crates/nexus-client"));
    let sha = factory.split_once('+').map(|(_, sha)| sha.to_string());
    println!("cargo::rerun-if-env-changed=NEXUS_DESKTOP_BUILD_VERSION");
    let semver = match std::env::var("NEXUS_DESKTOP_BUILD_VERSION") {
        Ok(version) if !version.trim().is_empty() => version.trim().to_string(),
        _ => std::env::var("CARGO_PKG_VERSION").unwrap_or_default(),
    };
    let version = match sha {
        Some(sha) => format!("{semver}+{sha}"),
        None => semver,
    };
    println!("cargo::rustc-env=NEXUS_HOST_VERSION={version}");
}

/// The files `src/assets.rs` and `src/tray.rs` embed with `held!("marks/x.svg")`, copied to `$OUT_DIR/held/`, where the
/// macro reads them. A file missing from `assets/` stops the build, unless the build opts in with NEXUS_HELD_STAND_INS=1.
/// The open-source repo does (its .cargo/config.toml): it leaves out the logos Legal has not cleared (export manifest,
/// B2), so each gets `assets/held/stand-in.svg` (or `.png`), a plain ring, or its own neutral stand-in there
/// (`app-mark.svg`). Any stand-in sets `cfg(nexus_marks_held)`, so the tests that check a real mark's drawing skip.
fn held_assets() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default());
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("cargo sets OUT_DIR")).join("held");
    println!("cargo::rustc-check-cfg=cfg(nexus_marks_held)");
    let mut names = std::collections::BTreeSet::new();
    for source in ["src/assets.rs", "src/tray.rs"] {
        println!("cargo::rerun-if-changed={source}");
        let text = std::fs::read_to_string(manifest.join(source)).unwrap_or_else(|e| panic!("read {source}: {e}"));
        for piece in text.split("held!(\"").skip(1) {
            if let Some((name, _)) = piece.split_once('"') {
                names.insert(name.to_string());
            }
        }
    }
    let mut stand_ins = 0;
    for name in &names {
        let real = manifest.join("assets").join(name);
        let bytes = match std::fs::read(&real) {
            Ok(bytes) => {
                println!("cargo::rerun-if-changed={}", real.display());
                bytes
            }
            Err(_) => {
                stand_ins += 1;
                // Its own stand-in when assets/held/ has one by the same file name (`app-mark.svg`: the layered mark
                // the animations cut up), else the generic ring.
                let ext = Path::new(name).extension().and_then(|e| e.to_str()).unwrap_or("svg");
                let own = Path::new(name)
                    .file_name()
                    .map(|file| manifest.join("assets/held").join(file));
                let stand_in = own
                    .filter(|path| path.is_file())
                    .unwrap_or_else(|| manifest.join(format!("assets/held/stand-in.{ext}")));
                std::fs::read(&stand_in).unwrap_or_else(|e| panic!("no stand-in for {name}: {e}"))
            }
        };
        let target = out.join(name);
        if std::fs::read(&target).ok().as_deref() != Some(bytes.as_slice()) {
            std::fs::create_dir_all(target.parent().expect("a file has a folder")).expect("create $OUT_DIR/held");
            std::fs::write(&target, bytes).expect("write a held asset");
        }
    }
    // A stand-in only where the build opts in: the open-source repo's .cargo/config.toml sets NEXUS_HELD_STAND_INS=1.
    // Anywhere else a missing logo is a mistyped `held!` path or a deleted mark, so the build stops.
    println!("cargo::rerun-if-env-changed=NEXUS_HELD_STAND_INS");
    if stand_ins > 0 && std::env::var("NEXUS_HELD_STAND_INS").as_deref() != Ok("1") {
        let missing: Vec<_> = names
            .iter()
            .filter(|n| !manifest.join("assets").join(n).is_file())
            .collect();
        panic!("these logos are not in assets/ (set NEXUS_HELD_STAND_INS=1 to build stand-ins): {missing:?}");
    }
    if stand_ins > 0 {
        // A folder that comes back (a new mark) reruns this, so the real file replaces its stand-in.
        println!("cargo::rerun-if-changed=assets");
        println!("cargo::rustc-cfg=nexus_marks_held");
        println!(
            "cargo::warning={stand_ins} of {} logos are not in the tree: they build as a generic stand-in",
            names.len()
        );
    }
}
