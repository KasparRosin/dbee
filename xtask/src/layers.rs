//! `cargo xtask layers`: dependency layering rules (AGENTS.md "Layering", docs/architecture.md).
//!
//! The rule engine works on a small, metadata-independent model so it can be
//! unit-tested; `from_metadata` builds that model from `cargo metadata`.

use serde_json::Value;

/// Where a workspace crate sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// Regular layered crate.
    Layer(u8),
    /// A database driver, at [`DRIVER_LAYER`]. Only the registry, the apps and test code may use
    /// it.
    Driver,
    /// The driver registry, at [`DRIVER_LAYER`]: it may use every driver, and only the apps and
    /// test code may use it.
    Registry,
    /// Binaries and build tooling: exempt from the rules.
    Exempt,
}

/// The layer of the drivers and their registry.
pub const DRIVER_LAYER: u8 = 4;

impl Class {
    fn layer(self) -> Option<u8> {
        match self {
            Class::Layer(l) => Some(l),
            Class::Driver | Class::Registry => Some(DRIVER_LAYER),
            Class::Exempt => None,
        }
    }
}

/// The layering table (AGENTS.md "Layering", docs/architecture.md). Names are package names
/// without the `dbee-` prefix. Register each new crate here in the same change; a new driver is
/// one `Class::Driver` line, which both driver rules then cover.
pub const TABLE: &[(&str, Class)] = &[
    ("core", Class::Layer(0)),
    ("testkit", Class::Layer(0)),
    ("sql", Class::Layer(1)),
    ("config", Class::Layer(1)),
    ("ssh", Class::Layer(2)),
    ("update", Class::Layer(2)),
    ("driver", Class::Layer(3)),
    ("pg", Class::Driver),
    ("mysql", Class::Driver),
    ("sqlite", Class::Driver),
    ("drivers", Class::Registry),
    ("engine", Class::Layer(5)),
    ("ui-egui", Class::Layer(6)),
    ("mcp", Class::Layer(6)),
    // L7 apps (exempt) and tooling (unchecked)
    ("dbee", Class::Layer(7)),
    ("cli", Class::Layer(7)),
    ("xtask", Class::Exempt),
];

/// Explicit orderings *within* a layer (earlier may be used by later), e.g. `&["core", "types"]`.
/// Each chain is separate. None yet: the registry's use of each driver comes from their classes.
pub const INTRA_LAYER_ORDER: &[&[&str]] = &[];

fn intra_layer_allowed(from: (&str, Class), to: (&str, Class)) -> bool {
    if from.1 == Class::Registry && to.1 == Class::Driver {
        return true;
    }
    let (from, to) = (short_name(from.0), short_name(to.0));
    INTRA_LAYER_ORDER.iter().any(|chain| match (chain.iter().position(|n| *n == from), chain.iter().position(|n| *n == to)) {
        (Some(f), Some(t)) => t < f,
        _ => false,
    })
}

/// NO_DRIVER_DEPS: the crates between the drivers and the apps (the engine and the UIs) may take
/// a driver or the registry only as a dev-dependency, so they hold drivers through `dbee-driver`'s
/// interface alone.
fn driver_dependency(layer: u8, to: Class, kind: DepKind) -> bool {
    layer > DRIVER_LAYER && matches!(to, Class::Driver | Class::Registry) && kind != DepKind::Dev
}

/// External crates that constitute a UI toolkit / windowing dependency.
/// Entries ending in `*` are prefixes.
pub const UI_CRATES: &[&str] = &["egui*", "eframe", "winit", "rfd", "wgpu*"];

/// First layer allowed to use UI crates.
pub const UI_MIN_LAYER: u8 = 6;

pub fn short_name(pkg: &str) -> &str {
    pkg.strip_prefix("dbee-").unwrap_or(pkg)
}

pub fn classify(pkg: &str) -> Option<Class> {
    classify_in(TABLE, pkg)
}

fn classify_in(table: &[(&str, Class)], pkg: &str) -> Option<Class> {
    let s = short_name(pkg);
    table.iter().find(|(n, _)| *n == s).map(|(_, c)| *c)
}

