//! The wire format between a [`crate::MathEngine`] and its worker process.
//!
//! On start the worker writes one handshake line, `lawh-math-worker <version>`
//! and `\n`. After that both sides only exchange frames: a little-endian `u32`
//! length, then that many payload bytes. The parent sends one request and
//! waits for its response before sending the next. All numbers are
//! little-endian; `f64`s are their IEEE 754 bits, so layouts survive the trip
//! exactly.
//!
//! ```text
//! request   u32 timeout_ms, then the formula as UTF-8 (the rest of the frame)
//! response  u8 0, layout | u8 1, UTF-8 compile error message | u8 2 (panicked)
//! layout    f64 width, f64 height, f64 baseline, u32 item count, items
//! glyphs    u8 0, u32 font, f64 size, rgba, u32 count, count x (u32 id, f64 x, f64 y)
//! path      u8 1, u8 flags (1 fill, 2 stroke), [rgba fill], [f64 width, rgba stroke],
//!           u32 count, count x element
//! element   u8 0 move | 1 line | 2 quad | 3 cubic | 4 close, then 1, 1, 2, 3 or 0 points
//! point     f64 x, f64 y
//! rgba      4 x u8
//! ```
//!
//! The parent treats everything the worker sends as untrusted: frames are
//! capped at [`MAX_FRAME_BYTES`], counts must fit in the bytes left, numbers
//! must be finite, fonts must be bundled, glyph ids must exist in their font,
//! paths must start with a move, and nothing may follow the payload.

use std::fmt;
use std::io::{self, Read, Write};
use std::sync::Arc;
use std::time::Duration;

use kurbo::{BezPath, PathEl, Point};

use crate::compile::Outcome;
use crate::fonts::glyph_count;
use crate::{MathError, MathGlyph, MathItem, MathLayout};

/// Version of this format. The parent refuses a worker that announces any
/// other.
pub(crate) const PROTOCOL_VERSION: u32 = 1;

/// Largest frame either side accepts, payload only.
pub(crate) const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

/// Longest handshake line the parent reads, newline included.
const MAX_HANDSHAKE_BYTES: usize = 64;

const RESPONSE_LAYOUT: u8 = 0;
const RESPONSE_COMPILE_ERROR: u8 = 1;
const RESPONSE_PANICKED: u8 = 2;

const ITEM_GLYPHS: u8 = 0;
const ITEM_PATH: u8 = 1;

const PATH_FILL: u8 = 1;
const PATH_STROKE: u8 = 2;

const EL_MOVE: u8 = 0;
const EL_LINE: u8 = 1;
const EL_QUAD: u8 = 2;
const EL_CUBIC: u8 = 3;
const EL_CLOSE: u8 = 4;

/// Smallest encoding of one item, glyph, path element and layout header, so a
/// count can be checked against the bytes left before anything is allocated.
const MIN_ITEM_BYTES: usize = 1 + 4 + 8 + 4 + 4;
const GLYPH_BYTES: usize = 4 + 8 + 8;
const MIN_ELEMENT_BYTES: usize = 1;

/// The line the worker announces itself with.
pub(crate) fn handshake() -> Vec<u8> {
    format!("lawh-math-worker {PROTOCOL_VERSION}\n").into_bytes()
}

/// A malformed frame or handshake.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProtocolError(&'static str);

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for ProtocolError {}

impl From<ProtocolError> for io::Error {
    fn from(err: ProtocolError) -> Self {
        io::Error::new(io::ErrorKind::InvalidData, err)
    }
}

/// Writes one frame and flushes.
pub(crate) fn write_frame(out: &mut impl Write, payload: &[u8]) -> io::Result<()> {
    let len = u32::try_from(payload.len())
        .ok()
        .filter(|_| payload.len() <= MAX_FRAME_BYTES)
        .ok_or(ProtocolError("frame too large"))?;
    out.write_all(&len.to_le_bytes())?;
    out.write_all(payload)?;
    out.flush()
}

/// Reads one frame. `Ok(None)` is a clean end of input between frames.
pub(crate) fn read_frame(input: &mut impl Read) -> io::Result<Option<Vec<u8>>> {
    let mut len = [0; 4];
    let mut got = 0;
    while got < len.len() {
        match input.read(&mut len[got..]) {
            Ok(0) if got == 0 => return Ok(None),
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => got += n,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(err) => return Err(err),
        }
    }
    let len = usize::try_from(u32::from_le_bytes(len))
        .ok()
        .filter(|len| *len <= MAX_FRAME_BYTES)
        .ok_or(ProtocolError("frame too large"))?;
    let mut payload = vec![0; len];
    input.read_exact(&mut payload)?;
    Ok(Some(payload))
}

