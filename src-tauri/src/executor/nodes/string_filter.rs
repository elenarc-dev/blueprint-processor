//! `string.filter` — keep only elements of `in` (an array of
//! strings) whose relationship with `needle` matches the active
//! `mode` parameter.
//!
//! Six modes split into two complementary halves:
//!
//!   Positive — keep slots where the predicate is `true`:
//!     * `contains`     — slot contains needle
//!     * `startsWith`   — slot starts with needle
//!     * `endsWith`     — slot ends with needle
//!
//!   Negative — keep slots where the predicate is `false`:
//!     * `notContains`     — slot does NOT contain needle
//!     * `notStartsWith`   — slot does NOT start with needle
//!     * `notEndsWith`     — slot does NOT end with needle
//!
//! **Empty needle semantics.** Mirrors `<str>::contains` /
//! `starts_with` / `ends_with`:
//!   * positive modes   — every slot matches (every string
//!     contains/starts with/ends with the empty string)
//!   * negative modes   — nothing matches (because "doesn't
//!     contain empty" is false for every non-empty string,
//!     and `null`/`empty` slots coerce to `""` which trivially
//!     does not start with `""` for the notStartsWith mode).
//! Pinning this so a future refactor that "fixes" empty-needle
//! handling cannot slip in silently — the catalog description
//! calls it out and lets the user pick an explicit non-empty
//! needle when they want a strict intent.
//!
//! **Case-sensitive** by default — matches `<str>` methods.
//!
//! **Element-wise coercion** mirrors `string.replace`:
//! non-string array slots flow through `value_to_string` before
//! the predicate runs, so heterogeneous arrays
//! (`["foo", 42, true, null]`) produce a coherent filter result
//! alongside length preservation. `null` becomes `""`; numbers
//! render to digits (`42` -> "42") and a `true` becomes the
//! literal text `"true"`. The catalog description documents
//! this so users wiring a heterogeneous producer upstream don't
//! have to consult the executor source.
//!
//! **Port-vs-param precedence** mirrors `image.format` and
//! `string.replace`: the wired `needle` port wins over the
//! `params.needle` body text input. Empty-string or whitespace-
//! only wired values collapse to the param fallback so a
//! placeholder literal wired into the port does NOT silently
//! override an intentional param choice (same trap-avoidance
//! as `image.format`'s empty-collapse-to-fallback semantics).
//! Non-string wired values coerce via `Value::to_string()` so a
//! `Number` wired into the `needle` port drives the predicate
//! against its digits.

use serde_json::{Map, Value};

use crate::executor::{single, value_to_string, EvalCtx, ExecError, NodeInputs, NodeOutputs};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum FilterMode {
    Contains,
    StartsWith,
    EndsWith,
    NotContains,
    NotStartsWith,
    NotEndsWith,
}

impl FilterMode {
    /// Parse the `mode` param string. The set of legal values is
    /// closed — directly compared against the literal keys in
    /// the catalog `select` so a typo surfaces as a clear
    /// runtime error rather than silently defaulting.
    fn from_param(s: &str) -> Result<Self, ExecError> {
        match s {
            "contains" => Ok(Self::Contains),
            "startsWith" => Ok(Self::StartsWith),
            "endsWith" => Ok(Self::EndsWith),
            "notContains" => Ok(Self::NotContains),
            "notStartsWith" => Ok(Self::NotStartsWith),
            "notEndsWith" => Ok(Self::NotEndsWith),
            other => Err(ExecError::Node {
                node: "string.filter".into(),
                message: format!(
                    "unknown mode '{other}'; supported: contains, startsWith, endsWith, \
                     notContains, notStartsWith, notEndsWith"
                ),
            }),
        }
    }

    /// Run the predicate against a single haystack + needle pair.
    /// Pulled out of `exec` so the six `match` arms stay one line
    /// each and a future mode addition is a four-line diff.
    fn matches(&self, haystack: &str, needle: &str) -> bool {
        match self {
            Self::Contains => haystack.contains(needle),
            Self::StartsWith => haystack.starts_with(needle),
            Self::EndsWith => haystack.ends_with(needle),
            Self::NotContains => !haystack.contains(needle),
            Self::NotStartsWith => !haystack.starts_with(needle),
            Self::NotEndsWith => !haystack.ends_with(needle),
        }
    }
}

