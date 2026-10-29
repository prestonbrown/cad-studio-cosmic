//! App settings. Key names and enum rawValues match upstream UserDefaults
//! (fixtures/upstream-source/AppSettings.swift); stored as JSON at
//! ~/.config/cad-studio/settings.json (directories lowercases and removes
//! spaces, so the app id must already be "cad-studio").

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::agent::locator::AgentKind;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MeasurementUnit {
    Millimeters,
    Centimeters,
    Meters,
    Inches,
}

// Upstream stores rawValues "millimeters"/"centimeters"/"meters"/"inches"
// and reads the key as string(forKey:) ?? default, so the Deserialize half
// maps unknown strings AND non-string values (null, numbers) to the default
// instead of erroring — same rule for PrinterType and PartsLayout below.
impl Serialize for MeasurementUnit {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(match self {
            MeasurementUnit::Millimeters => "millimeters",
            MeasurementUnit::Centimeters => "centimeters",
            MeasurementUnit::Meters => "meters",
            MeasurementUnit::Inches => "inches",
        })
    }
}

impl<'de> Deserialize<'de> for MeasurementUnit {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        Ok(match serde_json::Value::deserialize(de)? {
            serde_json::Value::String(s) => match s.as_str() {
                "centimeters" => MeasurementUnit::Centimeters,
                "meters" => MeasurementUnit::Meters,
                "inches" => MeasurementUnit::Inches,
                _ => MeasurementUnit::Millimeters,
            },
            _ => MeasurementUnit::Millimeters,
        })
    }
}

impl MeasurementUnit {
    pub fn mm_per_unit(self) -> f64 {
        match self {
            MeasurementUnit::Millimeters => 1.0,
            MeasurementUnit::Centimeters => 10.0,
            MeasurementUnit::Meters => 1000.0,
            MeasurementUnit::Inches => 25.4,
        }
    }

    pub fn fraction_digits(self) -> u32 {
        match self {
            MeasurementUnit::Millimeters => 1,
            MeasurementUnit::Centimeters | MeasurementUnit::Inches => 2,
            MeasurementUnit::Meters => 3,
        }
    }

    pub fn prompt_name(self) -> &'static str {
        match self {
            MeasurementUnit::Millimeters => "millimeters (mm)",
            MeasurementUnit::Centimeters => "centimeters (cm)",
            MeasurementUnit::Meters => "meters (m)",
            MeasurementUnit::Inches => "inches (in)",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum PrinterType {
    #[serde(rename = "fdm")]
    Fdm,
    #[serde(rename = "fdmMultiMaterial")]
    FdmMultiMaterial,
    #[serde(rename = "resin")]
    Resin,
    #[serde(rename = "sls")]
    Sls,
}

// string(forKey:) ?? default: unknown rawValues and non-strings read .fdm.
impl<'de> Deserialize<'de> for PrinterType {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        Ok(match serde_json::Value::deserialize(de)? {
            serde_json::Value::String(s) => match s.as_str() {
                "fdmMultiMaterial" => PrinterType::FdmMultiMaterial,
                "resin" => PrinterType::Resin,
                "sls" => PrinterType::Sls,
                _ => PrinterType::Fdm,
            },
            _ => PrinterType::Fdm,
        })
    }
}

/// Verbatim from fixtures/upstream-source/PrintPreferences.swift:73-107,
/// indexed by [`PrinterType`].
pub const PRINT_GUIDELINES: [&str; 4] = [
    "Single-filament FDM. Unless told otherwise: walls ≥ 1.2 mm, fit clearance 0.2 mm, overhangs ≤ 45°, short bridges, fillets or chamfers on sharp edges where they do not affect function, a flat base for bed adhesion, teardrop or controlled-overhang horizontal holes. Multiple colors need manual filament changes.",
    "Multi-material FDM (AMS, MMU): several colors or materials are possible, one per 3MF part. Unless told otherwise: walls ≥ 1.2 mm, fit clearance 0.2 mm, overhangs ≤ 45°, fillets or chamfers on sharp edges, a flat base for bed adhesion. Keep color changes per layer low when possible.",
    "Resin printing (SLA, DLP, MSLA). Unless told otherwise: walls ≥ 0.8 mm (≥ 1.5 mm for load-bearing parts), fit clearance 0.1-0.15 mm, fine details down to 0.2 mm, hollow parts with drain holes ≥ 3 mm, avoid large flat areas parallel to the plate, plan for a tilted orientation and supports. The build volume is usually small: flag large parts.",
    "SLS powder printing: no supports needed and overhangs are free. Unless told otherwise: walls ≥ 0.8 mm, fit clearance 0.3-0.5 mm, closed cavities with powder escape holes ≥ 5 mm, avoid strongly varying thicknesses that warp. Colors cannot be printed: use them only to tell parts apart.",
];

impl PrinterType {
    pub fn prompt_guidelines(self) -> &'static str {
        PRINT_GUIDELINES[match self {
            PrinterType::Fdm => 0,
            PrinterType::FdmMultiMaterial => 1,
            PrinterType::Resin => 2,
            PrinterType::Sls => 3,
        }]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum PartsLayout {
    #[serde(rename = "assembled")]
    Assembled,
    #[serde(rename = "exploded")]
    Exploded,
    #[serde(rename = "plate")]
    Plate,
}

// string(forKey:) ?? default: unknown rawValues and non-strings read .exploded.
impl<'de> Deserialize<'de> for PartsLayout {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        Ok(match serde_json::Value::deserialize(de)? {
            serde_json::Value::String(s) => match s.as_str() {
                "assembled" => PartsLayout::Assembled,
                "plate" => PartsLayout::Plate,
                _ => PartsLayout::Exploded,
            },
            _ => PartsLayout::Exploded,
        })
    }
}

/// Persisted app preferences. Defaults mirror upstream AppSettings.swift
/// (bools absent from UserDefaults read false; `object ?? true` reads true).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub default_project_location: PathBuf,
    pub agent_engine: String,
    pub claude_path: Option<String>,
    pub codex_path: Option<String>,
    pub claude_model: Option<String>,
    pub codex_model: Option<String>,
    pub claude_effort: Option<String>,
    pub restricted_bash: bool,
    pub streaming_output: bool,
    pub isolates_claude: bool,
    pub measurement_unit: MeasurementUnit,
    pub printer_type: PrinterType,
    pub printer_model: Option<String>,
    pub has_completed_setup: bool,
    /// Upstream stores a JSON-encoded LayoutNode here; kept opaque until the
    /// layout tree exists (M2+).
    pub default_layout: Option<serde_json::Value>,
    pub recent_projects: Vec<String>,
    pub parts_layout: PartsLayout,
    pub parts_collapsed: bool,
    pub parameters_collapsed: bool,
    /// The persisted language override ("en", "it", …; `None` follows the
    /// desktop request). Upstream keeps this in UserDefaults `AppleLanguages`
    /// rather than an AppSettings key; the port's ONE-writer law puts it
    /// here, and it applies at the NEXT launch — the app reads it once at
    /// startup, before language selection, and no live re-render follows.
    pub language: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            default_project_location: documents_dir().unwrap_or_else(|| PathBuf::from("/")),
            agent_engine: "claude".into(),
            claude_path: None,
            codex_path: None,
            claude_model: None,
            codex_model: None,
            claude_effort: None,
            restricted_bash: false,
            streaming_output: true,
            isolates_claude: true,
            measurement_unit: MeasurementUnit::Millimeters,
            printer_type: PrinterType::Fdm,
            printer_model: None,
            has_completed_setup: false,
            default_layout: None,
            recent_projects: Vec::new(),
            parts_layout: PartsLayout::Exploded,
            parts_collapsed: false,
            parameters_collapsed: false,
            language: None,
        }
    }
}

