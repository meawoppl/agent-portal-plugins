//! Configuration file support (port of `kicad_tools.config`).
//!
//! Hierarchical TOML configuration:
//! 1. project config: `.kicad-tools.toml` or `kicad-tools.toml`, found by
//!    walking up from the start directory (stopping at a `.git` directory or
//!    the filesystem root);
//! 2. user config: `~/.config/kicad-tools/config.toml`.
//!
//! Project values override user values, which override defaults; CLI flags
//! override all. Unknown keys produce warnings (collected on
//! [`Config::warnings`] and printed to stderr by [`Config::load`]).

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

/// Config file names searched for in project directories, in priority order.
pub const CONFIG_FILENAMES: [&str; 2] = [".kicad-tools.toml", "kicad-tools.toml"];

/// Known keys per section (for unknown-key warnings).
pub const KNOWN_KEYS: &[(&str, &[&str])] = &[
    ("defaults", &["format", "manufacturer", "verbose", "quiet"]),
    ("display", &["units", "precision_mm", "precision_mils"]),
    ("drc", &["strict", "layers", "filters"]),
    ("erc", &["filters"]),
    ("export", &["output_dir", "include_dnp"]),
    (
        "route",
        &[
            "strategy",
            "grid_resolution",
            "trace_width",
            "clearance",
            "via_drill",
            "via_diameter",
            "cache_enabled",
            "cache_dir",
            "cache_max_size_mb",
            "cache_ttl_days",
        ],
    ),
    ("parts", &["cache_dir", "cache_ttl_days"]),
    (
        "footprint_validation",
        &["kicad_library_path", "tolerance_mm", "library_mappings"],
    ),
    (
        "footprint_selection",
        &["profile", "capacitor", "resistor", "inductor"],
    ),
];

fn known_section(section: &str) -> Option<&'static [&'static str]> {
    KNOWN_KEYS
        .iter()
        .find(|(s, _)| *s == section)
        .map(|(_, keys)| *keys)
}

/// `~/.config/kicad-tools/config.toml` (`USER_CONFIG_PATH`).
pub fn user_config_path() -> PathBuf {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("~"));
    home.join(".config").join("kicad-tools").join("config.toml")
}

/// Default options for CLI commands.
#[derive(Debug, Clone, PartialEq)]
pub struct DefaultsConfig {
    pub format: String,
    pub manufacturer: Option<String>,
    pub verbose: bool,
    pub quiet: bool,
}

impl Default for DefaultsConfig {
    fn default() -> Self {
        Self {
            format: "table".into(),
            manufacturer: None,
            verbose: false,
            quiet: false,
        }
    }
}

/// Display configuration for CLI output.
#[derive(Debug, Clone, PartialEq)]
pub struct DisplayConfig {
    /// `"mm"` or `"mils"`.
    pub units: String,
    pub precision_mm: i64,
    pub precision_mils: i64,
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            units: "mm".into(),
            precision_mm: 3,
            precision_mils: 1,
        }
    }
}

/// DRC-specific configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct DrcConfig {
    pub strict: bool,
    pub layers: i64,
}

impl Default for DrcConfig {
    fn default() -> Self {
        Self {
            strict: false,
            layers: 2,
        }
    }
}

/// Export-specific configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct ExportConfig {
    pub output_dir: String,
    pub include_dnp: bool,
}

impl Default for ExportConfig {
    fn default() -> Self {
        Self {
            output_dir: "./manufacturing".into(),
            include_dnp: false,
        }
    }
}

/// Routing configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct RouteConfig {
    pub strategy: String,
    /// `"adaptive"` or `"uniform"`.
    pub grid_strategy: String,
    pub grid_resolution: f64,
    pub trace_width: f64,
    pub clearance: f64,
    pub via_drill: f64,
    pub via_diameter: f64,
    pub cache_enabled: bool,
    pub cache_dir: String,
    pub cache_max_size_mb: i64,
    pub cache_ttl_days: i64,
}

impl Default for RouteConfig {
    fn default() -> Self {
        Self {
            strategy: "negotiated".into(),
            grid_strategy: "adaptive".into(),
            grid_resolution: 0.1,
            trace_width: 0.2,
            clearance: 0.2,
            via_drill: 0.3,
            via_diameter: 0.6,
            cache_enabled: true,
            cache_dir: "~/.cache/kicad-tools/routing".into(),
            cache_max_size_mb: 500,
            cache_ttl_days: 30,
        }
    }
}

/// Parts lookup configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct PartsConfig {
    pub cache_dir: String,
    pub cache_ttl_days: i64,
}