/// Resolve the search-needle PATTERN SET (a `Vec<String>`, possibly
/// empty-singleton when only a scalar string is provided) with the
/// same precedence contract as `image.format::resolve_format`:
///   1. **Wired `needle` input port** — accepts a SCALAR string,
///      a SCALAR non-string (Number / Bool / Null coerced via
///      `Value::to_string()`), OR an ARRAY of strings. Each pattern
///      slot goes through trim + filter-empties before the
///      predicate runs, so a heterogeneous array with `\"\"` or
///      `null` slots collapses to its non-empty subset — pin so
///      the user can't inject a wildcard via a `null` slot.
///
///      Empty SCALAR wired (after trim) AND `Value::Null` wired
///      collapse to step 2 — same empty-collapse-to-fallback
///      contract as `image.format::resolve_format`.
///
///      Empty ARRAY wired (after trim+filter) is taken as-is,
///      NOT as a fall-through signal. The pattern set is "zero
///      patterns", so the predicate `iter().any()` over an
///      empty set is `false` → drops every input slot. This is
///      intentionally different from the scalar-empty case so a
///      user wiring `array.create(count=0)` doesn't accidentally
///      bypass their param fallback.
///
///   2. **Legacy `params.needle` entry** — same scalar / array /
///      empty-collapse semantics as step 1. Back-compat: blueprints
///      authored before this port accepted arrays keep working.
///
///   3. **Default** — `[<empty string>]`. Under positive modes
///      this is a no-op (every string contains `\"\"`); under
///      negative modes it drops every slot. Same backward-compat
///      default as the single-string predecessor.
///
/// The returned `Vec<String>` is the active pattern set.
/// `exec()` runs the predicate as
/// `iter().any(|p| mode.matches(slot, p))` — EXISTENTIAL — so
/// for `notContains` the slot passes if at least one pattern is
/// NOT contained by it (only slots that match EVERY pattern get
/// dropped; equivalent to `NOT all-match`).
fn resolve_needle_patterns(inputs: &NodeInputs<'_>, params: &Map<String, Value>) -> Vec<String> {
    if let Some(raw) = inputs.get("needle").copied() {
        let pats = patterns_from(&raw);
        // Empty SCALAR wired (or Null) → fall through. Empty
        // ARRAY wired is NOT a fallthrough signal — `pats.is_empty()`
        // here would be a deliberate "no patterns exist" outcome
        // and the predicate `iter().any()` on `[]` drops every slot.
        if !is_scalar_empty_signal(&raw) {
            return pats;
        }
    }
    if let Some(raw) = params.get("needle") {
        let pats = patterns_from(raw);
        if !is_scalar_empty_signal(raw) {
            return pats;
        }
    }
    // Default — single empty pattern preserves the historical
    // single-string "keep-everything-for-positive / drop-all-for-negative"
    // semantics so a pre-multi-pattern blueprint still runs.
    vec![String::new()]
}

