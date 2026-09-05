//! The two output shapes of `bz --list-providers` (config §6.1) — the `--json` object
//! and the padded text table — over the ONE [`Row`] the verb builds. A separate module
//! only so neither half crowds the other; both read the same struct, so the table and
//! the object can never name different facts.

use std::io::Write;

use super::Row;

/// Print the listing (config §6.1): `--json` the one `{"providers":[…]}` object
/// (serde-direct, like the event stream), else the seven columns space-padded to the
/// widest value, one row per line, no header — greppable and `awk`-able, the same
/// "one line per listed thing" shape `--list-models` prints. An empty table prints
/// nothing and exits 0: the loop over zero rows, not an empty-listing branch.
///
/// Each CAPABILITY group renders as ONE column naming the members the row has, `-` for
/// none, rather than as bare `true`/`false` columns under no header: `tuning`
/// (`effort,priority` — which knobs the row accepts) and `shapes` (`tools,multi_turn` —
/// which request shapes its dialect can carry, bl-5053). Same facts as the object's
/// booleans, one line each, and `grep tools` is the question an operator asks.
/// `credential` is the one column whose value can contain a space (`not required`), so
/// it stays LAST and whitespace-splitting a line keeps working; `device` sits beside it
/// for the same reason.
pub(super) fn print_rows(out: &mut dyn Write, rows: &[Row], json: bool) -> std::io::Result<()> {
    if json {
        let obj = serde_json::json!({ "providers": rows });
        return writeln!(out, "{obj}");
    }
    let tunings: Vec<String> = rows.iter().map(tuning_cell).collect();
    let shapes: Vec<String> = rows.iter().map(shapes_cell).collect();
    let devices: Vec<String> = rows.iter().map(device_cell).collect();
    let (name, protocol, auth) = (
        width(rows, |r| &r.name),
        width(rows, |r| &r.protocol),
        width(rows, |r| &r.auth),
    );
    let tuning = cell_width(&tunings);
    let shape = cell_width(&shapes);
    let device = cell_width(&devices);
    for (((r, t), s), d) in rows.iter().zip(&tunings).zip(&shapes).zip(&devices) {
        writeln!(
            out,
            "{:name$}  {:protocol$}  {:auth$}  {:tuning$}  {:shape$}  {:device$}  {}",
            r.name, r.protocol, r.auth, t, s, d, r.credential
        )?;
    }
    Ok(())
}

/// The `tuning` cell: the accepted knobs by name, comma-joined in the column's own
/// order, `-` when the row accepts neither — the text rendering of the two booleans
/// the `--json` shape carries, never a second computation of them.
fn tuning_cell(r: &Row) -> String {
    named(&[("effort", r.effort), ("priority", r.priority)])
}

/// The `shapes` cell: the request shapes this row's dialect can CARRY, by name, `-`
/// when it can carry neither (bl-5053). A `claude_code` row reads `-` here, which is
/// the whole point — the refusal was previously visible only as a `ParseInput` at call
/// time, to a caller that had already built the request.
fn shapes_cell(r: &Row) -> String {
    named(&[("tools", r.tools), ("multi_turn", r.multi_turn)])
}

/// The `device` cell: the headless flow this row serves by name, `-` when it serves
/// none — the text rendering of the `--json` shape's `device`, never a second read of
/// the row.
fn device_cell(r: &Row) -> String {
    r.device.clone().unwrap_or_else(|| "-".to_owned())
}

/// One capability cell: the members the row HAS, comma-joined in the column's own
/// order, `-` for none. The shared shape of `tuning` and `shapes` — two columns, one
/// rendering, so a third capability group is a call, not a copy.
fn named(members: &[(&str, bool)]) -> String {
    let held: Vec<&str> = members
        .iter()
        .filter_map(|(name, yes)| yes.then_some(*name))
        .collect();
    if held.is_empty() {
        return "-".to_owned();
    }
    held.join(",")
}

/// The widest value of one column, the padding every row aligns to.
fn width(rows: &[Row], field: impl Fn(&Row) -> &String) -> usize {
    rows.iter().map(|r| field(r).len()).max().unwrap_or(0)
}

/// The widest of an already-rendered column's cells — `width` for the cells computed
/// above rather than read off the row.
fn cell_width(cells: &[String]) -> usize {
    cells.iter().map(String::len).max().unwrap_or(0)
}