impl Default for PartsConfig {
    fn default() -> Self {
        Self {
            cache_dir: "~/.cache/kicad-tools/lcsc".into(),
            cache_ttl_days: 7,
        }
    }
}

/// Footprint validation configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct FootprintValidationConfig {
    pub kicad_library_path: Option<String>,
    pub tolerance_mm: f64,
    /// Footprint name -> library directory, in file order.
    pub library_mappings: Vec<(String, String)>,
}

impl Default for FootprintValidationConfig {
    fn default() -> Self {
        Self {
            kicad_library_path: None,
            tolerance_mm: 0.05,
            library_mappings: Vec::new(),
        }
    }
}

/// Footprint selection configuration for passives. Rule maps are
/// `value_range -> "Library:Footprint"` in file order.
#[derive(Debug, Clone, PartialEq)]
pub struct FootprintSelectionConfig {
    /// `default`, `machine`, `hand_solder`, `compact`.
    pub profile: String,
    pub capacitor: Vec<(String, String)>,
    pub resistor: Vec<(String, String)>,
    pub inductor: Vec<(String, String)>,
}

impl Default for FootprintSelectionConfig {
    fn default() -> Self {
        Self {
            profile: "default".into(),
            capacitor: Vec::new(),
            resistor: Vec::new(),
            inductor: Vec::new(),
        }
    }
}

/// Merged configuration from all sources.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Config {
    pub defaults: DefaultsConfig,
    pub display: DisplayConfig,
    pub drc: DrcConfig,
    pub export: ExportConfig,
    pub route: RouteConfig,
    pub parts: PartsConfig,
    pub footprint_validation: FootprintValidationConfig,
    pub footprint_selection: FootprintSelectionConfig,
    /// `section.key` -> source file (for `--show`).
    pub sources: BTreeMap<String, String>,
    /// Warnings raised while merging (unknown keys, wrong value types).
    pub warnings: Vec<String>,
}

/// Configuration-related errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError(pub String);

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ConfigError {}

impl Config {
    /// Load with precedence project > user > defaults, starting the project
    /// search at `start_dir` (default: current directory). Warnings are
    /// printed to stderr.
    pub fn load(start_dir: Option<&Path>) -> Result<Config, ConfigError> {
        let config = Self::load_with_user_config(start_dir, &user_config_path())?;
        for w in &config.warnings {
            eprintln!("Warning: {w}");
        }
        Ok(config)
    }

    /// [`Config::load`] with an explicit user-config path (upstream tests
    /// monkeypatch `USER_CONFIG_PATH`); warnings are only collected.
    pub fn load_with_user_config(
        start_dir: Option<&Path>,
        user_config: &Path,
    ) -> Result<Config, ConfigError> {
        let cwd;
        let start = match start_dir {
            Some(d) => d,
            None => {
                cwd = std::env::current_dir().map_err(|e| ConfigError(e.to_string()))?;
                &cwd
            }
        };
        let mut config = Config::default();
        if user_config.exists() {
            let data = load_toml_file(user_config)?;
            config.merge(&data, &user_config.display().to_string());
        }
        if let Some(project) = find_project_config(start) {
            let data = load_toml_file(&project)?;
            config.merge(&data, &project.display().to_string());
        }
        Ok(config)
    }

    /// Source file for `section.key`, or `"default"`.
    pub fn get_source(&self, key: &str) -> &str {
        self.sources
            .get(key)
            .map(String::as_str)
            .unwrap_or("default")
    }