/// Convert a needle `Value` into a `Vec<String>` of patterns.
/// Each pattern is the trimmed text form. Empty patterns (after
/// trim) are dropped from arrays; scalar strings keep one slot
/// even if trimmed to `\"\"` so the upstream `is_scalar_empty_signal`
/// check decides the fall-through semantics for that case
/// (RATHER than collapsing via this helper).
fn patterns_from(v: &Value) -> Vec<String> {
    match v {
        Value::String(s) => vec![s.trim().to_string()],
        Value::Array(items) => items
            .iter()
            .map(|slot| value_to_string(slot).trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        // Number / Bool / Object coerced via canonical `Value::to_string()`.
        // (`Value::Null` is mapped to `\"\"` here purely for
        // completeness; the upstream `is_scalar_empty_signal`
        // check sees `Value::Null` directly and triggers fall-through.)
        other => vec![other.to_string().trim().to_string()],
    }
}

/// Whether the source value should be treated as an empty-scalar
/// fall-through signal — a `Value::String` whose trimmed content
/// is empty, OR a `Value::Null`. Other shapes (Number / Bool /
/// Array / Object) are always taken as usable patterns, even
/// when their text form trims to `\"\"`.
fn is_scalar_empty_signal(v: &Value) -> bool {
    match v {
        Value::String(s) => s.trim().is_empty(),
        Value::Null => true,
        _ => false,
    }
}

pub fn exec(
    inputs: &NodeInputs<'_>,
    params: &Map<String, Value>,
    _ctx: &mut EvalCtx<'_>,
) -> Result<NodeOutputs, ExecError> {
    let patterns = resolve_needle_patterns(inputs, params);

    // `params.mode` is the catalogue `select` default of
    // `"contains"`, so the `unwrap_or` fallback to `Contains`
    // only fires when the param is literally absent (deliberately
    // omitted). A typo like `"Contain"` (capital C) or `"regex"`
    // is NOT swallowed — `from_param` returns an `Err` and the
    // chain's `?` propagates it so the runner surfaces the
    // legals list to the user.
    // Explicit `match` over `Option<&Value>` so a `params.mode`
    // that's PRESENT-but-wrong-shape (Number / Bool / Null /
    // Array, e.g. from a hand-edited saved blueprint or a JSON
    // migration that lost the string) errors via
    // `ExecError::Node` instead of silently collapsing to
    // `FilterMode::Contains`. The category dropdown UI coerces
    // so this is out-of-band defense-in-depth — not a
    // precedence contract like `image.format::resolve_format`'s
    // wired-port-over-param logic, which covers an unrelated
    // concern.
    let mode = match params.get("mode") {
        None => Ok(FilterMode::Contains),
        Some(v) => FilterMode::from_param(v.as_str().ok_or_else(|| ExecError::Node {
            node: "string.filter".into(),
            message: format!(
                "`mode` must be a string (got {})",
                crate::executor::value_type_name(v)
            ),
        })?),
    }?;

    // `params.matchAll` flips the multi-pattern predicate from
    // existential any-of-matches (`iter().any(...)`) to universal
    // all-of-matches (`iter().all(...)`). Defaults to `false` so a
    // blueprint authored BEFORE this toggle (or one that simply
    // omits the param) preserves its existing any-semantics —
    // back-compat, NOT a "matchAll is the new default" flip. A
    // PRESENT-but-non-bool value collapses to `false` via
    // `as_bool().unwrap_or(false)` — defensive against hand-edited
    // saved blueprints where, e.g. `matchAll: "true"` (string)
    // slipped past the boolean catalog UI coercion, mirroring the
    // lenient-shape read used by `image.format::resolve_format`'s
    // `as_str().unwrap_or_else(...)`.
    let match_all = params
        .get("matchAll")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let null = Value::Null;
    let provided = inputs.get("in").copied().unwrap_or(&null);

    // Predicate dispatch is extracted into a `Fn` closure so the
    // scalar branch and the array branch stay one-liners. Two
    // concerns collapse in here:
    //
    //   1. **`matchAll` toggle** — `iter().any(...)` when
    //      `match_all == false` (default; multi-pattern is
    //      EXISTENTIAL — keep a slot iff AT LEAST ONE pattern
    //      satisfies the mode) and `iter().all(...)` when
    //      `match_all == true` (multi-pattern is UNIVERSAL —
    //      keep a slot iff EVERY pattern satisfies the mode,
    //      which under negative modes means EVERY pattern must
    //      miss, equivalently drops only slots that satisfy
    //      every pattern).
    //
    //   2. **EMPTY-PATTERN KILL SWITCH** — when the resolved
    //      pattern set is empty (after `patterns_from`'s
    //      trim+filter pipeline), drop every input slot
    //      REGARDLESS of `match_all`. Naively, `all()` over an
    //      empty set is vacuously `true`, which would silently
    //      turn the filter into a pass-through under
    //      `matchAll=true` — the OPPOSITE outcome of the
    //      `match_all == false` branch. Rejecting the empty
    //      case up-front keeps the `match_all == false` kill-
    //      switch AND prevents an upstream `array.create(count=0)`
    //      from disappearing into a no-op when the user toggles
    //      `Match All`.
    let predicate = |slot: &str| -> bool {
        if patterns.is_empty() {
            return false;
        }
        if match_all {
            patterns.iter().all(|p| mode.matches(slot, p))
        } else {
            patterns.iter().any(|p| mode.matches(slot, p))
        }
    };

    // Shape handling mirrors `string.replace`: a scalar
    // `Value::String` wraps as a one-element array so the output
    // shape is always `Value::Array` (keeps downstream `array.*`
    // nodes consistent regardless of whether the producer was
    // a scalar or an array). Empty array passes through as
    // empty; `Value::Null` errors visibly so a missing-wire
    // mistake doesn't silently emit `[]`.
    match provided {
        Value::String(s) => {
            let out = if predicate(s.as_str()) {
                vec![Value::String((*s).clone())]
            } else {
                Vec::new()
            };
            Ok(single("out", Value::Array(out)))
        }
        Value::Array(items) => {
            let mut out: Vec<Value> = Vec::with_capacity(items.len());
            for slot in items.iter() {
                // Same weak coercion rule as `string.replace`:
                // `value_to_string` collapses Numbers/Booleans/
                // Null to canonical JSON text before the predicate.
                let s = value_to_string(slot);
                if predicate(&s) {
                    out.push(Value::String(s));
                }
            }
            Ok(single("out", Value::Array(out)))
        }
        Value::Null => Err(ExecError::Node {
            node: "string.filter".into(),
            message: "no array provided (connect `in` with a string or string-array) \
                      — empty/null inputs error so a missing wire is visible instead of \
                      silently emitting `[]`"
                .into(),
        }),
        other => Err(ExecError::Node {
            node: "string.filter".into(),
            message: format!(
                "`in` must be a string or array of strings (got {})",
                crate::executor::value_type_name(other)
            ),
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde_json::{json, Value};

    use super::exec;
    use super::ExecError;
    use crate::executor::nodes::test_helpers::ctx_with;

    /// Run with a fixed `in` value + optional `(port_name, &Value)`
    /// pairs for any wired input. Mirrors the `run_wired` helper
    /// in `string_replace.rs::tests` so the test surface for both
    /// String-group nodes stays consistent.
    fn run_with(
        params: serde_json::Map<String, Value>,
        in_val: Value,
        extra: &[(&str, &Value)],
    ) -> Result<HashMap<String, Value>, ExecError> {
        let mut outs = HashMap::new();
        let mut logs = Vec::new();
        let external = HashMap::new();
        let mut inputs: HashMap<String, &Value> = HashMap::new();
        inputs.insert("in".to_string(), &in_val);
        for &(k, v) in extra {
            inputs.insert(k.to_string(), v);
        }
        let mut ctx = ctx_with("n1", "string.filter", &external, &mut outs, &mut logs);
        exec(&inputs, &params, &mut ctx)
    }

    fn params_with(mode: &str, needle: &str) -> serde_json::Map<String, Value> {
        json!({ "mode": mode, "needle": needle })
            .as_object()
            .unwrap()
            .clone()
    }

    /// Pull the output array out of the result map as a `Vec<String>`.
    /// Most tests don't care about the wrapping `Value` plumbing —
    /// they want the textual outcome of the filter.
    fn extract_array(r: HashMap<String, Value>) -> Vec<String> {
        r.get("out")
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect()
    }

    // ---------- six modes (positive pins) ----------

    #[test]
    fn filters_contains_mode() {
        let params = params_with("contains", "oo");
        let r = run_with(
            params,
            Value::Array(vec![
                Value::String("food".into()),
                Value::String("bar".into()),
                Value::String("foobar".into()),
            ]),
            &[],
        )
        .unwrap();
        assert_eq!(extract_array(r), vec!["food", "foobar"]);
    }

    #[test]
    fn filters_starts_with_mode() {
        let params = params_with("startsWith", "img");
        let r = run_with(
            params,
            Value::Array(vec![
                // `image-001.png` does NOT starts_with("img"):
                // the strings diverge at the 3rd character
                // (i-m-a-... vs i-m-g-...). The fixture is
                // deliberately mixed so an off-by-one in the
                // prefix check (`starts_with` swapped for
                // `contains`, etc.) fails loud.
                Value::String("image-001.png".into()),     // no
                Value::String("video.mp4".into()),         // no
                Value::String("img-thumbnail.jpg".into()), // yes
            ]),
            &[],
        )
        .unwrap();
        assert_eq!(extract_array(r), vec!["img-thumbnail.jpg"]);
    }

    #[test]
    fn filters_ends_with_mode() {
        let params = params_with("endsWith", ".png");
        let r = run_with(
            params,
            Value::Array(vec![
                Value::String("a.png".into()),
                Value::String("b.jpg".into()),
                Value::String("c.png".into()),
            ]),
            &[],
        )
        .unwrap();
        assert_eq!(extract_array(r), vec!["a.png", "c.png"]);
    }

    // ---------- six modes (negative pins) ----------

    #[test]
    fn filters_not_contains_mode() {
        let params = params_with("notContains", "draft");
        let r = run_with(
            params,
            Value::Array(vec![
                Value::String("final.txt".into()),
                Value::String("draft-1.txt".into()),
                Value::String("final-2.txt".into()),
            ]),
            &[],
        )
        .unwrap();
        assert_eq!(extract_array(r), vec!["final.txt", "final-2.txt"]);
    }

    #[test]
    fn filters_not_starts_with_mode() {
        let params = params_with("notStartsWith", "_");
        let r = run_with(
            params,
            Value::Array(vec![
                Value::String("public".into()),
                Value::String("_internal".into()),
                Value::String("release".into()),
            ]),
            &[],
        )
        .unwrap();
        assert_eq!(extract_array(r), vec!["public", "release"]);
    }

    #[test]
    fn filters_not_ends_with_mode() {
        let params = params_with("notEndsWith", ".bak");
        let r = run_with(
            params,
            Value::Array(vec![
                Value::String("a.txt".into()),
                Value::String("b.bak".into()),
                Value::String("c.txt".into()),
            ]),
            &[],
        )
        .unwrap();
        assert_eq!(extract_array(r), vec!["a.txt", "c.txt"]);
    }

    #[test]
    fn case_sensitive_does_not_match_different_case() {
        // Pins the case-sensitive contract the catalog and
        // doc-block both claim. A future refactor that wraps
        // the predicate in
        // `to_lowercase().contains(&to_lowercase(needle))`
        // would silently regress case behaviour — this test
        // is the regression tripwire. Exercising all four
        // present/case-variants of the same filename makes the
        // intent obvious at-a-glance.
        let params = params_with("endsWith", ".PNG");
        let r = run_with(
            params,
            Value::Array(vec![
                Value::String("photo.png".into()), // no: case differs
                Value::String("photo.PNG".into()), // yes: exact case
                Value::String("PHOTO.png".into()), // no: case differs
                Value::String("PHOTO.PNG".into()), // yes: exact case
            ]),
            &[],
        )
        .unwrap();
        assert_eq!(extract_array(r), vec!["photo.PNG", "PHOTO.PNG"]);
    }

    // ---------- shape behaviour (mirrors string.replace) ----------

    #[test]
    fn empty_array_passes_through_as_empty() {
        let params = params_with("contains", "anything");
        let r = run_with(params, Value::Array(Vec::new()), &[]).unwrap();
        assert!(r.get("out").unwrap().as_array().unwrap().is_empty());
    }

    #[test]
    fn scalar_string_input_wraps_as_one_element_array() {
        let params = params_with("contains", "abc");
        let r = run_with(params, Value::String("abcdef".into()), &[]).unwrap();
        assert_eq!(extract_array(r), vec!["abcdef"]);
    }

    #[test]
    fn scalar_string_input_under_no_match_emits_empty() {
        let params = params_with("endsWith", ".png");
        let r = run_with(params, Value::String("hello.txt".into()), &[]).unwrap();
        assert!(r.get("out").unwrap().as_array().unwrap().is_empty());
    }

    #[test]
    fn null_input_errors() {
        let params = params_with("contains", "anything");
        let err = run_with(params, Value::Null, &[]).unwrap_err();
        assert!(
            matches!(err, ExecError::Node { ref message, .. } if message.contains("no array")),
            "expected no-array message, got: {err:?}"
        );
    }

    // ---------- mode-param error / default behaviour ----------

    #[test]
    fn unknown_mode_errors() {
        let params = params_with("fuzzy", "anything");
        let err =
            run_with(params, Value::Array(vec![Value::String("foo".into())]), &[]).unwrap_err();
        assert!(
            matches!(err, ExecError::Node { ref message, .. }
                if message.contains("unknown mode")),
            "expected unknown-mode message, got: {err:?}"
        );
    }

    #[test]
    fn defaults_to_contains_when_mode_param_missing() {
        let params = json!({ "needle": "oo" }).as_object().unwrap().clone();
        let r = run_with(
            params,
            Value::Array(vec![
                Value::String("food".into()),
                Value::String("bar".into()),
            ]),
            &[],
        )
        .unwrap();
        assert_eq!(extract_array(r), vec!["food"]);
    }

    // ---------- empty needle semantics (mirror JS String) ----------

    #[test]
    fn empty_needle_positive_mode_keeps_everything() {
        // `contains(\"\")` returns true for ALL strings per the
        // `<str>::contains` contract. Same for `starts_with` and
        // `ends_with` — empty string is a prefix/suffix of every
        // string by definition. So an empty needle in a positive
        // mode acts as a pass-everything filter; pin that.
        for mode in &["contains", "startsWith", "endsWith"] {
            let params = params_with(mode, "");
            let r = run_with(
                params,
                Value::Array(vec![
                    Value::String("foo".into()),
                    // Empty slot TRIVIALLY matches the empty needle
                    // in positive modes (\"\".contains(\"\") == true),
                    // and `value_to_string(Value::Null) == \"\"` so a
                    // null slot would coalesce into the same outcome
                    // we pin here for the explicit empty case.
                    Value::String(String::new()),
                    Value::String("bar".into()),
                ]),
                &[],
            )
            .unwrap();
            assert_eq!(
                extract_array(r),
                vec!["foo", "", "bar"],
                "positive mode '{mode}' should keep every slot (incl. empty) with empty needle",
            );
        }
    }

    #[test]
    fn non_empty_needle_does_not_match_empty_haystack_for_positive_modes() {
        // Pin the symmetric edge of the empty-string contract:
        // `\"\".contains(\"non-empty\") == false`,
        // `\"\".starts_with(\"non-empty\") == false`,
        // `\"\".ends_with(\"non-empty\") == false`. A future
        // refactor that early-returns `true` on
        // `haystack.is_empty()` (looking like a perf win) would
        // have to land loud and break this test. Mixing
        // explicit-empty slots with `Value::Null` exercises both
        // shapes' pass through `value_to_string`.
        for mode in &["contains", "startsWith", "endsWith"] {
            let params = params_with(mode, "non-empty-pattern");
            let r = run_with(
                params,
                Value::Array(vec![
                    Value::String(String::new()), // explicit empty
                    Value::Null,                  // coerces to ""
                ]),
                &[],
            )
            .unwrap();
            assert!(
                r.get("out").unwrap().as_array().unwrap().is_empty(),
                "positive mode '{mode}' should NOT match empty haystack with non-empty needle"
            );
        }
    }

    #[test]
    fn empty_needle_negative_mode_keeps_nothing() {
        // Mirror: positive modes pass-through and negative modes
        // match nothing with empty needle. The catalog calls this
        // out so users pick an explicit non-empty needle when they
        // want a strict intent.
        for mode in &["notContains", "notStartsWith", "notEndsWith"] {
            let params = params_with(mode, "");
            let r = run_with(
                params,
                Value::Array(vec![
                    Value::String("foo".into()),
                    Value::String("bar".into()),
                ]),
                &[],
            )
            .unwrap();
            assert!(
                r.get("out").unwrap().as_array().unwrap().is_empty(),
                "negative mode '{mode}' should drop every slot with empty needle"
            );
        }
    }

    // ---------- element-wise coercion ----------

    #[test]
    fn coerces_non_string_array_slot_via_value_to_string() {
        // Same weak-coercion rule as `string.replace`: a heterogeneous
        // array of strings, numbers, booleans, null all flow through
        // `value_to_string` before the predicate runs. Numbers
        // become digits (`42` -> \"42\"), booleans -> \"true\" /
        // \"false\", null -> \"\".
        let params = params_with("contains", "42");
        let r = run_with(
            params,
            Value::Array(vec![
                Value::String("abort".into()), // s: \"abort\".contains(\"42\")? No
                Value::from(42),               // s: \"42\".contains(\"42\")? Yes
                Value::String("id-42".into()), // s: \"id-42\".contains(\"42\")? Yes
                Value::Bool(true),             // s: \"true\".contains(\"42\")? No
                Value::Null,                   // s: \"\".contains(\"42\")? No
            ]),
            &[],
        )
        .unwrap();
        assert_eq!(extract_array(r), vec!["42", "id-42"]);
    }

    // ---------- port-vs-param precedence on `needle` ----------

    #[test]
    fn wired_needle_port_overrides_param_needle() {
        // params.needle would never match; wired needle says
        // \"img\" -> filter keeps only the startsWith=\"img\" slots.
        // The output proves the PORT wins.
        let params = json!({
            "mode": "startsWith",
            "needle": "ZZZ_NEVER_MATCHES"
        })
        .as_object()
        .unwrap()
        .clone();
        let needle_port = Value::String("img".into());
        let r = run_with(
            params,
            Value::Array(vec![
                // Same prefix trap as `filters_starts_with_mode`:
                // `image.png` does NOT starts_with("img") so the
                // expected output trims it. Pinning that here
                // catches a future special-case for the literal
                // "image-" prefix or a `.contains()`-instead-of-
                // `.starts_with()` refactor.
                Value::String("image.png".into()), // no: 3rd char 'a'
                Value::String("style.css".into()), // no
                Value::String("img.bin".into()),   // yes
            ]),
            &[("needle", &needle_port)],
        )
        .unwrap();
        assert_eq!(extract_array(r), vec!["img.bin"]);
    }

    #[test]
    fn empty_wired_needle_falls_back_to_param_needle() {
        // Wire empty string into needle, param has \"img\". Output
        // keeps only startsWith=\"img\" slots — same precedence
        // contract as `image.format::resolve_format`: empty
        // wired collapses to the param fallback.
        let params = json!({ "mode": "startsWith", "needle": "img" })
            .as_object()
            .unwrap()
            .clone();
        let needle_port = Value::String("".into());
        let r = run_with(
            params,
            Value::Array(vec![
                // Same prefix trap —
                // `image.png`.starts_with("img") is false (strings
                // diverge at the 3rd character). Keep it in the
                // fixture as a NON-match so the test catches any
                // future "image-path-is-special" shortcut.
                Value::String("image.png".into()), // no: 3rd char 'a'
                Value::String("img.bin".into()),   // yes
                Value::String("script.js".into()), // no
            ]),
            &[("needle", &needle_port)],
        )
        .unwrap();
        assert_eq!(extract_array(r), vec!["img.bin"]);
    }

    #[test]
    fn whitespace_only_wired_needle_falls_back_to_param() {
        // Same trim-then-collapse branch as `image.format`:
        // whitespace-only wired (\"   \\t\\n  \") trims to \"\" which
        // is empty and falls through to the param.
        let params = json!({ "mode": "contains", "needle": "foo" })
            .as_object()
            .unwrap()
            .clone();
        let needle_port = Value::String("   \t\n  ".into());
        let r = run_with(
            params,
            Value::Array(vec![
                Value::String("football".into()),
                Value::String("rugby".into()),
            ]),
            &[("needle", &needle_port)],
        )
        .unwrap();
        assert_eq!(extract_array(r), vec!["football"]);
    }

    #[test]
    fn non_string_wired_needle_runs_against_coerced_text() {
        // A `Value::Number(42)` wired into the needle port goes
        // through `to_string()` (`\"42\"`) and the predicate runs
        // against that text. params.needle would never match;
        // the wire wins AND the coercion path is exercised.
        let params = json!({ "mode": "contains", "needle": "ZZZ_NEVER_MATCHES" })
            .as_object()
            .unwrap()
            .clone();
        let needle_port = Value::from(42);
        let r = run_with(
            params,
            Value::Array(vec![
                Value::String("id-42".into()),
                Value::String("foo".into()),
                Value::String("42-pure".into()),
            ]),
            &[("needle", &needle_port)],
        )
        .unwrap();
        assert_eq!(extract_array(r), vec!["id-42", "42-pure"]);
    }

    // ---------- multi-pattern needle (any-of existential semantics) ----------

    #[test]
    fn filters_array_needle_intersects_with_any_pattern() {
        // Multi-pattern needle: `contains ["foo", "bar"]` keeps a
        // slot iff it satisfies the mode for AT LEAST ONE pattern
        // — existential OR-of-matches. Pins the user-specified
        // "al menos para alguno" semantics literally.
        let params = json!({ "mode": "contains", "needle": ["foo", "bar"] })
            .as_object()
            .unwrap()
            .clone();
        let r = run_with(
            params,
            Value::Array(vec![
                Value::String("food".into()),     // contains "foo" → KEEP
                Value::String("barley".into()),   // contains "bar" → KEEP
                Value::String("rinse".into()),    // matches neither → DROP
                Value::String("foobar".into()),   // contains both → KEEP
            ]),
            &[],
        )
        .unwrap();
        assert_eq!(extract_array(r), vec!["food", "barley", "foobar"]);
    }

    #[test]
    fn filters_array_needle_extension_set_practical_example() {
        // Real-world use: keep only image files by extension in
        // a single filter call rather than chaining three
        // `string.filter` invocations.
        let params = json!({ "mode": "endsWith", "needle": [".png", ".jpg", ".webp"] })
            .as_object()
            .unwrap()
            .clone();
        let r = run_with(
            params,
            Value::Array(vec![
                Value::String("a.png".into()),
                Value::String("b.jpg".into()),
                Value::String("c.webp".into()),
                Value::String("d.txt".into()),    // no extension match → DROP
                Value::String("e.bmp".into()),    // no extension match → DROP
            ]),
            &[],
        )
        .unwrap();
        assert_eq!(
            extract_array(r),
            vec!["a.png", "b.jpg", "c.webp"]
        );
    }

    #[test]
    fn filters_array_needle_negative_keeps_slot_missing_at_least_one() {
        // Multi-pattern negative mode: KEEP iff `!matches(slot, p)`
        // for AT LEAST ONE p → the slot must MISS at least one
        // pattern. Equivalently, drops only slots that match
        // EVERY pattern. This is the symmetric existential
        // interpretation of the user's spec — applied across all
        // six modes, not only the positive half — and pinning
        // it here is critical because the more intuitive
        // universal-negative reading ("drop on ANY match") would
        // produce the opposite outcome.
        let params = json!({ "mode": "notContains", "needle": ["draft", "tmp"] })
            .as_object()
            .unwrap()
            .clone();
        let r = run_with(
            params,
            Value::Array(vec![
                Value::String("final.txt".into()),                // misses both → KEEP
                Value::String("draft-1.txt".into()),              // matches "draft" only → KEEP
                Value::String("cache.tmp".into()),                // matches "tmp" only → KEEP
                Value::String("drafts-tmp-archive.txt".into()),   // matches both → DROP
            ]),
            &[],
        )
        .unwrap();
        assert_eq!(
            extract_array(r),
            vec!["final.txt", "draft-1.txt", "cache.tmp"]
        );
    }

    #[test]
    fn empty_array_needle_drops_every_input_slot() {
        // Distinct from the scalar-empty collapse-to-fallback
        // contract: an empty ARRAY (`[]`) wired into the needle
        // port represents "set of zero patterns" and DROPS every
        // input slot (predicate `iter().any()` over `[]` is
        // `false`). Pin so wiring `array.create(count=0)` does
        // NOT silently bypass the param fallback.
        let params = json!({ "mode": "contains", "needle": "fallback-pattern" })
            .as_object()
            .unwrap()
            .clone();
        let empty_array = Value::Array(Vec::new());
        let r = run_with(
            params,
            Value::Array(vec![
                Value::String("has-fallback-pattern".into()),
                Value::String("also-has-fallback-pattern".into()),
            ]),
            &[("needle", &empty_array)],
        )
        .unwrap();
        assert!(r.get("out").unwrap().as_array().unwrap().is_empty());
    }

    #[test]
    fn array_needle_filters_empty_slots_before_matching() {
        // Heterogeneous needle arrays go through the same
        // trim-then-filter-empties pipeline as the `in` slots.
        // A `[foo, "", null, bar]` needle collapses to
        // `[foo, bar]` before the predicate runs — pin so a
        // heterogeneous array with `null` slots doesn't behave
        // more leniently than an explicit empty slot.
        let params = json!({ "mode": "contains", "needle": ["foo", "", "bar"] })
            .as_object()
            .unwrap()
            .clone();
        let r = run_with(
            params,
            Value::Array(vec![
                Value::String("food".into()),     // matches foo → KEEP
                Value::String("barley".into()),   // matches bar → KEEP
                Value::String("rinse".into()),    // matches neither → DROP
            ]),
            &[],
        )
        .unwrap();
        assert_eq!(extract_array(r), vec!["food", "barley"]);
    }

    #[test]
    fn array_needle_with_non_string_slots_coerces_individually() {
        // Element-wise coercion applies to the `needle` array
        // slots too — same `value_to_string` path the `in`
        // array slots take. A mixed needle
        // `[Number(42), String("img")]` produces `[42_digits,
        // "img"]` after to_string + trim + filter; the
        // predicate then runs against each pattern verbatim.
        let params = json!({ "mode": "contains" }).as_object().unwrap().clone();
        let needle = Value::Array(vec![
            Value::from(42),
            Value::String("img".into()),
        ]);
        let r = run_with(
            params,
            Value::Array(vec![
                Value::String("build-42.log".into()), // matches "42" only → KEEP
                Value::String("my-img.png".into()),   // matches "img" only → KEEP
                Value::String("script.js".into()),    // matches neither → DROP
            ]),
            &[("needle", &needle)],
        )
        .unwrap();
        assert_eq!(extract_array(r), vec!["build-42.log", "my-img.png"]);
    }

    // ---------- `Match All` toggle (any-of ↔ all-of) ----------

    #[test]
    fn match_all_true_multi_pattern_positive_requires_every_match() {
        // UNAMBIGUOUS substrings: "id-" and "-42" don't share
        // any character sequence with each other, so the
        // per-slot match evaluation is unambiguous against the
        // substring-trap class I've hit twice (i-m-a vs i-m-g).
        // With `matchAll=true` AND positive `mode`, slot kept iff
        // BOTH patterns match — universal-quantifier semantics,
        // pin against the any-of default below.
        let params = json!({
            "mode": "contains",
            "needle": ["id-", "-42"],
            "matchAll": true,
        })
        .as_object()
        .unwrap()
        .clone();
        let r = run_with(
            params,
            Value::Array(vec![
                Value::String("id-user-42".into()), // contains BOTH → KEEP
                Value::String("id-user-99".into()), // contains "id-" only → DROP
                Value::String("admin-42".into()),   // contains "-42" only → DROP
                Value::String("blank".into()),      // contains neither → DROP
            ]),
            &[],
        )
        .unwrap();
        assert_eq!(extract_array(r), vec!["id-user-42"]);
    }

    #[test]
    fn match_all_true_multi_pattern_negative_requires_every_miss() {
        // UNAMBIGUOUS substrings: "test" / "mock" / "prod" don't
        // overlap. With `matchAll=true` AND `notContains`, slot
        // kept iff EVERY pattern misses — drops only slots that
        // satisfy every pattern (`test-mock-server` matches both,
        // so every miss is false → DROP). Asserts the symmetric
        // interpretation of the user's existential-any spec across
        // BOTH halves of the 6-mode catalog.
        let params = json!({
            "mode": "notContains",
            "needle": ["test", "mock"],
            "matchAll": true,
        })
        .as_object()
        .unwrap()
        .clone();
        let r = run_with(
            params,
            Value::Array(vec![
                Value::String("prod-utils".into()),       // misses BOTH → KEEP
                Value::String("mock-server".into()),      // matches "mock" only → DROP
                Value::String("test-server".into()),      // matches "test" only → DROP
                Value::String("test-mock-server".into()), // matches BOTH → DROP
            ]),
            &[],
        )
        .unwrap();
        assert_eq!(extract_array(r), vec!["prod-utils"]);
    }

    #[test]
    fn match_all_true_with_empty_pattern_set_drops_every_slot_regardless_of_toggle() {
        // Pins the explicit kill switch: a pattern set that
        // resolves to empty after `patterns_from`'s trim+filter
        // pipeline DROPS every input slot REGARDLESS of
        // `matchAll`. Without the kill switch, `all()` over `[]`
        // would return vacuously `true` under `matchAll=true`
        // and silently turn the filter into a pass-through —
        // the OPPOSITE outcome of the `match_all == false`
        // branch. The catalog description explicitly surfaces
        // this so a user toggling `Match All` with an empty
        // upstream `array.create(count=0)` doesn't lose filtering
        // stealth.
        let base_params = json!({
            "mode": "contains",
            "matchAll": true,
        })
        .as_object()
        .unwrap()
        .clone();

        // Case A — directly-empty `[]` needle. `patterns_from`
        // returns an empty `Vec`, no trim+filter needed.
        let empty_array = Value::Array(Vec::new());
        let r = run_with(
            base_params.clone(),
            Value::Array(vec![
                Value::String("alpha".into()),
                Value::String("bravo".into()),
            ]),
            &[("needle", &empty_array)],
        )
        .unwrap();
        assert!(
            r.get("out").unwrap().as_array().unwrap().is_empty(),
            "matchAll=true with directly-empty [] needle should DROP every slot (kill switch)"
        );

        // Case B — heterogeneous all-blank needle:
        // `[null, "", "   \t\n  "]` flows through `patterns_from`
        // and the per-slot `value_to_string` + trim + filter
        // empties pipeline, collapsing to an empty Vec. Exercises
        // the trim+filter path (not just direct empties).
        let blank_array = Value::Array(vec![
            Value::Null,                       // coerces to ""
            Value::String(String::new()),      // explicit ""
            Value::String("   \t\n  ".into()), // whitespace-only trims to ""
        ]);
        let r = run_with(
            base_params,
            Value::Array(vec![
                Value::String("alpha".into()),
                Value::String("bravo".into()),
            ]),
            &[("needle", &blank_array)],
        )
        .unwrap();
        assert!(
            r.get("out").unwrap().as_array().unwrap().is_empty(),
            "matchAll=true with all-blank needle should DROP every slot (trim+filter pipeline kill)"
        );
    }

    #[test]
    fn match_all_true_with_single_pattern_mirror_match_all_false() {
        // Pin: with a SINGLE-element pattern set, `matchAll=true`
        // and `matchAll=false` produce IDENTICAL outputs — a
        // single-element `any()` is observationally equivalent to a
        // single-element `all()` (each collapses to the single
        // match call). The genuine semantics divergence only kicks
        // in at length ≥ 2. Future refactor that always short-
        // circuits to `all()` for length-1 should still pass this;
        // pin the equivalence to catch an accidental divergence in
        // either direction.
        for match_all in [false, true] {
            let params = json!({
                "mode": "contains",
                "needle": ["bar"],
                "matchAll": match_all,
            })
            .as_object()
            .unwrap()
            .clone();
            let r = run_with(
                params,
                Value::Array(vec![
                    Value::String("foo".into()),
                    Value::String("bar".into()),
                    Value::String("baz".into()),
                ]),
                &[],
            )
            .unwrap();
            assert_eq!(
                extract_array(r),
                vec!["bar"],
                "matchAll={match_all} should keep only 'bar' with single-pattern ['bar']"
            );
        }
    }

    #[test]
    fn match_all_default_false_when_param_absent_preserves_back_compat() {
        // Back-compat pin: a blueprint authored BEFORE the
        // `matchAll` toggle landed doesn't include the param in
        // `params`. `params.get("matchAll") → None →
        // unwrap_or(false)` preserves the historical any-of-
        // matches behaviour. Critical because the multi-pattern
        // test surface (six of the existing 27 tests) all depends
        // on this default; missing this would silently flip their
        // semantics on next load.
        let params = json!({
            "mode": "contains",
            "needle": ["foo", "bar"],
            // no "matchAll" key — tests the absent-param branch.
        })
        .as_object()
        .unwrap()
        .clone();
        let r = run_with(
            params,
            Value::Array(vec![
                Value::String("food".into()),   // contains "foo" → KEEP
                Value::String("barley".into()), // contains "bar" → KEEP
                Value::String("rinse".into()),  // matches neither → DROP
            ]),
            &[],
        )
        .unwrap();
        assert_eq!(extract_array(r), vec!["food", "barley"]);
    }

    #[test]
    fn match_all_present_but_non_bool_falls_back_to_false() {
        // Defensive read: an out-of-band hand-edited saved
        // blueprint that wrote `matchAll: "true"` (a JSON string,
        // not a boolean) feeds `as_bool() → None →
        // unwrap_or(false)`. So a user who types the literal
        // text `"true"` in a JSON-editor view of the blueprint
        // does NOT silently flip the predicate to all-of-
        // matches — they get the un-checked any-semantics
        // instead. Same lenient-fallback shape as
        // `image.format::resolve_format`'s `as_str().unwrap_or_else`.
        let params = json!({
            "mode": "contains",
            "needle": ["foo", "bar"],
            "matchAll": "true", // string, NOT bool
        })
        .as_object()
        .unwrap()
        .clone();
        let r = run_with(
            params,
            Value::Array(vec![
                Value::String("food".into()),
                Value::String("barley".into()),
                Value::String("foobar".into()), // contains both → KEEP via any
            ]),
            &[],
        )
        .unwrap();
        assert_eq!(extract_array(r), vec!["food", "barley", "foobar"]);
    }
}
