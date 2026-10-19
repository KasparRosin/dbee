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
    /// Binaries and build tooling: exempt from the rules.
    Exempt,
}

impl Class {
    fn layer(self) -> Option<u8> {
        match self {
            Class::Layer(l) => Some(l),
            Class::Exempt => None,
        }
    }
}

/// The layering table (AGENTS.md "Layering", docs/architecture.md). Names are package names
/// without the `dbee-` prefix. L0–L4 are free for the domain crates added with the first
/// features; register each one here in the same change.
pub const TABLE: &[(&str, Class)] = &[
    ("core", Class::Layer(0)),
    ("testkit", Class::Layer(0)),
    ("sql", Class::Layer(1)),
    ("config", Class::Layer(1)),
    ("ssh", Class::Layer(2)),
    ("engine", Class::Layer(5)),
    ("ui-egui", Class::Layer(6)),
    ("mcp", Class::Layer(6)),
    // L7 apps (exempt) and tooling (unchecked)
    ("dbee", Class::Layer(7)),
    ("cli", Class::Layer(7)),
    ("xtask", Class::Exempt),
];

/// Explicit orderings *within* a layer (earlier may be used by later), e.g. `&["core", "types"]`.
/// Each chain is separate. None yet.
pub const INTRA_LAYER_ORDER: &[&[&str]] = &[];

fn intra_layer_allowed(from: &str, to: &str) -> bool {
    let (from, to) = (short_name(from), short_name(to));
    INTRA_LAYER_ORDER.iter().any(|chain| match (chain.iter().position(|n| *n == from), chain.iter().position(|n| *n == to)) {
        (Some(f), Some(t)) => t < f,
        _ => false,
    })
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
    let s = short_name(pkg);
    TABLE.iter().find(|(n, _)| *n == s).map(|(_, c)| *c)
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
        }
    }
}

/// Check all rules. Returns violations sorted for stable output.
pub fn check(crates: &[Crate]) -> Vec<Violation> {
    let mut out = Vec::new();
    for c in crates {
        let Some(class) = classify(&c.name) else {
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
                let Some(dc) = classify(&d.name) else { continue };
                let to = dc.layer().unwrap_or(u8::MAX);
                if to >= layer && !(to == layer && intra_layer_allowed(&c.name, &d.name)) {
                    out.push(Violation::Upward {
                        krate: c.name.clone(),
                        dep: d.name.clone(),
                        from: layer,
                        to: if to == u8::MAX { 7 } else { to },
                        kind: d.kind,
                    });
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
        Some(Class::Exempt) => "unchecked".into(),
        None => "UNREGISTERED".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use DepKind::*;

    fn c(name: &str, deps: &[(&str, DepKind, bool)]) -> Crate {
        Crate { name: name.into(), deps: deps.iter().map(|(n, k, w)| Dep { name: (*n).into(), kind: *k, workspace: *w }).collect() }
    }

    #[test]
    fn table_lists_only_dbee_crates() {
        let names: Vec<&str> = TABLE.iter().map(|(n, _)| *n).collect();
        assert_eq!(names, vec!["core", "testkit", "sql", "config", "ssh", "engine", "ui-egui", "mcp", "dbee", "cli", "xtask"]);
        assert!(INTRA_LAYER_ORDER.is_empty());
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