/// Reads the handshake line, newline included, without trusting its length.
pub(crate) fn read_handshake(input: &mut impl Read) -> io::Result<Vec<u8>> {
    let mut line = Vec::new();
    let mut byte = [0];
    while line.len() < MAX_HANDSHAKE_BYTES {
        match input.read(&mut byte) {
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(_) => {
                line.push(byte[0]);
                if byte[0] == b'\n' {
                    return Ok(line);
                }
            }
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(err) => return Err(err),
        }
    }
    Err(ProtocolError("handshake too long").into())
}

/// A compile request, or `None` if the formula can't fit in a frame.
pub(crate) fn encode_request(src: &str, timeout: Duration) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(4 + src.len());
    let millis = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX);
    out.extend_from_slice(&millis.to_le_bytes());
    out.extend_from_slice(src.as_bytes());
    (out.len() <= MAX_FRAME_BYTES).then_some(out)
}

/// The worker's side of [`encode_request`]: the parent's timeout and the
/// formula.
pub(crate) fn decode_request(payload: &[u8]) -> Result<(Duration, &str), ProtocolError> {
    let mut input = Cursor(payload);
    let millis = input.u32()?;
    let src = std::str::from_utf8(input.rest()).map_err(|_| ProtocolError("formula not UTF-8"))?;
    Ok((Duration::from_millis(u64::from(millis)), src))
}

/// The worker's answer to one request. A layout too big for a frame becomes
/// a compile error.
pub(crate) fn encode_response(outcome: &Outcome) -> Vec<u8> {
    let mut out = Vec::new();
    match outcome {
        Ok(layout) => {
            out.push(RESPONSE_LAYOUT);
            if encode_layout(layout, &mut out).is_some() && out.len() <= MAX_FRAME_BYTES {
                return out;
            }
            return encode_response(&Err(MathError::Compile(
                "the formula is too big to draw".to_owned(),
            )));
        }
        Err(MathError::Panicked) => out.push(RESPONSE_PANICKED),
        Err(MathError::Compile(message)) => {
            out.push(RESPONSE_COMPILE_ERROR);
            out.extend_from_slice(message.as_bytes());
        }
        // The worker's compile never produces these; pass them on as text.
        Err(other) => {
            out.push(RESPONSE_COMPILE_ERROR);
            out.extend_from_slice(other.to_string().as_bytes());
        }
    }
    // A compile error message longer than a frame is cut at a char boundary.
    if out.len() > MAX_FRAME_BYTES {
        let mut end = MAX_FRAME_BYTES;
        while end > 1 && !is_char_boundary(&out[1..], end - 1) {
            end -= 1;
        }
        out.truncate(end);
    }
    out
}

fn is_char_boundary(utf8: &[u8], at: usize) -> bool {
    // Continuation bytes look like 0b10xx_xxxx.
    utf8.get(at)
        .is_none_or(|b| (*b & 0b1100_0000) != 0b1000_0000)
}

/// The parent's side of [`encode_response`], checking everything.
pub(crate) fn decode_response(payload: &[u8]) -> Result<Outcome, ProtocolError> {
    let mut input = Cursor(payload);
    let outcome = match input.u8()? {
        RESPONSE_LAYOUT => Ok(Arc::new(decode_layout(&mut input)?)),
        RESPONSE_COMPILE_ERROR => {
            let message = std::str::from_utf8(input.rest())
                .map_err(|_| ProtocolError("error message not UTF-8"))?;
            Err(MathError::Compile(message.to_owned()))
        }
        RESPONSE_PANICKED => Err(MathError::Panicked),
        _ => return Err(ProtocolError("unknown response")),
    };
    input.finish()?;
    Ok(outcome)
}

