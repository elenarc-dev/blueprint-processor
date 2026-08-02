//! `string.replace` — literal find/replace on a single string or on
//! every element of a string array.
//!
//! **Shape mirroring.** Input `in` (port type `string`) may carry
//! either a scalar `Value::String` or a `Value::Array` thereof. The
//! output `out` shape mirrors the input:
//!   * `Value::String` in  → `Value::String` out
//!   * `Value::Array`  in  → `Value::Array` out (same length)
//!   * `Value::Null` / empty array → handled as documented below
//!
//! **Literal-only.** This node uses Rust's `str::replace`, which is
//! literal — no regex, no escape sequences. A regex variant would
//! need the `regex` crate plus a `isRegex` param; we add it later
//! only if a user explicitly asks.
//!
//! **Empty `find` rejection.** Rust's `str::replace` panics on an
//! empty pattern. We reject empty `find` upstream with `ExecError::Node`
//! so the panic never reaches the executor runtime — prefer a clean
//! upstream error to a runtime unwind. The check fires whether
//! `find` comes from the port (`inputs.find`) or from the param
//! (`params.find`), so a wired-but-empty pattern also errors.
//!
//! **Port-vs-param precedence.** Pattern (`find`) and replacement
//! (`replace`) can each be supplied either by an input port or by a
//! typed-in param. The data wiring wins: `inputs.find` overrides
//! `params.find`, and `inputs.replace` overrides `params.replace`.
//! Same precedence rule as `file.delete`. This lets blueprint
//! authors drive the pattern and replacement dynamically from a
//! `Blueprint Input`, `literal.string`, or any other string
//! producer. Non-string port values are coerced via
//! `value_to_string` (the same weak coercion used throughout the
//! String group: `string.concat`, `io.logger`, and the `in` port's
//! scalar non-string fallback here) so a `Number` or `Boolean`
//! routed into `find` is rendered to its canonical JSON form
//! before replacement. `replace` is a SCALAR-only port — wiring an
//! array is a port-type violation (see **Array `find` / scalar
//! `replace`** below).
//!
//! **Element-wise coercion.** Array elements that are not strings
//! go through `value_to_string` (the same weak coercion used by
//! `string.concat` and `io.logger`). Numbers, booleans, and null all
//! render to their canonical JSON-to-string form before replacement.
//! `null` elements become `""`, so a null in an array emits an empty
//! string slot — the array length is preserved, which matters for
//! any downstream `array.length` consumer.
//!
//! **Array `find` / scalar `replace`.** The `find` port accepts
//! either a scalar `Value::String` or a `Value::Array` of strings.
//! A scalar becomes a one-element pattern set; an array supplies the
//! full chain. Patterns run sequentially, so an earlier replacement
//! can introduce characters that a later pattern then matches
//! (pinned by `chained_application_replacement_can_match_a_later_pattern`).
//! The `replace` port is scalar-only (its port type is `string`); a
//! single replacement string BROADCASTS across every pattern in
//! `find_patterns` — the typical `find=[a,b,c], replace="X"` use
//! case rewrites every occurrence of a, b, or c to X. Wiring an
//! array into `replace` is a port-type violation that errors
//! loudly (pinned by `array_replace_value_is_rejected`); per-element
//! replacements belong upstream in `string.concat` or `literal.json`,
//! not on this port.

use serde_json::{Map, Value};

use crate::executor::{single, value_to_string, EvalCtx, ExecError, NodeInputs, NodeOutputs};

