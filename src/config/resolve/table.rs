//! Table membership (config §3.2): which rows of the merged list a run can REACH
//! without naming them. The merged `providers` list is the CATALOG — every row the
//! operator wrote plus every preset the embedded `defaults.toml` ships. The routing
//! TABLE is the subset an act of the operator's has touched: a row they NAMED in a
//! config file, or a preset a CREDENTIAL resolves for. One predicate, read by
//! routing's greedy scan and by `--list-providers`, so the listing shows exactly what
//! a bare `bz "q"` can reach. By-name selectors (`--provider`, `--login --provider`)
//! read the catalog: naming the row IS the act.

use crate::auth::cred_state;
use crate::config::partial::PartialProvider;
use crate::config::provider::AuthId;
use crate::store::{CredStore, Secret};

/// Is this row in the routing table? A row the operator named is in it outright (its
/// `preset` bit was cleared by the fold); a catalog preset is in it only while a
/// credential resolves for it — `inline` (`--api-key`/`BRAZEN_API_KEY`, provider-
/// agnostic, so it activates every keyed preset), `stored` (`bz --login`), or
/// `ambient` (a sibling tool's sign-in, the vendor env var). A keyless preset
/// (`auth = "none"`) has no credential to resolve and so enters only by name.
pub(crate) fn routable(
    name: &str,
    row: &PartialProvider,
    inline: Option<&Secret>,
    store: &dyn CredStore,
) -> bool {
    !row.preset
        || cred_state(
            store,
            name,
            row.auth.unwrap_or(AuthId::None),
            row.ambient.as_ref(),
            inline,
        )
        .activates()
}
