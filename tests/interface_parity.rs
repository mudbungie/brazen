//! Lib↔CLI interface parity (arch §9.8) — the executable invariant: the public `brazen`
//! surface is EXACTLY the interface its entry points define. The interface is the typed
//! I/O (a `CanonicalRequest` in, an `Event` stream out) plus the seams/config that drive
//! it; the byte CLI is one serialization of that (bl-b4a9). So parity is a TYPE-CLOSURE,
//! derived mechanically (no allowlist):
//!
//!   ROOTS   = every `pub` FN/CONST re-exported at the crate root (`run`, `generate`, …).
//!   CLOSURE = every crate TYPE transitively reachable from a root's signature (struct
//!             fields, enum variants, trait-method signatures).
//!
//! Asserts: the `pub` TYPES at the crate root == CLOSURE. A new entry point pulls its I/O
//! types into CLOSURE automatically; a `pub` type no root reaches is an orphan; a private
//! type in a public signature does not compile (`#![deny(private_interfaces)]` is the
//! other half). The surface is declared ONLY via `pub use`, which this also enforces.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use syn::visit::Visit;
use syn::{Fields, File, FnArg, Item, ReturnType, Signature, Type, UseTree, Visibility};

type Set = BTreeSet<String>;

fn manifest() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

fn parse(path: &Path) -> File {
    let src =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    syn::parse_file(&src).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

/// A crate item: a TYPE (struct/enum/union/trait/alias — a closure node) vs. a root
/// (fn/const), plus the crate type-names its definition/signature references.
#[derive(Default)]
struct ItemInfo {
    is_type: bool,
    refs: Set,
}

/// Library source files: under `src/`, minus the bin shim and the test-only trees.
fn lib_files() -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect(&manifest().join("src"), &mut files);
    files
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read_dir src") {
        let path = entry.expect("dir entry").path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if path.is_dir() {
            if !matches!(name, "native" | "tests" | "testing") {
                collect(&path, out);
            }
        } else if name.ends_with(".rs") && name != "main.rs" {
            out.push(path);
        }
    }
}

/// Visit every library item, recursing into inline modules but NOT `#[cfg(test)]` ones.
fn for_each_item(mut f: impl FnMut(&Item)) {
    fn walk(items: &[Item], f: &mut impl FnMut(&Item)) {
        for item in items {
            if let Item::Mod(m) = item {
                if !is_cfg_test(&m.attrs) {
                    if let Some((_, inner)) = &m.content {
                        walk(inner, f);
                    }
                }
                continue;
            }
            f(item);
        }
    }
    for file in lib_files() {
        walk(&parse(&file).items, &mut f);
    }
}

fn is_cfg_test(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| match &a.meta {
        syn::Meta::List(l) if a.path().is_ident("cfg") => l.tokens.to_string().contains("test"),
        _ => false,
    })
}

/// Collects crate-defined type names mentioned anywhere visited, filtering std/serde out.
struct Refs<'a> {
    defined: &'a Set,
    out: Set,
}

impl<'ast> Visit<'ast> for Refs<'_> {
    fn visit_ident(&mut self, id: &'ast syn::Ident) {
        let s = id.to_string();
        if self.defined.contains(&s) {
            self.out.insert(s);
        }
    }
}

fn type_refs(ty: &Type, defined: &Set) -> Set {
    let mut r = Refs {
        defined,
        out: Set::new(),
    };
    r.visit_type(ty);
    r.out
}

/// The crate types a signature exposes: its typed parameters and return (never `self`,
/// param names, or generics — only the I/O types).
fn sig_refs(sig: &Signature, defined: &Set) -> Set {
    let mut out = Set::new();
    for input in &sig.inputs {
        if let FnArg::Typed(pt) = input {
            out.extend(type_refs(&pt.ty, defined));
        }
    }
    if let ReturnType::Type(_, ty) = &sig.output {
        out.extend(type_refs(ty, defined));
    }
    out
}

fn field_refs(fields: &Fields, defined: &Set) -> Set {
    fields
        .iter()
        .flat_map(|f| type_refs(&f.ty, defined))
        .collect()
}

fn defined_types() -> Set {
    let mut defined = Set::new();
    for_each_item(|item| {
        let name = match item {
            Item::Struct(i) => &i.ident,
            Item::Enum(i) => &i.ident,
            Item::Union(i) => &i.ident,
            Item::Trait(i) => &i.ident,
            Item::Type(i) => &i.ident,
            _ => return,
        };
        defined.insert(name.to_string());
    });
    defined
}

