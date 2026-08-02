//! `image.format` — re-encodes one or many images into a different
//! `format` (png / jpeg / webp / gif / bmp). Image **resolution and
//! pixel data are unchanged** — only the byte-level encoding is
//! rewritten. `quality` is honoured for **JPEG only** in the
//! `image` crate 0.25 series (PNG / GIF / BMP are lossless and
//! ignore it; WEBP supports only the lossless encoder in 0.25 —
//! the lossy variant landed in a later release line).
//!
//! Inputs and outputs follow the same scalar-or-array convention as
//! `file.readBinary`: a single binary `in` yields a single binary
//! `out`; an array of N binaries yields an array of N binaries
//! (re-encoded per-element).
//!
//! **JPEG alpha pitfall:** the `image` crate's JPEG encoder panics if
//! given an RGBA image. We coerce via `into_rgb8()` (consuming — no
//! extra allocation) before writing. WEBP / PNG / GIF / BMP keep
//! whatever colour model the source had.

use std::io::Cursor;

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use image::{DynamicImage, ImageEncoder, ImageFormat as ImgFmt};
use serde_json::{Map, Value};

use crate::executor::{
    classify_input, single, EvalCtx, ExecError, InputShape, NodeInputs, NodeOutputs,
};

/// Supported target formats. Strings match the `image` crate's
/// `ImageFormatExt::write_to` extensions for round-tripping.
const SUPPORTED: &[&str] = &["png", "jpeg", "jpg", "webp", "gif", "bmp"];

/// Resolve a human-readable `format` short-string to the `image`
/// crate's enum. `jpg` is normalised to `jpeg`; unknown values error.
fn parse_format(s: &str) -> Result<ImgFmt, ExecError> {
    match s.to_ascii_lowercase().as_str() {
        "png" => Ok(ImgFmt::Png),
        "jpeg" | "jpg" => Ok(ImgFmt::Jpeg),
        "webp" => Ok(ImgFmt::WebP),
        "gif" => Ok(ImgFmt::Gif),
        "bmp" => Ok(ImgFmt::Bmp),
        other => Err(ExecError::Node {
            node: "image.format".into(),
            message: format!(
                "unsupported format '{other}'; supported: {}",
                SUPPORTED.join(", ")
            ),
        }),
    }
}

/// Encode a `DynamicImage` to the target format, returning the bytes.
/// Two of the four branches honour `quality` (JPEG lossy, WEBP lossy);
/// PNG / GIF / BMP are lossless and ignore it.
///
/// **Cursor lifetime:** the cursor MUST outlive the encoder (it owns
/// the borrowed `&mut buf`). We bind it to a `let` rather than
/// chaining `&mut Cursor::new(&mut buf)` inline, otherwise the
/// temporary is dropped before the encoder is used and we get
/// E0716 "temporary value dropped while borrowed".
fn encode(img: DynamicImage, fmt: ImgFmt, quality: u8) -> Result<Vec<u8>, ExecError> {
    match fmt {
        ImgFmt::Jpeg => {
            let rgb8 = img.into_rgb8();
            let mut buf = Vec::new();
            {
                let mut cursor = Cursor::new(&mut buf);
                let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(
                    &mut cursor,
                    quality,
                );
                encoder
                    .write_image(
                        rgb8.as_raw(),
                        rgb8.width(),
                        rgb8.height(),
                        image::ExtendedColorType::Rgb8,
                    )
                    .map_err(|e| ExecError::Node {
                        node: "image.format".into(),
                        message: format!("jpeg encode failed: {e}"),
                    })?;
            }
            Ok(buf)
        }
        ImgFmt::WebP => {
            // `image` 0.25.x only ships `WebPEncoder::new_lossless`.
            // The lossy variant landed in a later release; until we
            // bump, WEBP here is lossless and `quality` is ignored
            // (the catalog description is honest about this — see
            // the module-level doc comment).
            let mut buf = Vec::new();
            {
                let mut cursor = Cursor::new(&mut buf);
                let encoder = image::codecs::webp::WebPEncoder::new_lossless(&mut cursor);
                encoder
                    .write_image(
                        img.as_bytes(),
                        img.width(),
                        img.height(),
                        img.color().into(),
                    )
                    .map_err(|e| ExecError::Node {
                        node: "image.format".into(),
                        message: format!("webp encode failed (lossless): {e}"),
                    })?;
            }
            Ok(buf)
        }
        _ => {
            // PNG / GIF / BMP — quality is meaningless, lossless enums.
            let mut buf = Vec::new();
            {
                let mut cursor = Cursor::new(&mut buf);
                img.write_to(&mut cursor, fmt).map_err(|e| ExecError::Node {
                    node: "image.format".into(),
                    message: format!("encode failed: {e}"),
                })?;
            }
            Ok(buf)
        }
    }
}