pub fn exec(
    inputs: &NodeInputs<'_>,
    params: &Map<String, Value>,
    _ctx: &mut EvalCtx<'_>,
) -> Result<NodeOutputs, ExecError> {
    // Port-vs-param precedence for `find` and `replace`. Pattern from
    // `file_delete.rs`: `inputs.<key>` wins, `params.<key>` is the
    // fallback, and unwired both sides default to `Value::Null`
    // (which `value_to_string` collapses to `""`).
    //
    // `NodeInputs<'a> = HashMap<String, &'a Value>` so `.get(...)`
    // yields `Option<&&Value>`. `Option::cloned()` only unwraps ONE
    // level of reference because `Clone for &T` returns `&T`, so it
    // would give `Option<&Value>` rather than `Option<Value>`. Use
    // map-then-deref instead, mirroring `file_delete::exec`.
    let find_val: Value = inputs
        .get("find")
        .map(|v| (*v).clone())
        .or_else(|| params.get("find").cloned())
        .unwrap_or(Value::Null);
    let replace_val: Value = inputs
        .get("replace")
        .map(|v| (*v).clone())
        .or_else(|| params.get("replace").cloned())
        .unwrap_or(Value::Null);

    // Resolve the find patterns into a `Vec<String>`:
    //   * scalar `Value` (incl. `Value::String`) → 1-element vec
    //   * `Value::Array`                        → one slot per element
    //     (each coerced via `value_to_string`)
    // Empty array, empty slots, and the scalar-empty case all error
    // with a clear message naming the offending shape.
    let find_patterns: Vec<String> = match &find_val {
        Value::Array(items) => {
            if items.is_empty() {
                return Err(ExecError::Node {
                    node: "string.replace".into(),
                    message:
                        "find array is empty — wire a non-empty array or set params.find"
                            .into(),
                });
            }
            let mut v = Vec::with_capacity(items.len());
            for slot in items.iter() {
                let s = value_to_string(slot);
                if s.is_empty() {
                    return Err(ExecError::Node {
                        node: "string.replace".into(),
                        message: format!("find array slot {} is empty", v.len()),
                    });
                }
                v.push(s);
            }
            v
        }
        other => {
            let s = value_to_string(other);
            if s.is_empty() {
                return Err(ExecError::Node {
                    node: "string.replace".into(),
                    message:
                        "find is empty — set params.find or wire the find port with a non-empty value"
                            .into(),
                });
            }
            vec![s]
        }
    };

    // Resolve the single `replace` string. `replace` is a scalar-only
    // port (its type is `string`), so a Value::Array is a port-type
    // violation — error loudly rather than silently taking the first
    // element, so a misconfig (e.g. upstream `string.array` wired
    // here by accident) is visible at runtime instead of silently
    // truncating to one element. The scalar then BROADCASTS across
    // every pattern in `find_patterns`: `find=[a,b,c], replace="X"`
    // rewrites every occurrence of a, b, or c to X.
    let replace_str: String = match &replace_val {
        Value::Array(_) => {
            return Err(ExecError::Node {
                node: "string.replace".into(),                        message: "replace must be a scalar string — for per-element replacements, route them through string.concat or literal.json"
                            .into(),
            });
        }
        other => value_to_string(other),
    };
    // `vec![r; n]` clones `r` n times without allocating a separate
    // buffer; cheap and idiomatic for the broadcast shape.
    let replacements: Vec<String> = vec![replace_str; find_patterns.len()];

    // Apply the chain of (pattern, replacement) pairs in order to a
    // single string. Captured by-reference (`Fn`) so the closure does
    // not consume `find_patterns` / `replacements` and can be reused
    // across the three match arms below.
    //
    // Sequential application caveat: an earlier replacement can
    // introduce characters that a later pattern then matches. With
    // `find=["a","X"], replace="Y"`, input `"aX"` yields `"YY"`
    // because the first pass turns `a` into `Y` (giving `"YX"`) and
    // the second pass turns `X` into `Y` (giving `"YY"`). This is
    // intentional and pinned as a test below — it matches the
    // deterministic pass order of running `str::replace` in a loop.
    let apply = |mut s: String| -> String {
        for (pat, repl) in find_patterns.iter().zip(replacements.iter()) {
            s = s.replace(pat, repl);
        }
        s
    };

    let null = Value::Null;
    let provided = inputs.get("in").copied().unwrap_or(&null);

    match provided {
        Value::String(s) => Ok(single("out", Value::String(apply(s.clone())))),

        Value::Array(items) => {
            // Iterate; coerce each element so a heterogeneous array
            // (e.g. `["foo", 42, true, null]`) still produces a
            // parallel array of strings. Length is preserved 1:1 so
            // downstream `array.length` / `array.getField` keep
            // matching by index.
            let mut out: Vec<Value> = Vec::with_capacity(items.len());
            for slot in items.iter() {
                out.push(Value::String(apply(value_to_string(slot))));
            }
            Ok(single("out", Value::Array(out)))
        }

        // Scalar non-string falls through to toast-to-string then
        // applies the chain. Mirrors string.concat's "weak
        // coercion" rule.
        other => Ok(single("out", Value::String(apply(value_to_string(other))))),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde_json::{json, Value};

    use super::ExecError;
    use super::exec;
    use crate::executor::nodes::test_helpers::ctx_with;

    fn run_with(params: serde_json::Map<String, Value>, in_val: Value) -> HashMap<String, Value> {
        let mut outs = HashMap::new();
        let mut logs = Vec::new();
        let external = HashMap::new();
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), &in_val);
        let mut ctx = ctx_with("n1", "string.replace", &external, &mut outs, &mut logs);
        exec(&inputs, &params, &mut ctx).unwrap()
    }

    fn run_params(find: &str, replace: &str) -> serde_json::Map<String, Value> {
        json!({ "find": find, "replace": replace })
            .as_object()
            .unwrap()
            .clone()
    }

    #[test]
    fn replaces_single_occurrence_in_string() {
        let params = run_params("world", "Rust");
        let r = run_with(params, Value::String("hello world".into()));
        assert_eq!(r.get("out").unwrap().as_str(), Some("hello Rust"));
    }

    #[test]
    fn replaces_all_occurrences_in_string() {
        // str::replace is global (not just first), so abc -> ABC
        // replaces every `abc` not just the leading one. Pinned so
        // a future refactor that swaps to `replacen(_, _, 1)` fails.
        let params = run_params("abc", "X");
        let r = run_with(params, Value::String("abc and abc and abc".into()));
        assert_eq!(r.get("out").unwrap().as_str(), Some("X and X and X"));
    }

    #[test]
    fn returns_unchanged_when_no_match() {
        let params = run_params("zzz", "QQQ");
        let r = run_with(params, Value::String("hello world".into()));
        assert_eq!(r.get("out").unwrap().as_str(), Some("hello world"));
    }

    #[test]
    fn empty_replace_param_effectively_deletes_matches() {
        // `replace=""` deletes every occurrence of the pattern.
        // This is the standard Python/Rust/js literal semantics
        // and is the most common reason for using the node. Pin it.
        let params = run_params("X", "");
        let r = run_with(params, Value::String("aXbXc".into()));
        assert_eq!(r.get("out").unwrap().as_str(), Some("abc"));
    }

    #[test]
    fn replaces_in_array_of_strings() {
        let params = run_params("-", "_");
        let arr = Value::Array(vec![
            Value::String("a-b-c".into()),
            Value::String("x-y-z".into()),
            Value::String("plain".into()),
        ]);
        let r = run_with(params, arr);
        let out = r.get("out").unwrap().as_array().unwrap();
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].as_str(), Some("a_b_c"));
        assert_eq!(out[1].as_str(), Some("x_y_z"));
        assert_eq!(out[2].as_str(), Some("plain")); // unchanged
    }

    #[test]
    fn empty_array_emits_empty_array() {
        // Same shape-mirroring convention as `file.read`: an empty
        // input array passes through cleanly as an empty output
        // array. Pin so a future refactor doesn't "optimise" this
        // into a hard error.
        let params = run_params("a", "b");
        let arr = Value::Array(Vec::<Value>::new());
        let r = run_with(params, arr);
        let out = r.get("out").unwrap().as_array().unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn coerces_non_string_scalar_input_via_value_to_string() {
        // A `Value::Number` on the `in` port goes through the toast
        // helper before replace — same weak coercion rule as
        // string.concat and io.logger. Convenient for pipelines
        // that hop between string and numeric representations.
        let params = run_params("42", "X");
        let r = run_with(params, Value::from(42));
        assert_eq!(r.get("out").unwrap().as_str(), Some("X"));
    }

    #[test]
    fn null_in_array_slot_renders_as_empty_string() {
        // `value_to_string(&Value::Null)` returns "". Replace then
        // operates on the empty string. Length is preserved so
        // downstream array.* nodes keep matching by index.
        let params = run_params("foo", "bar");
        let arr = Value::Array(vec![
            Value::String("foo".into()),
            Value::Null,
            Value::String("foo again".into()),
        ]);
        let r = run_with(params, arr);
        let out = r.get("out").unwrap().as_array().unwrap();
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].as_str(), Some("bar"));
        assert_eq!(out[1].as_str(), Some(""));
        assert_eq!(out[2].as_str(), Some("bar again"));
    }

    #[test]
    fn literality_no_regex_interpretation() {
        // `.` is a regex meta-character; under literal semantics it
        // matches only the literal period. ASCII-only string for
        // portable comparison.
        let params = run_params("a.b", "X");
        let r = run_with(params, Value::String("a.b and aXb".into()));
        // `a.b` matches `a.b` literally (yes replacement to X);
        // `aXb` (the second chunk) contains only `aXb` so neither
        // pattern nor its substitute is plainly visible.
        assert_eq!(r.get("out").unwrap().as_str(), Some("X and aXb"));
    }

    #[test]
    fn errors_on_empty_find_param() {
        let params = run_params("", "X");
        let mut outs = HashMap::new();
        let mut logs = Vec::new();
        let external = HashMap::new();
        let in_val = Value::String("hello".into());
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), &in_val);
        let mut ctx = ctx_with("n1", "string.replace", &external, &mut outs, &mut logs);
        let err = exec(&inputs, &params, &mut ctx).unwrap_err();
        assert!(matches!(err, ExecError::Node { ref message, .. } if message.contains("find")));
    }

    #[test]
    fn empty_or_null_input_with_non_empty_find_passes_through() {
        // `Value::Null` falls through to the toast path:
        // `value_to_string(&Null) == ""`, then `""`.replace("a","b")
        // returns `""`. Same for an empty string input.
        let params = run_params("a", "b");
        let r1 = run_with(params.clone(), Value::Null);
        assert_eq!(r1.get("out").unwrap().as_str(), Some(""));
        let r2 = run_with(params, Value::String(String::new()));
        assert_eq!(r2.get("out").unwrap().as_str(), Some(""));
    }

    // ---------- port-vs-param precedence (wired inputs override params) ----------

    /// Wire a set of `(port_name, &Value)` pairs alongside the
    /// always-present `in` slot, then run `exec` and return the raw
    /// `Result`. Tests unwrap on the happy path (`.unwrap()`) and
    /// `.unwrap_err()` for empty-`find` cases — keeping both shapes
    /// in one helper instead of two. The existing `run_with` helper
    /// only wires `in`; this one is the `extra`-ports variant.
    fn run_wired(
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
        let mut ctx = ctx_with("n1", "string.replace", &external, &mut outs, &mut logs);
        exec(&inputs, &params, &mut ctx)
    }

    #[test]
    fn connected_find_port_overrides_param() {
        // params.find is set to a pattern that would NEVER match the
        // input, so if the param path were taken the output would be
        // the input verbatim. Wiring a matching pattern through the
        // `find` port proves the port wins.
        let params = json!({ "find": "ZZZ_NEVER_MATCHES", "replace": "Rust" })
            .as_object()
            .unwrap()
            .clone();
        let find_port = Value::String("world".into());
        let r = run_wired(
            params,
            Value::String("hello world".into()),
            &[("find", &find_port)],
        )
        .unwrap();
        assert_eq!(r.get("out").unwrap().as_str(), Some("hello Rust"));
    }

    #[test]
    fn connected_replace_port_overrides_param() {
        // Inverse: params.find matches the input but params.replace
        // would yield a clearly wrong output. Wiring the correct
        // replacement through `replace` proves THAT port wins.
        let params = json!({ "find": "world", "replace": "WRONG_REPL" })
            .as_object()
            .unwrap()
            .clone();
        let replace_port = Value::String("Rust".into());
        let r = run_wired(
            params,
            Value::String("hello world".into()),
            &[("replace", &replace_port)],
        )
        .unwrap();
        assert_eq!(r.get("out").unwrap().as_str(), Some("hello Rust"));
    }

    #[test]
    fn both_connected_ports_override_both_params() {
        // Both params set to wrong values; both ports provide correct
        // ones. The output should come entirely from the ports, not
        // from either param.
        let params = json!({ "find": "WRONG_FIND", "replace": "WRONG_REPL" })
            .as_object()
            .unwrap()
            .clone();
        let find_port = Value::String("world".into());
        let replace_port = Value::String("Rust".into());
        let r = run_wired(
            params,
            Value::String("hello world".into()),
            &[("find", &find_port), ("replace", &replace_port)],
        )
        .unwrap();
        assert_eq!(r.get("out").unwrap().as_str(), Some("hello Rust"));
    }

    #[test]
    fn non_string_find_port_is_coerced_to_text() {
        // A `Number` wired into `find` is coerced to "42" before
        // being used as the search pattern. `params.find` is set to
        // a string that would NOT match, so the only way the
        // pattern becomes "42" is through the text-coercion path on
        // the port — pinning it.
        let params = json!({ "find": "ZZZ_NEVER_MATCHES", "replace": "X" })
            .as_object()
            .unwrap()
            .clone();
        let find_port = Value::from(42);
        let r = run_wired(
            params,
            Value::String("abc 42 def 42".into()),
            &[("find", &find_port)],
        )
        .unwrap();
        assert_eq!(r.get("out").unwrap().as_str(), Some("abc X def X"));
    }

    #[test]
    fn non_string_replace_port_is_coerced_to_text() {
        // A `Boolean(true)` wired into `replace` is coerced to
        // "true" before being written where the find-pattern
        // matches. `params.replace` is set to a string that would
        // yield output "aZZZb" if taken — the actual output proves
        // the port wins AND the coercion path.
        let params = json!({ "find": "X", "replace": "ZZZ" })
            .as_object()
            .unwrap()
            .clone();
        let replace_port = Value::Bool(true);
        let r = run_wired(
            params,
            Value::String("aXb".into()),
            &[("replace", &replace_port)],
        )
        .unwrap();
        assert_eq!(r.get("out").unwrap().as_str(), Some("atrueb"));
    }

    #[test]
    fn null_find_port_errors_as_empty_find() {
        // A wired `Value::Null` collapses to `""` via the same
        // coercion path as Number/Bool, which then fails the
        // empty-find check. The error message must still mention
        // `find` so the user can locate the failure without reading
        // docs.
        let params = json!({ "find": "XXX", "replace": "Y" })
            .as_object()
            .unwrap()
            .clone();
        let find_port = Value::Null;
        let err = run_wired(
            params,
            Value::String("hello".into()),
            &[("find", &find_port)],
        )
        .unwrap_err();
        assert!(
            matches!(err, ExecError::Node { ref message, .. }
                if message.contains("find is empty")),
            "expected empty-find message, got: {err:?}"
        );
    }

    #[test]
    fn null_replace_port_renders_as_empty_replace() {
        // A wired `Value::Null` on `replace` collapses to `""`, which
        // `str::replace` interprets as "delete every match" — the
        // standard literal-replace semantics. Pinning this so a
        // future refactor that errors on null-coerced replace can't
        // slip in. Mirrors `null_in_array_slot_renders_as_empty_string`
        // but for the replacement slot specifically.
        let params = json!({ "find": "X", "replace": "WRONG_REPL" })
            .as_object()
            .unwrap()
            .clone();
        let replace_port = Value::Null;
        let r = run_wired(
            params,
            Value::String("aXb".into()),
            &[("replace", &replace_port)],
        )
        .unwrap();
        assert_eq!(r.get("out").unwrap().as_str(), Some("ab"));
    }

    #[test]
    fn param_still_used_when_neither_port_is_connected() {
        // Backwards-compat: when NEITHER `find` nor `replace` is
        // wired (only the param values are present), behaviour is
        // identical to the pre-port-vs-param implementation. This
        // pins every existing stored blueprint that has not been
        // edited to add wires.
        let params = json!({ "find": "world", "replace": "Rust" })
            .as_object()
            .unwrap()
            .clone();
        let r = run_wired(params, Value::String("hello world".into()), &[]).unwrap();
        assert_eq!(r.get("out").unwrap().as_str(), Some("hello Rust"));
    }

    // ---------- multi-pattern `find` (the user-requested feature) ----------

    #[test]
    fn array_find_with_scalar_replace_broadcasts_to_every_pattern() {
        // The user's primary use case: pass a list of patterns and a
        // single replacement; every pattern gets the same replacement.
        // Trace: "abc a b c abc" → pass a→X "Xbc X b c Xbc" → pass
        // b→X "XXc X X c XXc" → pass c→X "XXX X X X XXX".
        let params = json!({ "find": ["a", "b", "c"], "replace": "X" })
            .as_object()
            .unwrap()
            .clone();
        let r = run_wired(
            params,
            Value::String("abc a b c abc".into()),
            &[],
        )
        .unwrap();
        assert_eq!(r.get("out").unwrap().as_str(), Some("XXX X X X XXX"));
    }

    #[test]
    fn array_replace_value_is_rejected() {
        // `replace` is a scalar-only port. Wiring an array into it
        // is a port-type violation: the executor rejects loudly so
        // a misconfig (e.g. accidentally routing a string-array from
        // upstream through the `replace` slot) is visible at runtime
        // instead of silently coercing it. Pin so a future refactor
        // that "fixes" this by silently taking the first element
        // can't slip in without breaking this test. We only wire
        // `replace` here — the array-on-replace rejection fires before
        // `find_patterns` is resolved, so leaving `find` on its params
        // path keeps the test focused on the replace-shape contract.
        let replace_port = Value::Array(vec![Value::String("X".into()), Value::String("Y".into())]);
        let err = run_wired(
            json!({ "find": "anything" }).as_object().unwrap().clone(),
            Value::String("hello".into()),
            &[("replace", &replace_port)],
        )
        .unwrap_err();
        assert!(
            matches!(err, ExecError::Node { ref message, .. }
                if message.contains("replace must be a scalar string")),
            "expected scalar-replace violation error, got: {err:?}"
        );
    }

    #[test]
    fn empty_find_array_errors() {
        // `Value::Array(vec![])` — no patterns → distinct error
        // from the scalar-empty-find case so the user knows it's
        // the array shape that's empty, not a wired null.
        let params = json!({ "find": [], "replace": "X" })
            .as_object()
            .unwrap()
            .clone();
        let err = run_wired(
            params,
            Value::String("hello".into()),
            &[],
        )
        .unwrap_err();
        assert!(
            matches!(err, ExecError::Node { ref message, .. }
                if message.contains("find array is empty")),
            "expected find-array-is-empty error, got: {err:?}"
        );
    }

    #[test]
    fn single_element_array_find_behaves_like_scalar() {
        // `find=["a"]`, `replace="X"`, `in="aaa"` → "XXX". Pin so
        // the wrap-in-Vec path can't silently regress for the
        // single-element case (e.g. by skipping the loop entirely
        // because find_patterns.len() == 1).
        let params = json!({ "find": ["a"], "replace": "X" })
            .as_object()
            .unwrap()
            .clone();
        let r = run_wired(
            params,
            Value::String("aaa".into()),
            &[],
        )
        .unwrap();
        assert_eq!(r.get("out").unwrap().as_str(), Some("XXX"));
    }

    #[test]
    fn array_find_with_input_array_applies_per_element() {
        // shape-mirroring on `in` coexists with array find/replace.
        // Each input element gets the full pattern chain applied.
        // Output is a parallel array of equal length.
        let params = json!({ "find": ["a", "b"], "replace": "X" })
            .as_object()
            .unwrap()
            .clone();
        let arr_in = Value::Array(vec![
            Value::String("aba".into()),
            Value::String("bab".into()),
            Value::String("cab".into()),
        ]);
        let r = run_wired(params, arr_in, &[]).unwrap();
        let out = r.get("out").unwrap().as_array().unwrap();
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].as_str(), Some("XXX"));
        assert_eq!(out[1].as_str(), Some("XXX"));
        assert_eq!(out[2].as_str(), Some("cXX"));
    }

    #[test]
    fn array_find_on_port_broadcasts_to_param_replace() {
        // Split-source combo: `find` wired through the port as an
        // array, but `replace` comes from `params.replace` as a
        // scalar. Pins that broadcast works independently of where
        // each side's source lives — pin so a future refactor that
        // ties broadcast to "both port-sourced" doesn't slip in.
        let params = json!({ "find": "WRONG_FIND_PARAM", "replace": "X" })
            .as_object()
            .unwrap()
            .clone();
        let find_port = Value::Array(vec![Value::String("a".into()), Value::String("b".into())]);
        let r = run_wired(
            params,
            Value::String("a b a b".into()),
            &[("find", &find_port)],
        )
        .unwrap();
        assert_eq!(r.get("out").unwrap().as_str(), Some("X X X X"));
    }

    #[test]
    fn chained_application_replacement_can_match_a_later_pattern() {
        // Documented sequential-application edge case. With
        // `find=["a","X"]`, `replace="Y"`, `in="aX"`:
        //   pass 1: a→Y → "YX"
        //   pass 2: X→Y → "YY"
        // End result is "YY". Pin so a future refactor that changes
        // the chained-replacement contract (e.g. via a regex-set
        // single-pass approach) fails this test deliberately when
        // we eventually decide to switch.
        let params = json!({ "find": ["a", "X"], "replace": "Y" })
            .as_object()
            .unwrap()
            .clone();
        let r = run_wired(
            params,
            Value::String("aX".into()),
            &[],
        )
        .unwrap();
        assert_eq!(r.get("out").unwrap().as_str(), Some("YY"));
    }
}
