//! Broadcasting helpers for binary ops over JSON values.
//!
//! Both helpers accept a closure that maps a pair of values (or pair of
//! `f64`s) to a result. The pair shape rules are identical:
//! - scalar vs scalar → call the closure once.
//! - equal-length arrays → call the closure element-wise.
//! - scalar + array → broadcast the scalar to each element of the array.
//!
//! `broadcast_numeric` extracts `f64` from each side and is used by
//! `math.*` executors. `broadcast_pairs` hands the raw `serde_json::Value`
//! pair to the closure and is used by `string.*` and any other Value-level
//! ops.

use serde_json::Value;

use super::{f64_to_value, value_type_name, ExecError};

/// Broadcast a numeric binary op over scalar / equal-length-array inputs.
pub fn broadcast_numeric<F>(
    a: &Value,
    b: &Value,
    op: F,
    node: &str,
) -> Result<Value, ExecError>
where
    F: Fn(f64, f64) -> Result<f64, ExecError>,
{
    match (a, b) {
        (Value::Number(_), Value::Number(_)) => {
            let av = a.as_f64().unwrap();
            let bv = b.as_f64().unwrap();
            Ok(f64_to_value(op(av, bv)?))
        }
        (Value::Array(xs), Value::Array(ys)) if xs.len() == ys.len() => {
            let mut out: Vec<Value> = Vec::with_capacity(xs.len());
            for (xv, yv) in xs.iter().zip(ys) {
                let xn = xv.as_f64().ok_or_else(|| ExecError::TypeMismatch {
                    node: node.into(),
                    expected: "number".into(),
                    actual: value_type_name(xv).into(),
                })?;
                let yn = yv.as_f64().ok_or_else(|| ExecError::TypeMismatch {
                    node: node.into(),
                    expected: "number".into(),
                    actual: value_type_name(yv).into(),
                })?;
                out.push(f64_to_value(op(xn, yn)?));
            }
            Ok(Value::Array(out))
        }
        (Value::Array(xs), y) => {
            let yn = y.as_f64().ok_or_else(|| ExecError::TypeMismatch {
                node: node.into(),
                expected: "number".into(),
                actual: value_type_name(y).into(),
            })?;
            let mut out: Vec<Value> = Vec::with_capacity(xs.len());
            for xv in xs {
                let xn = xv.as_f64().ok_or_else(|| ExecError::TypeMismatch {
                    node: node.into(),
                    expected: "number".into(),
                    actual: value_type_name(xv).into(),
                })?;
                out.push(f64_to_value(op(xn, yn)?));
            }
            Ok(Value::Array(out))
        }
        (x, Value::Array(ys)) => {
            let xn = x.as_f64().ok_or_else(|| ExecError::TypeMismatch {
                node: node.into(),
                expected: "number".into(),
                actual: value_type_name(x).into(),
            })?;
            let mut out: Vec<Value> = Vec::with_capacity(ys.len());
            for yv in ys {
                let yn = yv.as_f64().ok_or_else(|| ExecError::TypeMismatch {
                    node: node.into(),
                    expected: "number".into(),
                    actual: value_type_name(yv).into(),
                })?;
                out.push(f64_to_value(op(xn, yn)?));
            }
            Ok(Value::Array(out))
        }
        (x, y) => Err(ExecError::TypeMismatch {
            node: node.into(),
            expected: "number or equal-length number-array".into(),
            actual: format!("{} vs {}", value_type_name(x), value_type_name(y)),
        }),
    }
}

/// Broadcast a Value-preserving binary op over scalar / equal-length-array inputs.
pub fn broadcast_pairs<F>(
    a: &Value,
    b: &Value,
    op: F,
) -> Result<Value, ExecError>
where
    F: Fn(&Value, &Value) -> Result<Value, ExecError>,
{
    match (a, b) {
        (Value::Array(xs), Value::Array(ys)) if xs.len() == ys.len() => {
            let mut out: Vec<Value> = Vec::with_capacity(xs.len());
            for (xv, yv) in xs.iter().zip(ys) {
                out.push(op(xv, yv)?);
            }
            Ok(Value::Array(out))
        }
        (Value::Array(xs), y) => {
            // LHS is the array: closure sees (lhs_elem, rhs_scalar)
            // every iteration so a positional op like concat composes
            // elem + scalar for each entry.
            let mut out: Vec<Value> = Vec::with_capacity(xs.len());
            for xv in xs {
                out.push(op(xv, y)?);
            }
            Ok(Value::Array(out))
        }
        (x, Value::Array(ys)) => {
            // RHS is the array: closure sees (lhs_scalar, rhs_elem)
            // every iteration so a positional op like concat composes
            // scalar + elem for each entry. Symmetric counterpart of
            // the (Value::Array(xs), y) arm.
            let mut out: Vec<Value> = Vec::with_capacity(ys.len());
            for yv in ys {
                out.push(op(x, yv)?);
            }
            Ok(Value::Array(out))
        }
        _ => op(a, b),
    }
}