/// Decode raw bytes into a `DynamicImage`. Errors are wrapped so the
/// user can tell bad base64 apart from garbage bytes.
fn decode(bytes: &[u8]) -> Result<DynamicImage, ExecError> {
    image::load_from_memory(bytes).map_err(|e| ExecError::Node {
        node: "image.format".into(),
        message: format!("could not decode image (input is not a supported image?): {e}"),
    })
}

/// Resolve the target image format with a strict precedence:
///
///   1. **Wired `format` input port** — accepts any value whose
///      serialised text is a non-empty, ASCII-trimmable string.
///      Non-string shapes (numbers, booleans, null) are coerced
///      via `Value::to_string()`; the resulting text is then sent
///      through `parse_format` so a numeric literal wired into
///      the port surfaces as `unsupported format '42'` instead of
///      silently routing through `image::ImageFormat::Png`.
///      Empty-string wired values collapse to step 2 — a
///      misconfigured `literal.string("")` MUST NOT silently win
///      over an intentional `params.format` choice, because the
///      user could have wired a placeholder literal while
///      relying on the param as the actual driver.
///   2. **Legacy `params.format` entry** — back-compat for
///      blueprints authored before this port existed. Removing
///      this fallback would weightless-migrate every existing
///      blueprint to a default `png` literal; it stays.
///   3. **Default** `"png"` — when neither yields a usable value.
///
/// Both wired and param values are trimmed so a trailing newline
/// from a textarea (`"jpeg\n"`) does not error in `parse_format`.
fn resolve_format(inputs: &NodeInputs<'_>, params: &Map<String, Value>) -> String {
    // 1. Wired format input port.
    if let Some(raw) = inputs.get("format").copied() {
        let s = raw
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| raw.to_string());
        let trimmed = s.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
        // empty wired → fall through to legacy path.
    }
    // 2. Legacy param.
    if let Some(raw) = params.get("format") {
        if let Some(s) = raw.as_str() {
            let trimmed = s.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }
    // 3. Default.
    "png".to_string()
}

