//! `specs/release.md` §4 in code: the per-case declaration of whether a live
//! assertion is deterministic, and the bounded retry that declaration buys.
//!
//! §4 rules that "every live assertion is *deterministic* unless the suite declares
//! that case model-discretion, and an undeclared case is therefore blocking", so the
//! declaration is a FIELD ON THE CASE, authored when the case is authored (bl-959b).
//! The release gate (`scripts/release-check.sh`) therefore needs no classification
//! list of its own — a second list outside the case is exactly what would drift, and
//! the retry runs where the classification lives instead of being a human action.
//!
//! Provider-agnostic on purpose: the policy is the repo's, not any one backend's, so
//! a second live suite reuses this leaf rather than re-spelling the rule.

// Each consumer declares only the variants its own cases use.
#![allow(dead_code)]

/// Whether a live case's answer is a fact about the wire dialect or a model choice.
#[derive(Clone, Copy)]
pub enum Determinism {
    /// A property of the dialect: exit codes, auth outcomes, whether the service
    /// accepted or rejected the wire shape, the canonical event grammar. One
    /// attempt — a failure is a defect in brazen or a real upstream dialect change,
    /// and it BLOCKS the release until it is resolved in the tree.
    Deterministic,
    /// The answer depends on what the model *chose* to emit — a reasoning summary it
    /// may skip, a tool it may decline to call, text a thinking budget may starve.
    Discretion,
}

/// §4's "re-run up to three times" as a number.
const DISCRETION_ATTEMPTS: u32 = 3;

impl Determinism {
    /// The case's attempt budget. A deterministic case is the same path with a
    /// budget of one, so the retry is never a branch on the case kind.
    pub fn attempts(self) -> u32 {
        match self {
            Determinism::Deterministic => 1,
            Determinism::Discretion => DISCRETION_ATTEMPTS,
        }
    }
}

/// Run `attempt` under the case's budget: the first `Ok` is green ("passing any run
/// is green"), an exhausted budget returns the LAST error. A discretion case that
/// burns its budget says so in the message, because §4 then permits the release only
/// with a signoff naming the case, the provider and model, plus a filed `bl` ball —
/// the gate quotes that line into the release PR comment, never re-deriving it.
pub fn under(
    label: &str,
    det: Determinism,
    mut attempt: impl FnMut() -> Result<(), String>,
) -> Result<(), String> {
    let budget = det.attempts();
    let mut last = String::new();
    for n in 1..=budget {
        match attempt() {
            Ok(()) => {
                if n > 1 {
                    println!("  {label:<22} model-discretion: passed on attempt {n}/{budget}");
                }
                return Ok(());
            }
            Err(e) => {
                if n < budget {
                    println!("  {label:<22} model-discretion: attempt {n}/{budget} failed ({e}) — re-running (release.md §4)");
                }
                last = e;
            }
        }
    }
    if budget == 1 {
        Err(last)
    } else {
        Err(format!(
            "DISCRETION exhausted — {budget} attempts, all failed: {last}"
        ))
    }
}

/// `under` is pure (a closure and its budget), so §4's retry rule is provable without
/// a network, a credential, or a token: these run in `make check` alongside everything
/// else. The live cases prove the WIRE; these prove the POLICY.
#[cfg(test)]
mod tests {
    use super::{under, Determinism};
    use std::cell::Cell;

    /// A closure failing its first `fails` calls, counting every call.
    fn flaky(calls: &Cell<u32>, fails: u32) -> impl FnMut() -> Result<(), String> + '_ {
        move || {
            calls.set(calls.get() + 1);
            if calls.get() <= fails {
                Err(format!("empty summary (run {})", calls.get()))
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn a_deterministic_case_gets_exactly_one_attempt_and_its_error_verbatim() {
        let calls = Cell::new(0);
        let r = under("det-red", Determinism::Deterministic, flaky(&calls, 9));
        assert_eq!(calls.get(), 1);
        assert_eq!(r, Err("empty summary (run 1)".to_owned()));
    }

    #[test]
    fn a_green_first_run_never_retries_whatever_the_budget() {
        for det in [Determinism::Deterministic, Determinism::Discretion] {
            let calls = Cell::new(0);
            assert_eq!(under("green", det, flaky(&calls, 0)), Ok(()));
            assert_eq!(calls.get(), 1);
        }
    }

    /// §4: "a declared-discretion case is re-run up to three times; passing any run is green".
    #[test]
    fn a_discretion_case_passing_on_a_later_run_is_green_and_stops_there() {
        let calls = Cell::new(0);
        assert_eq!(
            under("late-green", Determinism::Discretion, flaky(&calls, 2)),
            Ok(())
        );
        assert_eq!(calls.get(), 3);
    }

    /// §4: failing all three ships only with a signoff — so the message must NAME the
    /// exhaustion (the gate quotes this line into the release PR comment).
    #[test]
    fn a_discretion_case_failing_every_run_reports_an_exhausted_budget() {
        let calls = Cell::new(0);
        let e = under("all-red", Determinism::Discretion, flaky(&calls, 9)).unwrap_err();
        assert_eq!(calls.get(), Determinism::Discretion.attempts());
        assert!(e.starts_with("DISCRETION exhausted — 3 attempts"), "{e}");
        assert!(e.ends_with("empty summary (run 3)"), "{e}");
    }
}
