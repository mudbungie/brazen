//! Lib↔CLI interface parity (arch §9.8) — the executable form of the bidirectional
//! exclusive-parity invariant: the public `brazen` library surface is EXACTLY the
//! capability set the `bz` binary exposes. Two sets, derived mechanically from the
//! actual sources (no hand-maintained allowlist), asserted equal:
//!
//!   L = the public surface — every name re-exported `pub` at the crate root in
//!       `src/lib.rs` (the lib declares its surface ONLY via `pub use`; this test
//!       also enforces that, so nothing can leak via a `pub mod`/`pub fn`).
//!   B = the CLI-reachable set — every `brazen::` item the `bz` binary crate names
//!       (`src/main.rs` + `src/native/**`, production code AND the native-impl tests
//!       that pin the public round-trips, e.g. `Secret`/`AmbientFormat` reached via
//!       `Cred`/`AmbientSpec`). The `bz` crate is the lib's sole consumer.
//!
//! Forward-compatible by construction: a new capability is a new `brazen::` name in
//! the bin AND a new `pub use` leaf in the lib — both sets gain it with no edit here.
//! Add a `pub use` no bin names → L⊋B, this fails (dead surface). Name a `brazen::`
//! item the lib does not export → it does not compile (the compiler is the other half
//! of the invariant: a `pub fn` cannot expose a private type, and the bin cannot name
//! a non-`pub` item). So this test only has to police the lib⊆CLI direction.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use syn::visit::Visit;
use syn::{File, Item, UseTree, Visibility};

fn manifest() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

fn parse(path: &Path) -> File {
    let src =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    syn::parse_file(&src).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

// ---------- L: the public surface declared in src/lib.rs ----------

/// The names a `pub use` exposes at the crate root: descend through path segments to
/// the terminal leaves (`pub use a::b::{c, d}` → {c, d}; a `… as e` rename exposes
/// `e`). A glob would hide the surface from this check, so it is rejected outright.
fn pub_use_leaves(tree: &UseTree, out: &mut BTreeSet<String>) {
    match tree {
        UseTree::Path(p) => pub_use_leaves(&p.tree, out),
        UseTree::Name(n) => {
            out.insert(n.ident.to_string());
        }
        UseTree::Rename(r) => {
            out.insert(r.rename.to_string());
        }
        UseTree::Group(g) => g.items.iter().for_each(|t| pub_use_leaves(t, out)),
        UseTree::Glob(_) => panic!(
            "src/lib.rs uses a glob `pub use` — the public surface must be \
             enumerated so this parity check sees all of it (arch §9.8)"
        ),
    }
}

/// Is this top-level item `pub` (the surface-leaking visibility, not `pub(crate)`)?
fn is_public(item: &Item) -> bool {
    let vis = match item {
        Item::Const(i) => &i.vis,
        Item::Enum(i) => &i.vis,
        Item::ExternCrate(i) => &i.vis,
        Item::Fn(i) => &i.vis,
        Item::Mod(i) => &i.vis,
        Item::Static(i) => &i.vis,
        Item::Struct(i) => &i.vis,
        Item::Trait(i) => &i.vis,
        Item::TraitAlias(i) => &i.vis,
        Item::Type(i) => &i.vis,
        Item::Union(i) => &i.vis,
        Item::Use(i) => &i.vis,
        _ => return false,
    };
    matches!(vis, Visibility::Public(_))
}

fn lib_public_surface() -> BTreeSet<String> {
    let file = parse(&manifest().join("src/lib.rs"));
    let mut surface = BTreeSet::new();
    for item in &file.items {
        match item {
            Item::Use(u) if matches!(u.vis, Visibility::Public(_)) => {
                pub_use_leaves(&u.tree, &mut surface)
            }
            // The surface is declared EXCLUSIVELY via `pub use`. A `pub mod`/`pub fn`/
            // `pub struct` at the crate root would be public surface this check never
            // sees — fail loudly so the manifest stays the single source (arch §9.8).
            _ if is_public(item) => panic!(
                "src/lib.rs has a `pub` item that is not a `pub use` re-export — declare the \
                 public surface exclusively via `pub use` so interface parity sees all of it"
            ),
            _ => {}
        }
    }
    surface
}

// ---------- B: the CLI-reachable set the `bz` bin crate names ----------

/// Every brazen item the bin names: leaves of `use brazen::{…}` plus inline
/// `brazen::item(…)` path references. The public surface is flat (no `pub mod`), so
/// the first segment after `brazen` IS the public name in every form.
#[derive(Default)]
struct BinRefs {
    set: BTreeSet<String>,
}

fn brazen_use_roots(tree: &UseTree, out: &mut BTreeSet<String>) {
    match tree {
        UseTree::Path(p) => {
            out.insert(p.ident.to_string());
        }
        UseTree::Name(n) => {
            out.insert(n.ident.to_string());
        }
        UseTree::Rename(r) => {
            out.insert(r.ident.to_string());
        }
        UseTree::Group(g) => g.items.iter().for_each(|t| brazen_use_roots(t, out)),
        UseTree::Glob(_) => {}
    }
}

impl<'ast> Visit<'ast> for BinRefs {
    fn visit_item_use(&mut self, u: &'ast syn::ItemUse) {
        if let UseTree::Path(p) = &u.tree {
            if p.ident == "brazen" {
                brazen_use_roots(&p.tree, &mut self.set);
            }
        }
        syn::visit::visit_item_use(self, u);
    }

    fn visit_path(&mut self, path: &'ast syn::Path) {
        let mut segs = path.segments.iter();
        if let (Some(first), Some(second)) = (segs.next(), segs.next()) {
            if first.ident == "brazen" {
                self.set.insert(second.ident.to_string());
            }
        }
        syn::visit::visit_path(self, path);
    }
}

fn bin_source_files() -> Vec<PathBuf> {
    let src = manifest().join("src");
    let mut files = vec![src.join("main.rs"), src.join("native.rs")];
    collect_rs(&src.join("native"), &mut files);
    files
}

fn collect_rs(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read_dir src/native") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            collect_rs(&path, out);
        } else if path.extension().is_some_and(|x| x == "rs") {
            out.push(path);
        }
    }
}

fn cli_reachable_set() -> BTreeSet<String> {
    let mut refs = BinRefs::default();
    for file in bin_source_files() {
        refs.visit_file(&parse(&file));
    }
    refs.set
}

// ---------- the invariant ----------

#[test]
fn public_surface_equals_cli_reachable_set() {
    let lib = lib_public_surface();
    let cli = cli_reachable_set();

    let dead: Vec<_> = lib.difference(&cli).cloned().collect();
    let missing: Vec<_> = cli.difference(&lib).cloned().collect();

    assert!(
        dead.is_empty() && missing.is_empty(),
        "lib↔CLI interface parity broken (arch §9.8):\n  \
         PUBLIC but not CLI-reachable (demote to `pub(crate)` or wire it through `bz`): {dead:?}\n  \
         CLI-reachable but not PUBLIC (add to the `pub use` surface in src/lib.rs): {missing:?}\n  \
         lib surface ({} items): {lib:?}\n  \
         CLI set    ({} items): {cli:?}",
        lib.len(),
        cli.len(),
    );
}
