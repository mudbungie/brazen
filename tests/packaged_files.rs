//! The packaged-file guard (bl-e087): **what `cargo publish` would ship is read
//! off the real `cargo package --list`, and every path in it must be a class
//! that was ruled in.**
//!
//! Why a test and not a checklist item. `cargo publish` is irreversible — a
//! yanked version stays downloadable — so the one artifact whose mistakes
//! cannot be withdrawn is the one no human should be asked to re-audit by eye
//! each release. `Cargo.toml` declares an `include` ALLOWLIST rather than an
//! `exclude` denylist for the same asymmetry (its own comment states it), and
//! an allowlist without a test is a comment: nothing else notices when a later
//! edit widens it, and the notice arrives after the version is public.
//!
//! The classes below are a **second statement** of the manifest's policy, which
//! is deliberate and is the only shape that can work. A check that derived its
//! allowlist from the `include` key would widen with it and stay green through
//! the exact edit it exists to catch.
//!
//! Both directions, because a shape guard dies by matching nothing:
//! [`the_list_is_not_vacuous`] fails a spawn that answered with a short list,
//! and [`the_allowlist_sees_its_own_violations`] fails an `is_ruled_in` that
//! has quietly become true of everything.
//!
//! And one invariant rather than a second list: [`every_embed_of_a_shipped_file_ships`]
//! reads every `include_str!`/`include_bytes!` target of every SHIPPED source
//! file and requires it to ship too. That is what makes `!src/tests/**` a
//! provable subtraction instead of a remembered one — that module embeds
//! `tests/fixtures/`, which is apparatus, so it could not compile from the
//! package — and it is the answer to a fail-closed list's one cost: a build
//! input added tomorrow and left out of `include` is red here, not broken for
//! whoever downloads the crate.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The repository root — this test's own manifest directory.
fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The real answer to *"what would `cargo publish` upload?"*, one path per
/// line. `--offline` keeps the guard hermetic (the lockfile is committed and
/// every dependency is resolved by the time a test binary runs); `--allow-dirty`
/// is required because `cargo package` refuses a worktree with uncommitted
/// changes outright, and a claim worktree mid-edit is the normal case for the
/// author this test is addressed to. `--list` does not build.
///
/// **The separator is normalized here, once** (bl-a693): cargo prints the
/// PLATFORM separator, so on Windows every line came back `src\auth\device.rs`
/// and no class ruled any of it in — the guard reddened on all three tests at
/// once while the tree was correct. A package manifest's paths are posix by
/// definition (`include` patterns, the tarball's own entries), so the answer is
/// to fix the reading rather than to teach every comparison two spellings.
fn packaged() -> Vec<String> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let out = Command::new(cargo)
        .current_dir(root())
        .args(["package", "--list", "--offline", "--allow-dirty"])
        .output()
        .expect("spawn cargo package --list");
    assert!(
        out.status.success(),
        "cargo package --list did not answer: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|p| p.replace('\\', "/"))
        .collect()
}

/// The classes ruled into the published crate: the crate's own source outside
/// the `#[cfg(test)]` corpus, the two files the build embeds, and the files the
/// registry renders. `Cargo.toml.orig` and `.cargo_vcs_info.json` are minted by
/// cargo into the tarball and are not tree files at all.
fn is_ruled_in(path: &str) -> bool {
    let named = matches!(
        path,
        "Cargo.toml"
            | "Cargo.lock"
            | "Cargo.toml.orig"
            | ".cargo_vcs_info.json"
            | "README.md"
            | "LICENSE"
            | "CHANGELOG.md"
            | "SKILL.md"
            | "data/defaults.toml"
    );
    named
        || path
            .strip_prefix("src/")
            .is_some_and(|p| p.ends_with(".rs") && !p.starts_with("tests/"))
}

/// The defect: design commentary, gate apparatus, fabricated-secret fixtures and
/// agent guides shipping to crates.io with the binary. Stated as an allowlist so
/// the NEXT file class added to the tree is red here instead of public there.
#[test]
fn no_commentary_or_apparatus_ships() {
    let strays: Vec<String> = packaged().into_iter().filter(|p| !is_ruled_in(p)).collect();
    assert!(
        strays.is_empty(),
        "paths `cargo publish` would upload that no class rules in — a yanked \
         version stays downloadable, so widen `include` in Cargo.toml only with \
         a reason, and add the class here:\n{}",
        strays.join("\n")
    );
}