fn encode_layout(layout: &MathLayout, out: &mut Vec<u8>) -> Option<()> {
    put_f64(out, layout.width);
    put_f64(out, layout.height);
    put_f64(out, layout.baseline);
    put_len(out, layout.items.len())?;
    for item in &layout.items {
        match item {
            MathItem::Glyphs {
                font,
                size,
                fill,
                glyphs,
            } => {
                out.push(ITEM_GLYPHS);
                put_len(out, *font)?;
                put_f64(out, *size);
                out.extend_from_slice(fill);
                put_len(out, glyphs.len())?;
                for glyph in glyphs {
                    out.extend_from_slice(&glyph.id.to_le_bytes());
                    put_f64(out, glyph.x);
                    put_f64(out, glyph.y);
                }
            }
            MathItem::Path { path, fill, stroke } => {
                out.push(ITEM_PATH);
                let mut flags = 0;
                if fill.is_some() {
                    flags |= PATH_FILL;
                }
                if stroke.is_some() {
                    flags |= PATH_STROKE;
                }
                out.push(flags);
                if let Some(fill) = fill {
                    out.extend_from_slice(fill);
                }
                if let Some((width, color)) = stroke {
                    put_f64(out, *width);
                    out.extend_from_slice(color);
                }
                put_len(out, path.elements().len())?;
                for el in path.elements() {
                    encode_element(*el, out);
                }
            }
        }
    }
    Some(())
}

fn encode_element(el: PathEl, out: &mut Vec<u8>) {
    let (tag, points): (u8, &[Point]) = match &el {
        PathEl::MoveTo(p) => (EL_MOVE, std::slice::from_ref(p)),
        PathEl::LineTo(p) => (EL_LINE, std::slice::from_ref(p)),
        PathEl::QuadTo(a, p) => (EL_QUAD, &[*a, *p]),
        PathEl::CurveTo(a, b, p) => (EL_CUBIC, &[*a, *b, *p]),
        PathEl::ClosePath => (EL_CLOSE, &[]),
    };
    out.push(tag);
    for p in points {
        put_f64(out, p.x);
        put_f64(out, p.y);
    }
}

fn decode_layout(input: &mut Cursor<'_>) -> Result<MathLayout, ProtocolError> {
    let width = input.f64()?;
    let height = input.f64()?;
    let baseline = input.f64()?;
    let count = input.count(MIN_ITEM_BYTES)?;
    let mut items = Vec::with_capacity(count);
    for _ in 0..count {
        items.push(match input.u8()? {
            ITEM_GLYPHS => decode_glyphs(input)?,
            ITEM_PATH => decode_path(input)?,
            _ => return Err(ProtocolError("unknown item")),
        });
    }
    Ok(MathLayout {
        width,
        height,
        baseline,
        items,
    })
}

fn decode_glyphs(input: &mut Cursor<'_>) -> Result<MathItem, ProtocolError> {
    let font = usize::try_from(input.u32()?).map_err(|_| ProtocolError("bad font"))?;
    let glyphs_in_font = glyph_count(font).ok_or(ProtocolError("font not bundled"))?;
    let size = input.f64()?;
    let fill = input.rgba()?;
    let count = input.count(GLYPH_BYTES)?;
    let mut glyphs = Vec::with_capacity(count);
    for _ in 0..count {
        let id = input.u32()?;
        if id >= u32::from(glyphs_in_font) {
            return Err(ProtocolError("glyph not in font"));
        }
        glyphs.push(MathGlyph {
            id,
            x: input.f64()?,
            y: input.f64()?,
        });
    }
    Ok(MathItem::Glyphs {
        font,
        size,
        fill,
        glyphs,
    })
}

fn decode_path(input: &mut Cursor<'_>) -> Result<MathItem, ProtocolError> {
    let flags = input.u8()?;
    if flags & !(PATH_FILL | PATH_STROKE) != 0 {
        return Err(ProtocolError("unknown path flags"));
    }
    let fill = if flags & PATH_FILL == 0 {
        None
    } else {
        Some(input.rgba()?)
    };
    let stroke = if flags & PATH_STROKE == 0 {
        None
    } else {
        Some((input.f64()?, input.rgba()?))
    };
    let count = input.count(MIN_ELEMENT_BYTES)?;
    let mut elements = Vec::with_capacity(count);
    for _ in 0..count {
        elements.push(match input.u8()? {
            EL_MOVE => PathEl::MoveTo(input.point()?),
            EL_LINE => PathEl::LineTo(input.point()?),
            EL_QUAD => PathEl::QuadTo(input.point()?, input.point()?),
            EL_CUBIC => PathEl::CurveTo(input.point()?, input.point()?, input.point()?),
            EL_CLOSE => PathEl::ClosePath,
            _ => return Err(ProtocolError("unknown path element")),
        });
    }
    // kurbo requires a path to start with a move.
    if elements
        .first()
        .is_some_and(|el| !matches!(el, PathEl::MoveTo(_)))
    {
        return Err(ProtocolError("path does not start with a move"));
    }
    Ok(MathItem::Path {
        path: BezPath::from_vec(elements),
        fill,
        stroke,
    })
}

