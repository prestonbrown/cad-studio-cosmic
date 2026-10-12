//! STL: binary iff the byte length is exactly `84 + 50·n` with `n` the
//! header triangle count, else ASCII `solid … vertex x y z` triplets
//! (contract §3). Single color: STL files carry none.

use super::Part;

pub fn is_binary_stl(file_len: u64, header_tri_count: u32) -> bool {
    file_len == 84 + 50 * header_tri_count as u64
}

#[derive(Debug, thiserror::Error)]
#[error("STL parse: {0}")]
pub struct StlError(pub String);

pub fn parse_stl(bytes: &[u8]) -> Result<Part, StlError> {
    if bytes.len() < 84 {
        return Err(StlError("file shorter than an STL header".into()));
    }
    let n = u32::from_le_bytes([bytes[80], bytes[81], bytes[82], bytes[83]]);
    if is_binary_stl(bytes.len() as u64, n) {
        parse_binary(bytes, n as usize)
    } else {
        parse_ascii(bytes)
    }
}

/// Each 50-byte record is a 12-byte normal, 3 corners of 12 bytes, and a
/// u16 attribute byte count (ignored): corner `c`'s float `k` sits at record
/// offset `12 + 12·c + 4·k`. Normals are dropped — the viewer recomputes
/// them from the corners.
fn parse_binary(bytes: &[u8], n: usize) -> Result<Part, StlError> {
    let need = 84 + 50 * n;
    if bytes.len() < need {
        return Err(StlError(format!("truncated: {}/{} bytes", bytes.len(), need)));
    }
    let mut positions = Vec::with_capacity(n * 3);
    for i in 0..n {
        let off = 84 + i * 50;
        for c in 0..3 {
            let base = off + 12 + c * 12;
            let f = |k: usize| {
                f32::from_le_bytes(bytes[base + k * 4..base + k * 4 + 4].try_into().unwrap())
            };
            positions.push([f(0), f(1), f(2)]);
        }
    }
    Ok(Part {
        name: "model".into(),
        triangle_colors: vec![None; n],
        positions,
    })
}

fn parse_ascii(bytes: &[u8]) -> Result<Part, StlError> {
    let text = std::str::from_utf8(bytes).map_err(|e| StlError(e.to_string()))?;
    let mut positions = Vec::new();
    for line in text.lines() {
        let Some(rest) = line.trim().strip_prefix("vertex") else { continue };
        let mut it = rest.split_whitespace().filter_map(|v| v.parse::<f32>().ok());
        if let (Some(x), Some(y), Some(z)) = (it.next(), it.next(), it.next()) {
            positions.push([x, y, z]);
        } // unparseable line: skipped (upstream behavior)
    }
    if positions.is_empty() || positions.len() % 3 != 0 {
        return Err(StlError(format!("{} vertices do not form triangles", positions.len())));
    }
    Ok(Part {
        name: "model".into(),
        triangle_colors: vec![None; positions.len() / 3],
        positions,
    })
}