fn documents_dir() -> Option<PathBuf> {
    directories::UserDirs::new()?
        .document_dir()
        .map(Path::to_path_buf)
}

#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

impl Settings {
    /// The selected agent engine (upstream `AgentEngine.current`,
    /// AgentLocator.swift:7-9: unknown strings read claude).
    pub fn agent_kind(&self) -> AgentKind {
        AgentKind::from_raw(&self.agent_engine)
    }

    /// The engine's custom executable path (AgentEngine.pathKey).
    pub fn agent_path(&self, kind: AgentKind) -> Option<&str> {
        match kind {
            AgentKind::Claude => self.claude_path.as_deref(),
            AgentKind::Codex => self.codex_path.as_deref(),
        }
    }

    /// The engine's model override (AgentEngine.modelKey).
    pub fn agent_model(&self, kind: AgentKind) -> Option<&str> {
        match kind {
            AgentKind::Claude => self.claude_model.as_deref(),
            AgentKind::Codex => self.codex_model.as_deref(),
        }
    }

    /// Stage the engine's custom executable path — the writer the
    /// agent-missing sheet's field persists through.
    pub fn set_agent_path(&mut self, kind: AgentKind, path: Option<String>) {
        match kind {
            AgentKind::Claude => self.claude_path = path,
            AgentKind::Codex => self.codex_path = path,
        }
    }

    /// Stage the engine's model override — the settings window's Agente tab
    /// writer (AgentEngine.modelKey's @AppStorage binding).
    pub fn set_agent_model(&mut self, kind: AgentKind, model: Option<String>) {
        match kind {
            AgentKind::Claude => self.claude_model = model,
            AgentKind::Codex => self.codex_model = model,
        }
    }

    pub fn load(path: &Path) -> Result<Self, SettingsError> {
        match std::fs::read_to_string(path) {
            Ok(json) => Ok(serde_json::from_str(&json)?),
            // A missing settings file is a fresh install, not an error.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), SettingsError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)?;
        let tmp = path.with_extension("json.tmp");
        {
            let mut file = std::fs::File::create(&tmp)?;
            std::io::Write::write_all(&mut file, json.as_bytes())?;
            file.sync_all()?;
        }
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}

pub fn settings_path() -> PathBuf {
    directories::ProjectDirs::from("", "", "cad-studio")
        .expect("no home directory: cannot locate settings")
        .config_dir()
        .join("settings.json")
}