fn item_model(defined: &Set) -> BTreeMap<String, ItemInfo> {
    let mut map: BTreeMap<String, ItemInfo> = BTreeMap::new();
    for_each_item(|item| {
        let (name, is_type, refs) = match item {
            Item::Struct(i) => (&i.ident, true, field_refs(&i.fields, defined)),
            Item::Enum(i) => (
                &i.ident,
                true,
                i.variants
                    .iter()
                    .flat_map(|v| field_refs(&v.fields, defined))
                    .collect(),
            ),
            Item::Trait(i) => (
                &i.ident,
                true,
                i.items
                    .iter()
                    .filter_map(|ti| match ti {
                        syn::TraitItem::Fn(m) => Some(sig_refs(&m.sig, defined)),
                        _ => None,
                    })
                    .flatten()
                    .collect(),
            ),
            Item::Type(i) => (&i.ident, true, type_refs(&i.ty, defined)),
            Item::Fn(i) => (&i.sig.ident, false, sig_refs(&i.sig, defined)),
            Item::Const(i) => (&i.ident, false, type_refs(&i.ty, defined)),
            _ => return,
        };
        // A name may be defined once and impl'd elsewhere; the type wins, refs union.
        let slot = map.entry(name.to_string()).or_default();
        slot.is_type |= is_type;
        slot.refs.extend(refs);
    });
    map
}

fn pub_use_leaves(tree: &UseTree, out: &mut Set) {
    match tree {
        UseTree::Path(p) => pub_use_leaves(&p.tree, out),
        UseTree::Name(n) => {
            out.insert(n.ident.to_string());
        }
        UseTree::Rename(r) => {
            out.insert(r.rename.to_string());
        }
        UseTree::Group(g) => g.items.iter().for_each(|t| pub_use_leaves(t, out)),
        UseTree::Glob(_) => panic!("src/lib.rs uses a glob `pub use` — enumerate it (§9.8)"),
    }
}

/// The crate-root public names — and the guard that the surface is declared ONLY via
/// `pub use` (a `pub mod`/`pub fn` would be surface this check never sees, §9.8).
fn public_surface() -> Set {
    let file = parse(&manifest().join("src/lib.rs"));
    let mut surface = Set::new();
    for item in &file.items {
        match item {
            Item::Use(u) if matches!(u.vis, Visibility::Public(_)) => {
                pub_use_leaves(&u.tree, &mut surface)
            }
            Item::Use(_) => {}
            _ => assert!(
                !is_public(item),
                "lib.rs has a `pub` item that is not a `pub use`"
            ),
        }
    }
    surface
}

fn is_public(item: &Item) -> bool {
    let vis = match item {
        Item::Const(i) => &i.vis,
        Item::Enum(i) => &i.vis,
        Item::Fn(i) => &i.vis,
        Item::Mod(i) => &i.vis,
        Item::Static(i) => &i.vis,
        Item::Struct(i) => &i.vis,
        Item::Trait(i) => &i.vis,
        Item::Type(i) => &i.vis,
        _ => return false,
    };
    matches!(vis, Visibility::Public(_))
}

#[test]
fn public_types_equal_the_entry_point_closure() {
    let defined = defined_types();
    let model = item_model(&defined);
    let surface = public_surface();

    let unknown: Vec<_> = surface.iter().filter(|n| !model.contains_key(*n)).collect();
    assert!(
        unknown.is_empty(),
        "public names with no crate definition: {unknown:?}"
    );

    let (mut roots, mut public_types) = (Set::new(), Set::new());
    for name in &surface {
        if model[name].is_type {
            public_types.insert(name.clone());
        } else {
            roots.insert(name.clone());
        }
    }

    // Closure: BFS from the roots' signature types through every reachable type def.
    let mut closure = Set::new();
    let mut work: Vec<String> = roots
        .iter()
        .flat_map(|r| model[r].refs.iter().cloned())
        .collect();
    while let Some(t) = work.pop() {
        if closure.insert(t.clone()) {
            if let Some(info) = model.get(&t) {
                work.extend(info.refs.iter().cloned());
            }
        }
    }

    let orphans: Vec<_> = public_types.difference(&closure).cloned().collect();
    let missing: Vec<_> = closure.difference(&public_types).cloned().collect();
    assert!(
        orphans.is_empty() && missing.is_empty(),
        "lib↔CLI interface parity broken (§9.8): {} roots, {} types, {} closure.\n  \
         ORPHAN (no entry point reaches them — demote to `pub(crate)`): {orphans:?}\n  \
         MISSING (an entry point's signature needs them public): {missing:?}",
        roots.len(),
        public_types.len(),
        closure.len(),
    );
}