    fn merge(&mut self, data: &toml::Table, source: &str) {
        for key in data.keys() {
            if known_section(key).is_none() {
                self.warnings
                    .push(format!("Unknown config key '{key}' in {source}"));
            }
        }
        let mut m = Merger {
            data,
            source,
            sources: &mut self.sources,
            warnings: &mut self.warnings,
            section: "",
        };
        if m.section("defaults") {
            m.string("format", &mut self.defaults.format);
            m.opt_string("manufacturer", &mut self.defaults.manufacturer);
            m.boolean("verbose", &mut self.defaults.verbose);
            m.boolean("quiet", &mut self.defaults.quiet);
        }
        if m.section("display") {
            m.string("units", &mut self.display.units);
            m.int("precision_mm", &mut self.display.precision_mm);
            m.int("precision_mils", &mut self.display.precision_mils);
        }
        if m.section("drc") {
            m.boolean("strict", &mut self.drc.strict);
            m.int("layers", &mut self.drc.layers);
        }
        if m.section("export") {
            m.string("output_dir", &mut self.export.output_dir);
            m.boolean("include_dnp", &mut self.export.include_dnp);
        }
        if m.section("route") {
            m.string("strategy", &mut self.route.strategy);
            m.float("grid_resolution", &mut self.route.grid_resolution);
            m.float("trace_width", &mut self.route.trace_width);
            m.float("clearance", &mut self.route.clearance);
            m.float("via_drill", &mut self.route.via_drill);
            m.float("via_diameter", &mut self.route.via_diameter);
        }
        if m.section("parts") {
            m.string("cache_dir", &mut self.parts.cache_dir);
            m.int("cache_ttl_days", &mut self.parts.cache_ttl_days);
        }
        if m.section("footprint_validation") {
            let fpv = &mut self.footprint_validation;
            m.opt_string("kicad_library_path", &mut fpv.kicad_library_path);
            m.float("tolerance_mm", &mut fpv.tolerance_mm);
            m.map("library_mappings", &mut fpv.library_mappings);
        }
        if m.section("footprint_selection") {
            let fps = &mut self.footprint_selection;
            m.string("profile", &mut fps.profile);
            m.map("capacitor", &mut fps.capacitor);
            m.map("resistor", &mut fps.resistor);
            m.map("inductor", &mut fps.inductor);
        }
    }
}

struct Merger<'a> {
    data: &'a toml::Table,
    source: &'a str,
    sources: &'a mut BTreeMap<String, String>,
    warnings: &'a mut Vec<String>,
    section: &'static str,
}

impl Merger<'_> {
    /// Select a section; warns about its unknown keys. False if absent.
    fn section(&mut self, name: &'static str) -> bool {
        self.section = name;
        let Some(value) = self.data.get(name) else {
            return false;
        };
        let Some(table) = value.as_table() else {
            self.warnings.push(format!(
                "Invalid config section '{name}' in {}: expected a table",
                self.source
            ));
            return false;
        };
        let known = known_section(name).unwrap_or(&[]);
        for key in table.keys() {
            if !known.contains(&key.as_str()) {
                self.warnings.push(format!(
                    "Unknown config key '{name}.{key}' in {}",
                    self.source
                ));
            }
        }
        true
    }

    fn get(&mut self, key: &str) -> Option<&toml::Value> {
        self.data.get(self.section)?.as_table()?.get(key)
    }

    fn set<T>(&mut self, key: &str, slot: &mut T, parsed: Option<T>, expected: &str) {
        match parsed {
            Some(v) => {
                *slot = v;
                self.sources
                    .insert(format!("{}.{key}", self.section), self.source.to_string());
            }
            None => self.warnings.push(format!(
                "Invalid value for config key '{}.{key}' in {}: expected {expected}",
                self.section, self.source
            )),
        }
    }

    fn string(&mut self, key: &str, slot: &mut String) {
        if let Some(v) = self.get(key).cloned() {
            self.set(key, slot, v.as_str().map(str::to_string), "a string");
        }
    }

    fn opt_string(&mut self, key: &str, slot: &mut Option<String>) {
        if let Some(v) = self.get(key).cloned() {
            self.set(
                key,
                slot,
                v.as_str().map(|s| Some(s.to_string())),
                "a string",
            );
        }
    }

    fn boolean(&mut self, key: &str, slot: &mut bool) {
        if let Some(v) = self.get(key).cloned() {
            self.set(key, slot, v.as_bool(), "a boolean");
        }
    }

    fn int(&mut self, key: &str, slot: &mut i64) {
        if let Some(v) = self.get(key).cloned() {
            self.set(key, slot, v.as_integer(), "an integer");
        }
    }

    fn float(&mut self, key: &str, slot: &mut f64) {
        if let Some(v) = self.get(key).cloned() {
            let parsed = v.as_float().or_else(|| v.as_integer().map(|i| i as f64));
            self.set(key, slot, parsed, "a number");
        }
    }

    fn map(&mut self, key: &str, slot: &mut Vec<(String, String)>) {
        if let Some(v) = self.get(key).cloned() {
            let parsed = v.as_table().and_then(|t| {
                t.iter()
                    .map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
                    .collect::<Option<Vec<_>>>()
            });
            self.set(key, slot, parsed, "a table of strings");
        }
    }
}