/// The other side of a fail-closed list: the crate must still be a crate.
#[test]
fn the_files_crates_io_and_the_build_need_ship() {
    let list = packaged();
    for needed in [
        "Cargo.toml",
        "Cargo.lock",
        "README.md",
        "LICENSE",
        // the two compile-time embeds — without either the published crate does
        // not build at all
        "SKILL.md",
        "data/defaults.toml",
        "src/lib.rs",
        "src/main.rs",
    ] {
        assert!(
            list.iter().any(|p| p == needed),
            "{needed} is not in the packaged list — `include` dropped a file \
             crates.io or the build needs"
        );
    }
}

/// Every `include_str!`/`include_bytes!` target named by a file the package
/// carries, as a package-root-relative path. Read off the shipped source, so it
/// answers a question about the tree rather than restating a list.
fn embeds_of_shipped_files(list: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for src in list.iter().filter(|p| p.ends_with(".rs")) {
        let dir = Path::new(src).parent().unwrap_or(Path::new("")).to_owned();
        let text = std::fs::read_to_string(root().join(src)).expect("read a packaged source file");
        for line in text.lines() {
            for macro_name in ["include_str!(\"", "include_bytes!(\""] {
                let Some((_, tail)) = line.split_once(macro_name) else {
                    continue;
                };
                let Some((literal, _)) = tail.split_once('"') else {
                    continue;
                };
                out.push(normalize(&dir.join(literal)));
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// `a/b/../../c` → `c`: the package list is flat, package-root-relative text,
/// so an embed's `../..` hops have to be resolved textually — the file it names
/// need not exist on this box for the question to be answerable.
fn normalize(path: &Path) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.iter().filter_map(|p| p.to_str()) {
        match part {
            "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    parts.join("/")
}

/// The fail-closed list's one cost, paid: a file the build reads and `include`
/// does not carry compiles here and fails to compile for everyone who downloads
/// the crate. Also the proof behind `!src/tests/**` — that module embeds
/// `tests/fixtures/`, so shipping it would land here.
#[test]
fn every_embed_of_a_shipped_file_ships() {
    let list = packaged();
    let missing: Vec<String> = embeds_of_shipped_files(&list)
        .into_iter()
        .filter(|p| !list.contains(p))
        .collect();
    assert!(
        missing.is_empty(),
        "paths the packaged source embeds with include_str!/include_bytes! that \
         the package does not carry — the published crate cannot compile:\n{}",
        missing.join("\n")
    );
}

/// A guard that measured nothing must not read as a pass: a failed spawn, an
/// empty stdout, or a sweep that found no embeds all land here.
#[test]
fn the_list_is_not_vacuous() {
    let list = packaged();
    let sources = list.iter().filter(|p| p.starts_with("src/")).count();
    assert!(
        sources > 100,
        "the packaged list carries {sources} src paths over {} entries — the \
         spawn is broken, not the tree",
        list.len()
    );
    // The sweep must still be finding the two embeds it exists to protect; a
    // parser that matched nothing would pass `every_embed_of_a_shipped_file_ships`
    // forever.
    let embeds = embeds_of_shipped_files(&list);
    for needed in ["SKILL.md", "data/defaults.toml"] {
        assert!(
            embeds.contains(&needed.to_owned()),
            "the embed sweep found {embeds:?} — it no longer sees {needed}"
        );
    }
}

/// The negative direction for the restated policy: each excluded class, and the
/// measured unanchored-pattern trap, must be seen as a violation — and the
/// classes that ship must not.
#[test]
fn the_allowlist_sees_its_own_violations() {
    for stray in [
        "specs/architecture.md",
        "specs/auth.md",
        "AGENTS.md",
        "Makefile",
        "deny.toml",
        "release-plz.toml",
        "examples/stdio_transport.rs",
        "tests/packaged_files.rs",
        "tests/fixtures/claude_session_request.json",
        ".github/workflows/ci.yml",
        ".githooks/pre-commit",
        ".claude/settings.json",
        "scripts/leak-scan.sh",
        // the fabricated-secret corpus, and the unanchored-pattern sighting: a
        // bare `README.md` include pattern ships this out of a list that names
        // no `scripts` entry
        "scripts/leak-fixtures/README.md",
        "scripts/leak-fixtures/private-key.txt",
        // the `#[cfg(test)]` corpus: it embeds `tests/fixtures/`, so it cannot
        // compile from the package, and it carries the Claude-session recipe
        "src/tests/mod.rs",
        "src/tests/claude_session_conformance.rs",
    ] {
        assert!(!is_ruled_in(stray), "{stray} must not be ruled in");
    }
    for shipped in [
        "src/main.rs",
        "src/lib.rs",
        "src/run/generate.rs",
        "src/native/tests/mod.rs",
        "SKILL.md",
        "data/defaults.toml",
        "LICENSE",
    ] {
        assert!(is_ruled_in(shipped), "{shipped} must be ruled in");
    }
}
