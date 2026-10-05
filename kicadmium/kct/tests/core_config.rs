//! Port of upstream tests/test_config.py (USER_CONFIG_PATH monkeypatching
//! maps to the explicit user-config path arguments).

use std::path::{Path, PathBuf};

use kct::config::{
    find_project_config, generate_template, get_config_paths_with, load_toml_file, Config,
    DefaultsConfig, DrcConfig, ExportConfig, PartsConfig, RouteConfig,
};

fn git_root() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir(tmp.path().join(".git")).unwrap();
    tmp
}

fn canon(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap()
}

fn load(start: &Path, user: &Path) -> Config {
    Config::load_with_user_config(Some(start), user).unwrap()
}

#[test]
fn dataclass_defaults() {
    let d = DefaultsConfig::default();
    assert_eq!(d.format, "table");
    assert!(d.manufacturer.is_none() && !d.verbose && !d.quiet);
    let drc = DrcConfig::default();
    assert!(!drc.strict);
    assert_eq!(drc.layers, 2);
    let e = ExportConfig::default();
    assert_eq!(e.output_dir, "./manufacturing");
    assert!(!e.include_dnp);
    let r = RouteConfig::default();
    assert_eq!(r.strategy, "negotiated");
    assert_eq!(
        (
            r.grid_resolution,
            r.trace_width,
            r.clearance,
            r.via_drill,
            r.via_diameter
        ),
        (0.1, 0.2, 0.2, 0.3, 0.6)
    );
    let p = PartsConfig::default();
    assert_eq!(p.cache_dir, "~/.cache/kicad-tools/lcsc");
    assert_eq!(p.cache_ttl_days, 7);
    assert_eq!(Config::default().route, RouteConfig::default());
}

#[test]
fn discovery() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = tmp.path().join(".kicad-tools.toml");
    std::fs::write(&cfg, "[defaults]\nformat = 'json'\n").unwrap();
    assert_eq!(find_project_config(tmp.path()), Some(canon(&cfg)));

    let tmp = tempfile::tempdir().unwrap();
    let cfg = tmp.path().join("kicad-tools.toml");
    std::fs::write(&cfg, "[defaults]\n").unwrap();
    assert_eq!(find_project_config(tmp.path()), Some(canon(&cfg)));

    // Hidden name preferred.
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("kicad-tools.toml"), "").unwrap();
    let hidden = tmp.path().join(".kicad-tools.toml");
    std::fs::write(&hidden, "").unwrap();
    assert_eq!(find_project_config(tmp.path()), Some(canon(&hidden)));

    // Walks up.
    let tmp = tempfile::tempdir().unwrap();
    let parent_cfg = tmp.path().join(".kicad-tools.toml");
    std::fs::write(&parent_cfg, "").unwrap();
    let sub = tmp.path().join("src/deep");
    std::fs::create_dir_all(&sub).unwrap();
    assert_eq!(find_project_config(&sub), Some(canon(&parent_cfg)));

    // Stops at .git.
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join("parent/project");
    std::fs::create_dir_all(project.join(".git")).unwrap();
    std::fs::write(tmp.path().join("parent/.kicad-tools.toml"), "[defaults]\n").unwrap();
    assert_eq!(find_project_config(&project), None);

    // Found at the git root itself.
    let tmp = git_root();
    let cfg = tmp.path().join(".kicad-tools.toml");
    std::fs::write(&cfg, "[defaults]\n").unwrap();
    assert_eq!(find_project_config(tmp.path()), Some(canon(&cfg)));

    let tmp = git_root();
    assert_eq!(find_project_config(tmp.path()), None);
}

#[test]
fn toml_loading() {
    let tmp = tempfile::tempdir().unwrap();
    let f = tmp.path().join("config.toml");
    std::fs::write(
        &f,
        "\n[defaults]\nformat = \"json\"\nmanufacturer = \"jlcpcb\"\n\n[route]\ntrace_width = 0.3\n",
    )
    .unwrap();
    let t = load_toml_file(&f).unwrap();
    assert_eq!(t["defaults"]["format"].as_str(), Some("json"));
    assert_eq!(t["defaults"]["manufacturer"].as_str(), Some("jlcpcb"));
    assert_eq!(t["route"]["trace_width"].as_float(), Some(0.3));

    std::fs::write(&f, "invalid [ toml syntax").unwrap();
    assert!(load_toml_file(&f).unwrap_err().0.contains("Invalid TOML"));
    assert!(load_toml_file(&tmp.path().join("nonexistent.toml"))
        .unwrap_err()
        .0
        .contains("Cannot read"));
}

#[test]
fn load_defaults_project_and_user() {
    let tmp = git_root();
    let none = tmp.path().join("no-exist.toml");
    let c = load(tmp.path(), &none);
    assert_eq!(c.defaults.format, "table");
    assert!(c.defaults.manufacturer.is_none());
    assert_eq!(c.route.strategy, "negotiated");

    std::fs::write(
        tmp.path().join(".kicad-tools.toml"),
        "[defaults]\nformat = \"json\"\nmanufacturer = \"jlcpcb\"\n",
    )
    .unwrap();
    let c = load(tmp.path(), &none);
    assert_eq!(c.defaults.format, "json");
    assert_eq!(c.defaults.manufacturer.as_deref(), Some("jlcpcb"));

    let tmp = git_root();
    let user = tmp.path().join("user-config.toml");
    std::fs::write(
        &user,
        "[defaults]\nverbose = true\n\n[route]\ntrace_width = 0.25\n",
    )
    .unwrap();
    let c = load(tmp.path(), &user);
    assert!(c.defaults.verbose);
    assert_eq!(c.route.trace_width, 0.25);
}