fn put_f64(out: &mut Vec<u8>, v: f64) {
    out.extend_from_slice(&v.to_le_bytes());
}

/// A count or index as `u32`; `None` if it doesn't fit.
fn put_len(out: &mut Vec<u8>, n: usize) -> Option<()> {
    out.extend_from_slice(&u32::try_from(n).ok()?.to_le_bytes());
    Some(())
}

/// Reads a payload front to back.
struct Cursor<'a>(&'a [u8]);

impl<'a> Cursor<'a> {
    fn bytes<const N: usize>(&mut self) -> Result<[u8; N], ProtocolError> {
        let (head, tail) = self
            .0
            .split_first_chunk::<N>()
            .ok_or(ProtocolError("frame ends early"))?;
        self.0 = tail;
        Ok(*head)
    }

    fn u8(&mut self) -> Result<u8, ProtocolError> {
        self.bytes::<1>().map(|[b]| b)
    }

    fn u32(&mut self) -> Result<u32, ProtocolError> {
        self.bytes().map(u32::from_le_bytes)
    }

    fn f64(&mut self) -> Result<f64, ProtocolError> {
        let v = f64::from_le_bytes(self.bytes()?);
        if v.is_finite() {
            Ok(v)
        } else {
            Err(ProtocolError("number not finite"))
        }
    }

    fn point(&mut self) -> Result<Point, ProtocolError> {
        Ok(Point::new(self.f64()?, self.f64()?))
    }

    fn rgba(&mut self) -> Result<[u8; 4], ProtocolError> {
        self.bytes()
    }

    /// A count of things at least `min_bytes` long each, checked against the
    /// bytes left so a lying count can't make the parent allocate.
    fn count(&mut self, min_bytes: usize) -> Result<usize, ProtocolError> {
        let count = usize::try_from(self.u32()?).map_err(|_| ProtocolError("bad count"))?;
        if count > self.0.len() / min_bytes {
            return Err(ProtocolError("count larger than the frame"));
        }
        Ok(count)
    }

