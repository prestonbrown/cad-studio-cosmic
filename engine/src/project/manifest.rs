//! manifest.json decoding. Every field is best-effort (upstream wraps each
//! decode in try? ?? default): malformed values drop the field, never the file.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct BBox {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Drawing {
    pub file: String,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PartRef {
    pub name: String,
    pub quantity: u32,
    pub color: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct Manifest {
    pub name: Option<String>,
    pub bounding_box: Option<BBox>,
    pub warnings: Vec<String>,
    pub parameters: BTreeMap<String, f64>,
    pub drawings: Vec<Drawing>,
    pub bed: Option<[f64; 2]>,
    pub parts: Vec<PartRef>,
}

impl<'de> Deserialize<'de> for Manifest {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let v = serde_json::Value::deserialize(de)?;
        let obj = v.as_object().cloned().unwrap_or_default();
        let strings = |k: &str| -> Vec<String> {
            obj.get(k)
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default()
        };
        let bed = obj.get("bed").and_then(|v| v.as_array()).and_then(|a| {
            let (Some(x), Some(y), None) = (
                a.first().and_then(|v| v.as_f64()),
                a.get(1).and_then(|v| v.as_f64()),
                a.get(2),
            ) else {
                return None;
            };
            (x > 0.0 && y > 0.0).then_some([x, y])
        });
        let parameters = obj
            .get("parameters")
            .and_then(|v| v.as_object())
            .map(|m| {
                m.iter()
                    .filter_map(|(k, v)| v.as_f64().map(|n| (k.clone(), n)))
                    .collect()
            })
            .unwrap_or_default();
        let drawings = obj
            .get("drawings")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|e| {
                        let f = e.get("file")?.as_str()?.to_string();
                        let t = e.get("title")?.as_str()?.to_string();
                        Some(Drawing { file: f, title: t })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let parts = obj
            .get("parts")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|e| {
                        Some(PartRef {
                            name: e.get("name")?.as_str()?.to_string(),
                            quantity: e.get("quantity").and_then(|v| v.as_u64()).unwrap_or(1)
                                as u32,
                            color: e.get("color").and_then(|v| v.as_str()).map(String::from),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let bounding_box = obj.get("bounding_box").and_then(|e| {
            Some(BBox {
                x: e.get("x").and_then(|v| v.as_f64())?,
                y: e.get("y").and_then(|v| v.as_f64())?,
                z: e.get("z").and_then(|v| v.as_f64())?,
            })
        });
        Ok(Manifest {
            name: obj.get("name").and_then(|v| v.as_str()).map(String::from),
            bounding_box,
            warnings: strings("warnings"),
            parameters,
            drawings,
            bed,
            parts,
        })
    }
}

impl Manifest {
    pub fn load(path: &Path) -> Result<Self, serde_json::Error> {
        let text = std::fs::read_to_string(path).map_err(|e| serde_json::Error::io(e))?;
        serde_json::from_str(&text)
    }
}