#[test]
fn project_overrides_user_and_sources() {
    let tmp = git_root();
    let user = tmp.path().join("user-config.toml");
    std::fs::write(
        &user,
        "[defaults]\nformat = \"csv\"\nmanufacturer = \"pcbway\"\n",
    )
    .unwrap();
    std::fs::write(
        tmp.path().join(".kicad-tools.toml"),
        "[defaults]\nformat = \"json\"\n[route]\ntrace_width = 0.3\n",
    )
    .unwrap();
    let c = load(tmp.path(), &user);
    assert_eq!(c.defaults.format, "json");
    assert_eq!(c.defaults.manufacturer.as_deref(), Some("pcbway"));
    assert!(c
        .get_source("defaults.manufacturer")
        .contains("user-config.toml"));
    assert!(c
        .get_source("route.trace_width")
        .contains(".kicad-tools.toml"));
    assert!(c
        .get_source("defaults.format")
        .contains(".kicad-tools.toml"));
    assert_eq!(c.get_source("drc.strict"), "default");
}

#[test]
fn unknown_key_warnings() {
    let tmp = git_root();
    let none = tmp.path().join("no-exist.toml");
    std::fs::write(
        tmp.path().join(".kicad-tools.toml"),
        "[unknown_section]\nkey = \"value\"\n",
    )
    .unwrap();
    let c = load(tmp.path(), &none);
    assert_eq!(c.warnings.len(), 1);
    assert!(c.warnings[0].contains("unknown_section"));

    std::fs::write(
        tmp.path().join(".kicad-tools.toml"),
        "[defaults]\nunknown_key = \"value\"\n",
    )
    .unwrap();
    let c = load(tmp.path(), &none);
    assert_eq!(c.warnings.len(), 1);
    assert!(c.warnings[0].contains("defaults.unknown_key"));
}

#[test]
fn template() {
    let t = generate_template();
    let parsed: toml::Table = t.parse().unwrap();
    assert!(parsed.contains_key("defaults"));
    for s in ["[defaults]", "[drc]", "[export]", "[route]", "[parts]"] {
        assert!(t.contains(s));
    }
    for k in ["format", "manufacturer", "trace_width", "strategy"] {
        assert!(t.contains(k));
    }
}

#[test]
fn config_paths() {
    let tmp = git_root();
    let none = tmp.path().join("no-exist.toml");
    let p = get_config_paths_with(tmp.path(), &none);
    assert!(p.contains_key("user") && p.contains_key("project"));
    assert!(p["user"].is_none() && p["project"].is_none());

    let project = tmp.path().join(".kicad-tools.toml");
    std::fs::write(&project, "[defaults]\n").unwrap();
    let user = tmp.path().join("user.toml");
    std::fs::write(&user, "[defaults]\n").unwrap();
    let p = get_config_paths_with(tmp.path(), &user);
    assert_eq!(p["user"].as_deref(), Some(user.as_path()));
    assert_eq!(p["project"], Some(canon(&project)));
}

#[test]
fn full_merge_and_all_types() {
    let tmp = git_root();
    let user = tmp.path().join("user.toml");
    std::fs::write(
        &user,
        "[defaults]\nformat = \"csv\"\nmanufacturer = \"pcbway\"\nverbose = true\n\n\
         [route]\nstrategy = \"basic\"\ntrace_width = 0.15\n",
    )
    .unwrap();
    std::fs::write(
        tmp.path().join(".kicad-tools.toml"),
        "[defaults]\nformat = \"json\"\n\n[drc]\nstrict = true\nlayers = 4\n\n\
         [route]\ngrid_resolution = 0.5\n",
    )
    .unwrap();
    let c = load(tmp.path(), &user);
    assert_eq!(c.defaults.format, "json");
    assert_eq!(c.defaults.manufacturer.as_deref(), Some("pcbway"));
    assert!(c.defaults.verbose);
    assert!(c.drc.strict);
    assert_eq!(c.drc.layers, 4);
    assert_eq!(c.route.strategy, "basic");
    assert_eq!(c.route.grid_resolution, 0.5);
    assert_eq!(c.route.trace_width, 0.15);

    let tmp = git_root();
    std::fs::write(
        tmp.path().join(".kicad-tools.toml"),
        r#"
[defaults]
format = "json"
manufacturer = "jlcpcb"
verbose = true
quiet = false

[drc]
strict = true
layers = 4

[route]
strategy = "monte-carlo"
grid_resolution = 0.125
trace_width = 0.254
clearance = 0.127
via_drill = 0.4
via_diameter = 1

[parts]
cache_dir = "/custom/cache"
cache_ttl_days = 30

[footprint_selection]
profile = "compact"

[footprint_selection.capacitor]
"0-100nF" = "Capacitor_SMD:C_0402_1005Metric"
"100nF-1uF" = "Capacitor_SMD:C_0603_1608Metric"
"#,
    )
    .unwrap();
    let c = load(tmp.path(), &tmp.path().join("none.toml"));
    assert!(!c.defaults.quiet);
    assert_eq!(c.parts.cache_ttl_days, 30);
    assert_eq!(c.parts.cache_dir, "/custom/cache");
    assert_eq!(c.route.grid_resolution, 0.125);
    assert_eq!(c.route.trace_width, 0.254);
    assert_eq!(c.route.via_diameter, 1.0);
    assert_eq!(c.footprint_selection.profile, "compact");
    assert_eq!(
        c.footprint_selection.capacitor,
        vec![
            (
                "0-100nF".to_string(),
                "Capacitor_SMD:C_0402_1005Metric".to_string()
            ),
            (
                "100nF-1uF".to_string(),
                "Capacitor_SMD:C_0603_1608Metric".to_string()
            ),
        ]
    );
    assert!(c.warnings.is_empty(), "{:?}", c.warnings);
}