pub fn exec(
    inputs: &NodeInputs<'_>,
    params: &Map<String, Value>,
    _ctx: &mut EvalCtx<'_>,
) -> Result<NodeOutputs, ExecError> {
    let format_str = resolve_format(inputs, params);
    let fmt = parse_format(&format_str)?;

    // Quality only matters for JPEG/WEBP; default to 90 for parity
    // with Photoshop / GIMP (NOT the `image` crate's default of 100).
    // Clamped to [1, 100] so an out-of-range param can't crash.
    let quality = params
        .get("quality")
        .and_then(|x| x.as_i64())
        .map(|q| q.clamp(1, 100) as u8)
        .unwrap_or(90);

    let null = Value::Null;
    let provided = inputs.get("in").copied().unwrap_or(&null);

    let convert_one = |b64: &str| -> Result<Value, ExecError> {
        let bytes = B64
            .decode(b64)
            .map_err(|e| ExecError::Node {
                node: "image.format".into(),
                message: format!("invalid base64 payload on `in`: {e}"),
            })?;
        let dyn_img = decode(&bytes)?;
        let out_bytes = encode(dyn_img, fmt, quality)?;
        Ok(Value::String(B64.encode(&out_bytes)))
    };

    match classify_input(provided) {
        InputShape::Empty => Err(ExecError::Node {
            node: "image.format".into(),
            message: "input `in` is empty".into(),
        }),
        InputShape::Single => {
            let s = provided.as_str().ok_or_else(|| ExecError::Node {
                node: "image.format".into(),
                message: "input `in` must be base64-encoded bytes (string)".into(),
            })?;
            let out = convert_one(s)?;
            Ok(single("out", out))
        }
        InputShape::Array(_) => {
            // Length is irrelevant; convert each element. We discard
            // the count here because `arr.len()` is what we want to
            // reserve — keeping the count would compile to the same
            // Vec::with_capacity call.
            let arr = provided.as_array().unwrap();
            let mut out: Vec<Value> = Vec::with_capacity(arr.len());
            for (i, slot) in arr.iter().enumerate() {
                let s = slot.as_str().ok_or_else(|| ExecError::Node {
                    node: "image.format".into(),
                    message: format!("array element #{i} is not a base64-encoded string"),
                })?;
                out.push(convert_one(s)?);
            }
            Ok(single("out", Value::Array(out)))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use image::{ImageBuffer, Rgb};
    use serde_json::{json, Value};

    use super::exec;
    use crate::executor::nodes::test_helpers::ctx_with;
    use crate::executor::{decode_b64 as b64_decode, encode_b64 as b64_encode};

    fn b64(bytes: &[u8]) -> String {
        b64_encode(bytes)
    }

    /// Build a deterministic 2x2 PNG so all tests share the same input
    /// surface. Re-using the bytes keeps the magic-byte sniff single
    /// source of truth.
    fn build_png_bytes() -> Vec<u8> {
        let img = ImageBuffer::from_fn(2, 2, |x, y| {
            let r = (x as u8) * 80;
            let g = (y as u8) * 80;
            let b = 128;
            Rgb([r, g, b])
        });
        let mut buf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut buf, image::ImageFormat::Png)
            .unwrap();
        buf.into_inner()
    }

    /// Build a noisy 32x32 texture so lossy-vs-lossy quality actually
    /// shows in the byte length (a 2x2 grid re-encodes to nearly the
    /// same size regardless of quality, which masks regressions).
    fn build_png_noise() -> Vec<u8> {
        let img = ImageBuffer::from_fn(32, 32, |x, y| {
            let r = ((x * 7 + y * 3) % 256) as u8;
            let g = ((x * 5 + y * 11) % 256) as u8;
            let b = ((x * 13 + y * 17) % 256) as u8;
            Rgb([r, g, b])
        });
        let mut buf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut buf, image::ImageFormat::Png)
            .unwrap();
        buf.into_inner()
    }

    fn run(params: serde_json::Map<String, Value>, input_b64: String) -> HashMap<String, Value> {
        let mut outs = HashMap::new();
        let mut logs = Vec::new();
        let external = HashMap::new();
        let in_val = Value::String(input_b64);
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), &in_val);
        let mut ctx = ctx_with("n1", "image.format", &external, &mut outs, &mut logs);
        exec(&inputs, &params, &mut ctx).unwrap()
    }

    #[test]
    fn converts_png_to_jpeg() {
        let bytes = build_png_bytes();
        let in_b64 = b64(&bytes);

        let params = json!({ "format": "jpeg" }).as_object().unwrap().clone();
        let r = run(params, in_b64);
        let out = r.get("out").unwrap().as_str().unwrap();
        let decoded = b64_decode(out).unwrap();

        // JPEG magic bytes: FF D8 FF
        assert_eq!(&decoded[0..3], &[0xFF, 0xD8, 0xFF]);
        assert!(decoded.len() > 10);
    }

    #[test]
    fn normalises_jpg_alias_to_jpeg() {
        let bytes = build_png_bytes();
        let in_b64 = b64(&bytes);

        let params = json!({ "format": "jpg" }).as_object().unwrap().clone();
        let r = run(params, in_b64);
        let out = r.get("out").unwrap().as_str().unwrap();
        let decoded = b64_decode(out).unwrap();
        assert_eq!(&decoded[0..3], &[0xFF, 0xD8, 0xFF]);
    }

    #[test]
    fn converts_array_input_to_array_output() {
        let bytes = build_png_bytes();
        let in_b64 = b64(&bytes);

        let params = json!({ "format": "png" }).as_object().unwrap().clone();
        let mut outs = HashMap::new();
        let mut logs = Vec::new();
        let external = HashMap::new();
        let in_val = Value::Array(vec![Value::String(in_b64.clone()), Value::String(in_b64.clone())]);
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), &in_val);
        let mut ctx = ctx_with("n1", "image.format", &external, &mut outs, &mut logs);
        let r = exec(&inputs, &params, &mut ctx).unwrap();

        let arr = r.get("out").unwrap().as_array().unwrap();
        assert_eq!(arr.len(), 2);
        // PNG magic header.
        for v in arr {
            let dec = b64_decode(v.as_str().unwrap()).unwrap();
            assert_eq!(&dec[0..4], &[0x89, b'P', b'N', b'G']);
        }
    }

    #[test]
    fn converts_to_webp_round_trips() {
        // Confirms the WEBP encoder path produces a file the loader
        // can re-parse. Without this, a WEBP regression could ship
        // without anybody noticing until a user opens a saved file.
        let bytes = build_png_bytes();
        let in_b64 = b64(&bytes);

        let params = json!({ "format": "webp", "quality": 80 })
            .as_object()
            .unwrap()
            .clone();
        let r = run(params, in_b64);
        let out = r.get("out").unwrap().as_str().unwrap();
        let decoded = b64_decode(out).unwrap();

        // WEBP magic (RIFF container): "RIFF" .... "WEBP".
        assert_eq!(&decoded[0..4], b"RIFF");
        assert_eq!(&decoded[8..12], b"WEBP");

        let parsed = image::load_from_memory(&decoded).expect("WEBP round-trip");
        assert_eq!(parsed.width(), 2);
        assert_eq!(parsed.height(), 2);
    }

    #[test]
    fn quality_materially_changes_jpeg_output() {
        // Regression guard: if someone reverts `encode` to ignore
        // quality for JPEG, the two outputs would be byte-identical.
        // Quality 10 vs 90 on noisy pixels differs by thousands.
        let bytes = build_png_noise();
        let in_b64 = b64(&bytes);

        let low = run(json!({ "format": "jpeg", "quality": 10 }).as_object().unwrap().clone(), in_b64.clone());
        let high = run(json!({ "format": "jpeg", "quality": 90 }).as_object().unwrap().clone(), in_b64);
        let low_bytes = b64_decode(low.get("out").unwrap().as_str().unwrap()).unwrap();
        let high_bytes = b64_decode(high.get("out").unwrap().as_str().unwrap()).unwrap();
        assert!(
            low_bytes.len() != high_bytes.len(),
            "JPEG quality should change encoded size; got same length {} == {}",
            low_bytes.len(),
            high_bytes.len()
        );
    }

    #[test]
    fn errors_on_unknown_format() {
        let bytes = build_png_bytes();
        let params = json!({ "format": "tiff" }).as_object().unwrap().clone();
        let mut outs = HashMap::new();
        let mut logs = Vec::new();
        let external = HashMap::new();
        let in_val = Value::String(b64(&bytes));
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), &in_val);
        let mut ctx = ctx_with("n1", "image.format", &external, &mut outs, &mut logs);
        let err = exec(&inputs, &params, &mut ctx).unwrap_err();
        assert!(matches!(err, crate::executor::ExecError::Node { .. }));
    }

    #[test]
    fn errors_on_garbage_payload() {
        let params = json!({ "format": "png" }).as_object().unwrap().clone();
        let mut outs = HashMap::new();
        let mut logs = Vec::new();
        let external = HashMap::new();
        let in_val = Value::String(b64(b"definitely not an image"));
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), &in_val);
        let mut ctx = ctx_with("n1", "image.format", &external, &mut outs, &mut logs);
        let err = exec(&inputs, &params, &mut ctx).unwrap_err();
        assert!(matches!(err, crate::executor::ExecError::Node { .. }));
    }

    #[test]
    fn wired_format_input_overrides_param() {
        // Regression guard for the precedence contract:
        //   wired `format` input port (when present and non-empty
        //   after trimming) MUST win over the legacy
        //   `params.format` entry.
        //
        // Param says png; wired says jpeg. Output must be JPEG.
        let bytes = build_png_bytes();
        let in_b64 = b64(&bytes);

        let params = json!({ "format": "png" }).as_object().unwrap().clone();
        let mut outs = HashMap::new();
        let mut logs = Vec::new();
        let external = HashMap::new();
        let in_val = Value::String(in_b64);
        let fmt_val = Value::String("jpeg".to_string());
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), &in_val);
        inputs.insert("format".to_string(), &fmt_val);
        let mut ctx = ctx_with("n1", "image.format", &external, &mut outs, &mut logs);
        let r = exec(&inputs, &params, &mut ctx).unwrap();

        let out = r.get("out").unwrap().as_str().unwrap();
        let decoded = b64_decode(out).unwrap();
        // JPEG magic: FF D8 FF
        assert_eq!(&decoded[0..3], &[0xFF, 0xD8, 0xFF]);
    }

    #[test]
    fn empty_wired_format_falls_back_to_param() {
        // Regression guard for the empty-string wired-format
        // collapse: a `literal.string("")` wired to the format
        // port MUST fall back to `params.format` so the user can
        // wire a placeholder literal without overwriting their
        // intentional param choice.
        //
        // Param says jpeg; wired says "". Output must be JPEG
        // (from the param fallback) — proves the wired-empty
        // value did NOT silently short-circuit to the default
        // `"png"`.
        let bytes = build_png_bytes();
        let in_b64 = b64(&bytes);

        let params = json!({ "format": "jpeg" }).as_object().unwrap().clone();
        let mut outs = HashMap::new();
        let mut logs = Vec::new();
        let external = HashMap::new();
        let in_val = Value::String(in_b64);
        let fmt_val = Value::String("".to_string());
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), &in_val);
        inputs.insert("format".to_string(), &fmt_val);
        let mut ctx = ctx_with("n1", "image.format", &external, &mut outs, &mut logs);
        let r = exec(&inputs, &params, &mut ctx).unwrap();

        let out = r.get("out").unwrap().as_str().unwrap();
        let decoded = b64_decode(out).unwrap();
        assert_eq!(&decoded[0..3], &[0xFF, 0xD8, 0xFF]);
    }

    #[test]
    fn whitespace_only_wired_format_falls_back_to_param() {
        // Regression guard for the trim-then-collapse branch:
        // a wired string of pure whitespace (`"  "`, `"\t\n"`,
        // etc.) trims to empty and MUST fall back to
        // `params.format` — same end-state as the empty-string
        // case but reachable via a textarea copy/paste that
        // drags trailing whitespace along.
        let bytes = build_png_bytes();
        let in_b64 = b64(&bytes);

        let params = json!({ "format": "jpeg" }).as_object().unwrap().clone();
        let mut outs = HashMap::new();
        let mut logs = Vec::new();
        let external = HashMap::new();
        let in_val = Value::String(in_b64);
        let fmt_val = Value::String("   \t\n  ".to_string());
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), &in_val);
        inputs.insert("format".to_string(), &fmt_val);
        let mut ctx = ctx_with("n1", "image.format", &external, &mut outs, &mut logs);
        let r = exec(&inputs, &params, &mut ctx).unwrap();

        let out = r.get("out").unwrap().as_str().unwrap();
        let decoded = b64_decode(out).unwrap();
        assert_eq!(&decoded[0..3], &[0xFF, 0xD8, 0xFF]);
    }

    #[test]
    fn wired_non_string_format_errors() {
        // Regression guard for the non-shape-coercion branch:
        // a non-string value wired into the `format` port
        // (typical miswire: `literal.number` whose id was
        // accidentally dragged here, or `literal.boolean` from a
        // toggle fan-out) is coerced via `Value::to_string()` and
        // then routed through `parse_format`, which MUST error
        // with an `ExecError::Node` message that names the
        // offending coerced value — so the runner surfaces
        // `"unsupported format '42'"` instead of silently going
        // to the default `"png"`.
        let bytes = build_png_bytes();
        let in_b64 = b64(&bytes);

        // `json!(42)` round-trips through `from_str` to give
        // us a `Value::Number` — direct construction
        // `Value::Number(42)` does NOT compile because `42` is
        // `i32` and the enum variant expects `serde_json::Number`
        // (the inference is ambiguous across `From<i8>..From<u64>`,
        // and in particular `Value::Number(42_i64)` also wouldn't
        // compile because `i64` is not `Number`). The macro
        // produces `Value::Number(Number::PosInt(42))` whose
        // `Display` formats as `"42"` — exactly what we need to
        // drive `parse_format` into its `other`-branch error
        // message.
        let params = json!({ "format": "png" }).as_object().unwrap().clone();
        let mut outs = HashMap::new();
        let mut logs = Vec::new();
        let external = HashMap::new();
        let in_val = Value::String(in_b64);
        let fmt_val = json!(42);
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), &in_val);
        inputs.insert("format".to_string(), &fmt_val);
        let mut ctx = ctx_with("n1", "image.format", &external, &mut outs, &mut logs);
        let err = exec(&inputs, &params, &mut ctx).unwrap_err();
        match err {
            crate::executor::ExecError::Node { node, message } => {
                assert_eq!(node, "image.format");
                assert!(
                    message.contains("unsupported format '42'"),
                    "expected message to mention unsupported format '42', got: {message}"
                );
            }
            other => panic!("expected ExecError::Node, got {other:?}"),
        }
    }
}
