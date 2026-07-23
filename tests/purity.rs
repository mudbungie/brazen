//! The network-free-library invariant, as an executable test (arch §9.5, §10).
//!
//! brazen ships as ONE crate: the pure pipeline + canonical types + seam traits are
//! the `brazen` library, and the impure native wiring (the `ureq` HTTP transport,
//! the `libc` SIGPIPE/isatty calls, the XDG files) is confined to the `bz` bin
//! (`src/main.rs`) and `src/native/`. While `bz` and `brazen` were two separate
//! crates, that confinement was the crate graph's: a library module that wrote `use
//! ureq` simply would not compile, because `ureq`/`libc` were not in the lib crate's
//! dependency tree (bl-c420). Collapsing to one publishable crate (so `cargo install
//! brazen` can build the `bz` bin, bl-c1e2) makes `ureq`/`libc` crate-wide deps, so
//! the compiler can no longer forbid them in a pure module.
//!
//! This test re-establishes the invariant: it walks every library source file —
//! everything under `src/` EXCEPT the shim (`src/main.rs` and `src/native/`) — and
//! fails if any of them references the impure surface (`ureq`, `libc`, `std::net`,
//! `TcpListener`/`TcpStream`). A would-be impurity in the pure core now turns a
//! green build red here instead of at link time, which is the same guarantee one
//! layer out. The shim itself is exempt: that is exactly where the impurity belongs.
//!
//! The shim (`src/native/`) is also re-exposed as the `brazen::native` module under the
//! OFF-by-default `native-host` feature (yog DESIGN §16.7 U-brazen; bl-547d), so an
//! embedding host can drive brazen's seams in process without a system `bz`. That does
//! NOT widen the impurity boundary: the exempt paths are the impure surface whether it is
//! reached via the `bz` bin or via `native-host`, and the SECOND test below pins the
//! other half — the exposure is feature-gated, so the default (pure) build never links
//! `native` into the library. The feature IS the boundary; the pure core stays pure.

use std::path::{Path, PathBuf};

/// The impure tokens a library module must never contain. These are code-shaped
/// (a path separator or a `use`), so prose that merely names "ureq" in a doc comment
/// — e.g. `src/transport.rs` describing `ureq`'s body-timeout semantics — does not
/// trip the scan; only an actual import/usage does.
const FORBIDDEN: &[&str] = &[
    "use ureq",
    "ureq::",
    "use libc",
    "libc::",
    "std::net",
    "TcpListener",
    "TcpStream",
    // The exec transport (claude-code spec §3.4): the pure lib can no more spawn a
    // subprocess than open a socket — the spawn lives in `src/native/exec.rs`.
    // (`std::process::id`, a mere pid read, stays allowed; only spawning is impure.)
    "std::process::Command",
    "process::Command",
    "Command::new",
];

/// Source paths under `src/` that are the SHIM, not the library, and so are allowed
/// the impure deps: the `bz` bin entry (`main.rs`), the native module root
/// (`native.rs`), and everything under the native module dir (`native/`). These are
/// the impure boundary whether reached via the `bz` bin or the `native-host` feature
/// (`native_host_exposure_is_feature_gated` pins that the latter never leaks into the
/// default build).
fn is_shim(rel: &Path) -> bool {
    rel == Path::new("main.rs") || rel == Path::new("native.rs") || rel.starts_with("native")
}

/// Recursively collect every `.rs` file under `dir`.
fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read_dir src/") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            rs_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn library_modules_never_import_the_network_or_libc() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rs_files(&src, &mut files);
    // Guard against a refactor silently emptying the scan (e.g. src/ moving): the
    // library is many files, so a near-zero count means the walk broke, not that
    // the code got pure.
    assert!(
        files.len() > 20,
        "purity scan found only {} files under src/ — the walk is broken",
        files.len()
    );

    let mut offenders = Vec::new();
    for file in &files {
        let rel = file.strip_prefix(&src).expect("under src/");
        if is_shim(rel) {
            continue;
        }
        let body = std::fs::read_to_string(file).expect("read source");
        for token in FORBIDDEN {
            if body.contains(token) {
                offenders.push(format!("{}: contains `{token}`", rel.display()));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "library modules must stay network-free (the bl-c420 invariant, now \
         test-enforced after the single-crate collapse bl-c1e2) — move the impurity \
         into src/native/ or the bz bin:\n  {}",
        offenders.join("\n  ")
    );
}

/// The other half of the boundary (bl-547d): `src/native/` is re-exposed as the
/// `brazen::native` module, but ONLY under the `native-host` feature — never in the
/// default (pure) build. Enforce it textually against `src/lib.rs`: the sole `pub mod
/// native` must be immediately preceded by its `#[cfg(feature = "native-host")]` gate,
/// and there must be no un-gated exposure. Without the gate, the default `cargo build`
/// would link the impure `ureq`/`TcpListener` shim into the library — the very thing
/// this file exists to forbid — so a dropped gate turns green red here.
#[test]
fn native_host_exposure_is_feature_gated() {
    let lib = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lib.rs");
    let src = std::fs::read_to_string(&lib).expect("read src/lib.rs");
    // Normalize whitespace between the gate and the `pub mod` so formatting can't fool
    // the check (the attribute and the item sit on their own lines).
    let gated = "#[cfg(feature = \"native-host\")]\npub mod native;";
    assert!(
        src.contains(gated),
        "src/lib.rs must expose `native` ONLY as `{gated}` — the native-host feature is \
         the purity boundary (bl-547d)"
    );
    // No exposure of `native` may escape the gate: every `pub mod native` occurrence must
    // be the gated one. (A bare `mod native;` — non-`pub` — would be a library-private
    // link of the impure shim, equally forbidden.)
    let pub_count = src.matches("pub mod native;").count();
    let mod_count = src.matches("mod native;").count();
    assert_eq!(
        pub_count, 1,
        "src/lib.rs has {pub_count} `pub mod native;` — expected exactly one, the gated one"
    );
    assert_eq!(
        mod_count, pub_count,
        "src/lib.rs links `native` other than through the gated `pub mod` (bl-547d)"
    );
}
