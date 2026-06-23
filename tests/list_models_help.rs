//! `bz list-models` discovery short-circuit (§5.5): `--help`/`--version` print the
//! ONE shared doc to stdout and exit 0 BEFORE any provider resolution or network —
//! the same short-circuit the data plane (`run_control`) and `bz login` honor, so a
//! probe answers with no provider configured and never touches the transport. Split
//! from `list_models.rs` to keep each file under the 300-line cap; same harness.

mod list_models_support;

use brazen::testing::{MemoryCredStore, MockTransport};
use list_models_support::go;

#[test]
fn help_prints_the_shared_usage_to_stdout_exit_0() {
    let tx = MockTransport::ok(vec![]);
    let o = go(&["list-models", "--help"], &tx, &MemoryCredStore::new());
    assert_eq!(o.code, 0);
    assert!(o.stderr.is_empty(), "help goes to stdout, not stderr");
    assert!(o.stdout.contains("USAGE:"));
    assert!(o.stdout.contains("list-models"));
    assert!(o.stdout.contains("--browser"));
    assert!(tx.requests().is_empty(), "help does no network");
}

#[test]
fn version_prints_the_package_version_to_stdout_exit_0() {
    let tx = MockTransport::ok(vec![]);
    let o = go(&["list-models", "--version"], &tx, &MemoryCredStore::new());
    assert_eq!(o.code, 0);
    assert!(o.stderr.is_empty());
    assert_eq!(o.stdout, concat!("bz ", env!("CARGO_PKG_VERSION"), "\n"));
    assert!(tx.requests().is_empty(), "version does no network");
}