fn is_ui_crate(name: &str) -> bool {
    UI_CRATES.iter().any(|p| match p.strip_suffix('*') {
        Some(prefix) => name.starts_with(prefix),
        None => name == *p,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepKind {
    Normal,
    Dev,
    Build,
}

#[derive(Debug, Clone)]
pub struct Dep {
    pub name: String,
    pub kind: DepKind,
    /// `true` if the dependency is a workspace member.
    pub workspace: bool,
}

#[derive(Debug, Clone)]
pub struct Crate {
    pub name: String,
    pub deps: Vec<Dep>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Violation {
    Unregistered { krate: String },
    Upward { krate: String, dep: String, from: u8, to: u8, kind: DepKind },
    UiBelowL6 { krate: String, dep: String, layer: u8 },
    DriverDependency { krate: String, dep: String, kind: DepKind },
}

impl std::fmt::Display for Violation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Violation::Unregistered { krate } => {
                write!(f, "{krate}: unknown workspace crate; register it in xtask/src/layers.rs TABLE (see docs/architecture.md)")
            }
            Violation::Upward { krate, dep, from, to, kind } => {
                write!(f, "{krate} (L{from}) -> {dep} (L{to}) [{kind:?}]: may only depend on strictly lower layers")
            }
            Violation::UiBelowL6 { krate, dep, layer } => {
                write!(f, "{krate} (L{layer}) depends on UI crate `{dep}`; UI toolkits are only allowed in L6+")
            }
            Violation::DriverDependency { krate, dep, kind } => {
                write!(
                    f,
                    "{krate} -> {dep} [{kind:?}]: the engine and the UIs may use a driver crate only as a dev-dependency; they reach drivers through dbee-driver"
                )
            }
        }
    }
}

/// Check all rules. Returns violations sorted for stable output.
pub fn check(crates: &[Crate]) -> Vec<Violation> {
    check_in(TABLE, crates)
}

fn check_in(table: &[(&str, Class)], crates: &[Crate]) -> Vec<Violation> {
    let mut out = Vec::new();
    for c in crates {
        let Some(class) = classify_in(table, &c.name) else {
            out.push(Violation::Unregistered { krate: c.name.clone() });
            continue;
        };
        // Tooling and the L7 apps sit on top of everything.
        let Some(layer) = class.layer().filter(|l| *l < 7) else {
            continue;
        };
        for d in &c.deps {
            // Self dev-dependencies (e.g. to enable features in tests) are fine.
            if d.name == c.name {
                continue;
            }
            if d.workspace {
                // Unregistered deps are reported on their own entry.
                let Some(dc) = classify_in(table, &d.name) else { continue };
                let to = dc.layer().unwrap_or(u8::MAX);
                if to >= layer && !(to == layer && intra_layer_allowed((&c.name, class), (&d.name, dc))) {
                    out.push(Violation::Upward {
                        krate: c.name.clone(),
                        dep: d.name.clone(),
                        from: layer,
                        to: if to == u8::MAX { 7 } else { to },
                        kind: d.kind,
                    });
                } else if driver_dependency(layer, dc, d.kind) {
                    out.push(Violation::DriverDependency { krate: c.name.clone(), dep: d.name.clone(), kind: d.kind });
                }
            } else if layer < UI_MIN_LAYER && is_ui_crate(&d.name) {
                out.push(Violation::UiBelowL6 { krate: c.name.clone(), dep: d.name.clone(), layer });
            }
        }
    }
    out.sort_by_key(|v| v.to_string());
    out.dedup();
    out
}

/// Build the model from `cargo metadata --format-version 1 --no-deps`.
pub fn from_metadata(meta: &Value) -> Result<Vec<Crate>, String> {
    let pkgs = meta["packages"].as_array().ok_or("metadata: no packages array")?;
    let members: Vec<&str> = pkgs.iter().filter_map(|p| p["name"].as_str()).collect();
    let mut out = Vec::new();
    for p in pkgs {
        let name = p["name"].as_str().ok_or("package without name")?.to_owned();
        let mut deps = Vec::new();
        for d in p["dependencies"].as_array().into_iter().flatten() {
            let dname = d["name"].as_str().unwrap_or_default().to_owned();
            let kind = match d["kind"].as_str() {
                Some("dev") => DepKind::Dev,
                Some("build") => DepKind::Build,
                _ => DepKind::Normal,
            };
            let workspace = members.contains(&dname.as_str()) || d["path"].is_string();
            deps.push(Dep { name: dname, kind, workspace });
        }
        out.push(Crate { name, deps });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

pub fn describe(class: Option<Class>) -> String {
    match class {
        Some(Class::Layer(7)) => "L7 app".into(),
        Some(Class::Layer(l)) => format!("L{l}"),
        Some(Class::Driver) => format!("L{DRIVER_LAYER} driver"),
        Some(Class::Registry) => format!("L{DRIVER_LAYER} registry"),
        Some(Class::Exempt) => "unchecked".into(),
        None => "UNREGISTERED".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use DepKind::*;

    #[test]
    fn driver_crates_sit_at_l4() {
        for name in ["pg", "mysql", "sqlite", "drivers"] {
            assert_eq!(classify(name).and_then(Class::layer), Some(4), "{name}");
        }
        assert_eq!(classify("driver"), Some(Class::Layer(3)));
        assert_eq!(describe(classify("pg")), "L4 driver");
        assert_eq!(describe(classify("drivers")), "L4 registry");
    }

    fn c(name: &str, deps: &[(&str, DepKind, bool)]) -> Crate {
        Crate { name: name.into(), deps: deps.iter().map(|(n, k, w)| Dep { name: (*n).into(), kind: *k, workspace: *w }).collect() }
    }

    /// Workspace crates by short name, each with its workspace dependencies.
    fn model(crates: &[(&str, &[(&str, DepKind)])]) -> Vec<Crate> {
        crates
            .iter()
            .map(|(name, deps)| Crate {
                name: format!("dbee-{name}"),
                deps: deps.iter().map(|(n, k)| Dep { name: format!("dbee-{n}"), kind: *k, workspace: true }).collect(),
            })
            .collect()
    }

    #[test]
    fn table_lists_only_dbee_crates() {
        let names: Vec<&str> = TABLE.iter().map(|(n, _)| *n).collect();
        assert_eq!(
            names,
            vec![
                "core", "testkit", "sql", "config", "ssh", "update", "driver", "pg", "mysql", "sqlite", "drivers", "engine", "ui-egui", "mcp",
                "dbee", "cli", "xtask"
            ]
        );
        assert!(INTRA_LAYER_ORDER.is_empty());
    }

    #[test]
    fn engine_may_not_depend_on_a_driver() {
        let m = model(&[("engine", &[("pg", Normal)])]);
        assert!(matches!(check(&m)[..], [Violation::DriverDependency { .. }]), "{:?}", check(&m));
        for (from, to) in [("engine", "drivers"), ("ui-egui", "mysql"), ("mcp", "sqlite"), ("ui-egui", "drivers")] {
            let v = check(&model(&[(from, &[(to, Normal)])]));
            assert!(
                matches!(&v[..], [Violation::DriverDependency { krate, dep, .. }] if krate.ends_with(from) && dep.ends_with(to)),
                "{from} -> {to}: {v:?}"
            );
        }
        let v = check(&model(&[("engine", &[("pg", Build)])]));
        assert!(matches!(v[..], [Violation::DriverDependency { kind: Build, .. }]), "{v:?}");
    }

    #[test]
    fn engine_may_test_with_drivers() {
        let m = model(&[("engine", &[("drivers", Dev)])]);
        assert!(check(&m).is_empty());
        let m = model(&[("engine", &[("pg", Dev), ("driver", Normal)]), ("ui-egui", &[("drivers", Dev)]), ("mcp", &[("sqlite", Dev)])]);
        assert!(check(&m).is_empty(), "{:?}", check(&m));
    }

    #[test]
    fn drivers_may_use_each_driver() {
        let m = model(&[("drivers", &[("pg", Normal), ("mysql", Normal), ("sqlite", Normal), ("driver", Normal)])]);
        assert!(check(&m).is_empty(), "{:?}", check(&m));
        let v = check(&model(&[("pg", &[("mysql", Normal)])]));
        assert!(matches!(&v[..], [Violation::Upward { from: 4, to: 4, .. }]), "{v:?}");
        let v = check(&model(&[("pg", &[("drivers", Normal)])]));
        assert!(matches!(&v[..], [Violation::Upward { from: 4, to: 4, .. }]), "a driver never names the registry: {v:?}");
    }

    #[test]
    fn a_new_driver_crate_is_one_table_line() {
        let table: Vec<(&str, Class)> = TABLE.iter().copied().chain([("oracle", Class::Driver)]).collect();
        let v = check_in(&table, &model(&[("engine", &[("oracle", Normal)])]));
        assert!(matches!(&v[..], [Violation::DriverDependency { .. }]), "{v:?}");
        assert!(check_in(&table, &model(&[("engine", &[("oracle", Dev)])])).is_empty());
        assert!(check_in(&table, &model(&[("drivers", &[("oracle", Normal)])])).is_empty());
        let v = check_in(&table, &model(&[("oracle", &[("pg", Normal)])]));
        assert!(matches!(&v[..], [Violation::Upward { from: 4, to: 4, .. }]), "{v:?}");
        assert!(check_in(&table, &model(&[("oracle", &[("driver", Normal), ("ssh", Normal)])])).is_empty());
    }

    #[test]
    fn clean_downward_graph_passes() {
        let g = vec![
            c("dbee-engine", &[("serde_json", Normal, false)]),
            c("dbee-ui-egui", &[("dbee-engine", Normal, true), ("egui", Normal, false), ("wgpu", Normal, false)]),
            c("dbee-mcp", &[("dbee-engine", Normal, true)]),
            c("dbee-cli", &[("dbee-mcp", Normal, true), ("dbee-engine", Normal, true)]),
            c("dbee", &[("dbee-ui-egui", Normal, true), ("eframe", Normal, false)]),
        ];
        assert_eq!(check(&g), vec![]);
    }

    #[test]
    fn upward_dependency_flagged() {
        let v = check(&[c("dbee-engine", &[("dbee-ui-egui", Normal, true)])]);
        assert!(matches!(&v[..], [Violation::Upward { from: 5, to: 6, .. }]), "{v:?}");
    }

    #[test]
    fn sideways_dependency_flagged() {
        let v = check(&[c("dbee-ui-egui", &[("dbee-mcp", Normal, true)])]);
        assert!(matches!(&v[..], [Violation::Upward { from: 6, to: 6, .. }]), "{v:?}");
    }

    #[test]
    fn self_dev_dependency_ignored() {
        assert!(check(&[c("dbee-engine", &[("dbee-engine", Dev, true)])]).is_empty());
    }

    #[test]
    fn upward_dev_dependency_flagged() {
        let v = check(&[c("dbee-engine", &[("dbee-mcp", Dev, true)])]);
        assert!(matches!(&v[..], [Violation::Upward { kind: Dev, .. }]), "{v:?}");
    }

    #[test]
    fn ui_crates_below_l6_flagged() {
        for dep in ["egui", "eframe", "winit", "egui_kittest", "egui_extras", "rfd", "wgpu", "wgpu-core"] {
            let v = check(&[c("dbee-engine", &[(dep, Normal, false)])]);
            assert!(matches!(v[..], [Violation::UiBelowL6 { layer: 5, .. }]), "{dep}");
        }
        assert!(check(&[c("dbee-engine", &[("serde_json", Normal, false)])]).is_empty());
        assert!(check(&[c("dbee-mcp", &[("winit", Normal, false)])]).is_empty());
    }

    #[test]
    fn unregistered_crate_is_error() {
        let v = check(&[c("dbee-mystery", &[])]);
        assert!(matches!(&v[..], [Violation::Unregistered { krate }] if krate == "dbee-mystery"));
        assert!(v[0].to_string().contains("register"));
    }

    #[test]
    fn apps_and_xtask_exempt() {
        for app in ["dbee", "dbee-cli", "xtask"] {
            assert!(check(&[c(app, &[("egui", Normal, false), ("dbee-ui-egui", Normal, true)])]).is_empty(), "{app}");
        }
    }

    #[test]
    fn metadata_parsing() {
        let meta: Value = serde_json::from_str(
            r#"{"packages":[
                {"name":"dbee-mcp","dependencies":[
                    {"name":"dbee-engine","kind":null,"path":"/x/crates/engine"},
                    {"name":"serde_json","kind":null},
                    {"name":"proptest","kind":"dev"}]},
                {"name":"dbee-engine","dependencies":[]}
            ]}"#,
        )
        .unwrap();
        let g = from_metadata(&meta).unwrap();
        assert_eq!(g.len(), 2);
        let mcp = g.iter().find(|c| c.name == "dbee-mcp").unwrap();
        assert!(mcp.deps[0].workspace && !mcp.deps[1].workspace);
        assert_eq!(mcp.deps[2].kind, Dev);
        assert!(check(&g).is_empty());
    }
}