/// Walk up from `start_dir` looking for a project config; stops at a
/// directory containing `.git` or at the filesystem root.
pub fn find_project_config(start_dir: &Path) -> Option<PathBuf> {
    let mut current = std::fs::canonicalize(start_dir).unwrap_or_else(|_| start_dir.to_path_buf());
    loop {
        for name in CONFIG_FILENAMES {
            let candidate = current.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        if current.join(".git").exists() {
            return None;
        }
        match current.parent() {
            Some(parent) if parent != current => current = parent.to_path_buf(),
            _ => return None,
        }
    }
}

/// Parse a TOML file.
pub fn load_toml_file(path: &Path) -> Result<toml::Table, ConfigError> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| ConfigError(format!("Cannot read config file {}: {e}", path.display())))?;
    text.parse::<toml::Table>()
        .map_err(|e| ConfigError(format!("Invalid TOML in {}: {e}", path.display())))
}

/// Paths of the config files that would be loaded (`user`, `project`).
pub fn get_config_paths() -> BTreeMap<&'static str, Option<PathBuf>> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    get_config_paths_with(&cwd, &user_config_path())
}

/// [`get_config_paths`] with an explicit start dir and user-config path.
pub fn get_config_paths_with(
    start_dir: &Path,
    user_config: &Path,
) -> BTreeMap<&'static str, Option<PathBuf>> {
    BTreeMap::from([
        (
            "user",
            user_config.exists().then(|| user_config.to_path_buf()),
        ),
        ("project", find_project_config(start_dir)),
    ])
}

/// Template config file with every option documented (all commented out).
pub fn generate_template() -> &'static str {
    TEMPLATE
}

const TEMPLATE: &str = r#"# kicad-tools configuration file
# Place as .kicad-tools.toml in project root or ~/.config/kicad-tools/config.toml for user defaults

[defaults]
# Output format: table, json, csv
# format = "table"

# Default manufacturer for DRC checks: jlcpcb, pcbway, oshpark, seeed
# manufacturer = "jlcpcb"

# Enable verbose output by default
# verbose = false

# Enable quiet mode by default
# quiet = false

[display]
# Unit system for output: "mm" or "mils"
# units = "mm"

# Decimal precision for mm values (default: 3)
# precision_mm = 3

# Decimal precision for mils values (default: 1)
# precision_mils = 1

[drc]
# Use strict DRC checking
# strict = false

# Default number of PCB layers
# layers = 2

[export]
# Default output directory for exports
# output_dir = "./manufacturing"

# Include DNP (Do Not Populate) components in exports
# include_dnp = false

[route]
# Routing strategy: basic, negotiated, monte-carlo
# strategy = "negotiated"

# Grid resolution in mm
# grid_resolution = 0.1

# Default trace width in mm
# trace_width = 0.2

# Default clearance in mm
# clearance = 0.2

# Via drill size in mm
# via_drill = 0.3

# Via diameter in mm
# via_diameter = 0.6

[parts]
# Cache directory for LCSC parts data
# cache_dir = "~/.cache/kicad-tools/lcsc"

# Cache TTL in days
# cache_ttl_days = 7

[footprint_validation]
# Path to KiCad standard footprint library (auto-detected if not set)
# kicad_library_path = "/Applications/KiCad/KiCad.app/Contents/SharedSupport/footprints"

# Tolerance for dimension comparison in mm
# tolerance_mm = 0.05

# Custom mappings from footprint names to library directories
# Useful for non-standard naming conventions
# [footprint_validation.library_mappings]
# "CustomCap_0402" = "Capacitor_SMD"
# "MyResistor_0603" = "Resistor_SMD"

[footprint_selection]
# Footprint selection profile: "default", "machine", "hand_solder", "compact"
# - default: Balanced profile for general use
# - machine: Optimized for pick & place assembly (smaller packages)
# - hand_solder: Larger packages easier to hand solder
# - compact: Smallest packages that can handle the values
# profile = "default"

# Custom capacitor footprint rules (overrides profile defaults)
# Format: "value_range" = "Footprint:Name"
# [footprint_selection.capacitor]
# "0-100nF" = "Capacitor_SMD:C_0402_1005Metric"
# "100nF-1uF" = "Capacitor_SMD:C_0603_1608Metric"
# "1uF-10uF" = "Capacitor_SMD:C_0805_2012Metric"
# "10uF+" = "Capacitor_SMD:C_1206_3216Metric"

# Custom resistor footprint rules (overrides profile defaults)
# [footprint_selection.resistor]
# "0-10k" = "Resistor_SMD:R_0402_1005Metric"
# "10k+" = "Resistor_SMD:R_0603_1608Metric"

# Custom inductor footprint rules (overrides profile defaults)
# [footprint_selection.inductor]
# "0-10uH" = "Inductor_SMD:L_0603_1608Metric"
# "10uH+" = "Inductor_SMD:L_0805_2012Metric"
"#;