    fn rest(&mut self) -> &'a [u8] {
        std::mem::take(&mut self.0)
    }

    fn finish(self) -> Result<(), ProtocolError> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(ProtocolError("trailing bytes"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> MathLayout {
        let mut path = BezPath::new();
        path.move_to((0.5, 1.0));
        path.line_to((2.0, 3.0));
        path.quad_to((1.0, 1.0), (4.0, 0.0));
        path.curve_to((1.0, 2.0), (3.0, 4.0), (5.0, 6.0));
        path.close_path();
        MathLayout {
            width: 80.9,
            height: 42.4,
            baseline: 23.8,
            items: vec![
                MathItem::Glyphs {
                    font: 1,
                    size: 20.0,
                    fill: [0, 0, 0, 255],
                    glyphs: vec![
                        MathGlyph {
                            id: 3,
                            x: 0.1,
                            y: 19.4,
                        },
                        MathGlyph {
                            id: 7,
                            x: 10.25,
                            y: -1e-300,
                        },
                    ],
                },
                MathItem::Path {
                    path,
                    fill: Some([1, 2, 3, 4]),
                    stroke: Some((0.8, [5, 6, 7, 8])),
                },
                MathItem::Path {
                    path: BezPath::new(),
                    fill: None,
                    stroke: None,
                },
            ],
        }
    }

    fn layout_frame(layout: &MathLayout) -> Vec<u8> {
        encode_response(&Ok(Arc::new(layout.clone())))
    }

    #[test]
    fn outcomes_round_trip_exactly() {
        let layout = sample();
        assert_eq!(
            decode_response(&layout_frame(&layout)),
            Ok(Ok(Arc::new(layout)))
        );
        for err in [
            MathError::Compile("unknown variable: é".into()),
            MathError::Panicked,
        ] {
            let frame = encode_response(&Err(err.clone()));
            assert_eq!(decode_response(&frame), Ok(Err(err)));
        }
    }

    #[test]
    fn requests_round_trip() {
        let frame = encode_request("A = pi r^2", Duration::from_millis(2000)).unwrap();
        assert_eq!(
            decode_request(&frame),
            Ok((Duration::from_millis(2000), "A = pi r^2"))
        );
        assert!(encode_request(&"x".repeat(MAX_FRAME_BYTES), Duration::ZERO).is_none());
    }

    #[test]
    fn every_truncation_is_rejected() {
        let frame = layout_frame(&sample());
        for len in 0..frame.len() {
            assert!(decode_response(&frame[..len]).is_err(), "cut at {len}");
        }
        let mut long = frame.clone();
        long.push(0);
        assert_eq!(decode_response(&long), Err(ProtocolError("trailing bytes")));
    }

    #[test]
    fn non_finite_numbers_are_rejected() {
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let mut layout = sample();
            layout.width = bad;
            assert_eq!(
                decode_response(&layout_frame(&layout)),
                Err(ProtocolError("number not finite"))
            );
            let mut layout = sample();
            let MathItem::Glyphs { glyphs, .. } = &mut layout.items[0] else {
                unreachable!()
            };
            glyphs[1].y = bad;
            assert!(decode_response(&layout_frame(&layout)).is_err());
        }
    }

    #[test]
    fn fonts_and_glyphs_must_exist() {
        let mut layout = sample();
        let MathItem::Glyphs { font, .. } = &mut layout.items[0] else {
            unreachable!()
        };
        *font = crate::fonts().len();
        assert_eq!(
            decode_response(&layout_frame(&layout)),
            Err(ProtocolError("font not bundled"))
        );

        let mut layout = sample();
        let MathItem::Glyphs { glyphs, .. } = &mut layout.items[0] else {
            unreachable!()
        };
        glyphs[0].id = u32::from(u16::MAX) + 1;
        assert_eq!(
            decode_response(&layout_frame(&layout)),
            Err(ProtocolError("glyph not in font"))
        );
    }

    #[test]
    fn lying_counts_are_rejected_before_allocating() {
        let mut frame = vec![RESPONSE_LAYOUT];
        for v in [1.0, 1.0, 1.0] {
            put_f64(&mut frame, v);
        }
        frame.extend_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(
            decode_response(&frame),
            Err(ProtocolError("count larger than the frame"))
        );
    }

    #[test]
    fn paths_must_start_with_a_move() {
        let mut frame = vec![RESPONSE_LAYOUT];
        for v in [1.0, 1.0, 1.0] {
            put_f64(&mut frame, v);
        }
        frame.extend_from_slice(&1u32.to_le_bytes());
        frame.extend_from_slice(&[ITEM_PATH, 0]);
        frame.extend_from_slice(&1u32.to_le_bytes());
        frame.push(EL_LINE);
        put_f64(&mut frame, 1.0);
        put_f64(&mut frame, 1.0);
        assert_eq!(
            decode_response(&frame),
            Err(ProtocolError("path does not start with a move"))
        );
    }

    #[test]
    fn unknown_tags_are_rejected() {
        assert!(decode_response(&[9]).is_err());
        assert!(decode_response(&[]).is_err());
        assert!(decode_response(&[RESPONSE_COMPILE_ERROR, 0xff]).is_err());
        assert!(decode_response(&[RESPONSE_PANICKED, 0]).is_err());
    }

    #[test]
    fn frames_are_capped_both_ways() {
        let mut wire = Vec::new();
        write_frame(&mut wire, b"hello").unwrap();
        let mut input = wire.as_slice();
        assert_eq!(read_frame(&mut input).unwrap(), Some(b"hello".to_vec()));
        assert_eq!(read_frame(&mut input).unwrap(), None);

        let huge = u32::try_from(MAX_FRAME_BYTES + 1).unwrap().to_le_bytes();
        assert!(read_frame(&mut huge.as_slice()).is_err());
        assert!(read_frame(&mut [1, 0].as_slice()).is_err());
        assert!(read_frame(&mut [5, 0, 0, 0, 1].as_slice()).is_err());
        assert!(write_frame(&mut Vec::new(), &vec![0; MAX_FRAME_BYTES + 1]).is_err());
    }

    #[test]
    fn handshake_is_bounded() {
        let line = handshake();
        assert_eq!(read_handshake(&mut line.as_slice()).unwrap(), line);
        assert!(read_handshake(&mut [b'a'; 100].as_slice()).is_err());
        assert!(read_handshake(&mut b"lawh".as_slice()).is_err());
    }

    #[test]
    fn oversized_errors_are_cut_at_a_char_boundary() {
        let message = "é".repeat(MAX_FRAME_BYTES);
        let frame = encode_response(&Err(MathError::Compile(message)));
        assert!(frame.len() <= MAX_FRAME_BYTES);
        assert!(matches!(
            decode_response(&frame),
            Ok(Err(MathError::Compile(_)))
        ));
    }
}
