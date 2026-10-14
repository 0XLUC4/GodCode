//! A look through the whole project: how much of each language, the biggest files, the notes
//! left in the code (TODO, FIXME…) and the dependencies that have a new major version.

use std::collections::HashMap;
use std::sync::OnceLock;

use regex::Regex;

/// A note left in the code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Note {
    /// TODO, FIXME, HACK, XXX, BUG.
    pub tag: String,
    /// 1-based.
    pub line: usize,
    pub text: String,
}

/// Notes in comments: `// TODO: …`, `# FIXME …`, `/* HACK */`, `<!-- XXX -->`.
pub fn notes(text: &str) -> Vec<Note> {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    let pattern = PATTERN.get_or_init(|| {
        Regex::new(r"(?://+|#|/\*+|\*|--|<!--|;+|%)\s*\b(TODO|FIXME|HACK|XXX|BUG)\b[\s:(\-]*(.*)")
            .expect("valid note pattern")
    });
    text.lines()
        .enumerate()
        .filter(|(_, line)| line.len() < 2000)
        .filter_map(|(index, line)| {
            let captures = pattern.captures(line)?;
            let text = captures[2]
                .trim()
                .trim_end_matches("*/")
                .trim_end_matches("-->")
                .trim()
                .to_owned();
            Some(Note {
                tag: captures[1].to_owned(),
                line: index + 1,
                text,
            })
        })
        .collect()
}

/// Lines of code, blank lines left out.
pub fn code_lines(text: &str) -> usize {
    text.lines().filter(|line| !line.trim().is_empty()).count()
}

/// Totals per language, biggest first.
#[derive(Default)]
pub struct Languages {
    totals: HashMap<String, (usize, usize)>,
}

impl Languages {
    pub fn add(&mut self, language: &str, lines: usize) {
        let entry = self.totals.entry(language.to_owned()).or_default();
        entry.0 += 1;
        entry.1 += lines;
    }

    /// `(language, files, lines)`, most lines first.
    pub fn sorted(&self) -> Vec<(String, usize, usize)> {
        let mut list: Vec<_> = self
            .totals
            .iter()
            .map(|(language, (files, lines))| (language.clone(), *files, *lines))
            .collect();
        list.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.cmp(&b.0)));
        list
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Registry {
    Crates,
    Npm,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dependency {
    pub name: String,
    /// What the manifest asks for: `1.2`, `^4.0.0`, `~0.3`.
    pub requirement: String,
    pub registry: Registry,
    pub dev: bool,
}

/// `[dependencies]`, `[dev-dependencies]`, `[build-dependencies]`, `[workspace.dependencies]`
/// and their `[target.….dependencies]`; path and workspace dependencies are left out.
pub fn cargo_dependencies(manifest: &str) -> Vec<Dependency> {
    let mut found = Vec::new();
    let mut section: Option<bool> = None;
    for raw in manifest.lines() {
        let line = raw.split(" #").next().unwrap_or(raw).trim();
        if line.starts_with('[') {
            let name = line.trim_matches(['[', ']']).trim();
            section = if name.ends_with("dev-dependencies") {
                Some(true)
            } else if name.ends_with("dependencies") {
                Some(false)
            } else {
                None
            };
            continue;
        }
        let Some(dev) = section else {
            continue;
        };
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        let name = name.trim().trim_matches('"');
        let value = value.trim();
        let requirement = if let Some(text) = value.strip_prefix('"') {
            text.split('"').next().map(str::to_owned)
        } else if value.starts_with('{') {
            let local = ["path", "workspace", "git"].iter().any(|key| {
                value.contains(&format!("{key} =")) || value.contains(&format!("{key}="))
            });
            if local {
                None
            } else {
                value
                    .split("version")
                    .nth(1)
                    .and_then(|rest| rest.split('"').nth(1))
                    .map(str::to_owned)
            }
        } else {
            None
        };
        if let Some(requirement) = requirement
            && !name.is_empty()
            && !name.contains('.')
        {
            let real_name = value
                .split("package")
                .nth(1)
                .and_then(|rest| rest.split('"').nth(1))
                .unwrap_or(name);
            found.push(Dependency {
                name: real_name.to_owned(),
                requirement,
                registry: Registry::Crates,
                dev,
            });
        }
    }
    found
}

/// `dependencies` and `devDependencies` of a `package.json`; links and files are left out.
pub fn npm_dependencies(manifest: &str) -> Vec<Dependency> {
    let Ok(json) = serde_json::from_str::<serde_json::Value>(manifest) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for (key, dev) in [("dependencies", false), ("devDependencies", true)] {
        let Some(map) = json.get(key).and_then(serde_json::Value::as_object) else {
            continue;
        };
        for (name, value) in map {
            let Some(requirement) = value.as_str() else {
                continue;
            };
            let local = [
                "file:",
                "link:",
                "workspace:",
                "git",
                "http",
                "github:",
                "npm:",
            ]
            .iter()
            .any(|start| requirement.starts_with(start));
            if !local && requirement != "*" && requirement != "latest" {
                found.push(Dependency {
                    name: name.clone(),
                    requirement: requirement.to_owned(),
                    registry: Registry::Npm,
                    dev,
                });
            }
        }
    }
    found
}

fn numbers(version: &str) -> Vec<u64> {
    version
        .trim_start_matches(|ch: char| !ch.is_ascii_digit())
        .split(['.', '-', '+'])
        .take(3)
        .map_while(|part| part.parse().ok())
        .collect()
}

/// The latest version breaks what the requirement accepts: a new major version
/// (or a new minor one under 1.0, as Cargo and npm read `^0.x`).
pub fn is_outdated(requirement: &str, latest: &str) -> bool {
    // `>=1, <3` or `1 || 2`: the highest bound is the last one written.
    let requirement = requirement
        .rsplit(['|', ','])
        .next()
        .unwrap_or(requirement)
        .trim();
    if requirement.starts_with('<') || requirement.starts_with(">=") {
        return false;
    }
    let wanted = numbers(requirement);
    let latest = numbers(latest);
    let (Some(&want_major), Some(&last_major)) = (wanted.first(), latest.first()) else {
        return false;
    };
    if last_major != want_major {
        return last_major > want_major;
    }
    if want_major == 0 {
        let want_minor = wanted.get(1).copied().unwrap_or(0);
        let last_minor = latest.get(1).copied().unwrap_or(0);
        return wanted.len() > 1 && last_minor > want_minor;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_notes_in_comments() {
        let text = "\
fn main() {
    // TODO: read the config
    let todo_list = 3; // not a note: todo_list
    # FIXME handle errors
    /* HACK until the API is fixed */
}
<!-- XXX remove -->";
        let found = notes(text);
        let tags: Vec<&str> = found.iter().map(|note| note.tag.as_str()).collect();
        assert_eq!(tags, ["TODO", "FIXME", "HACK", "XXX"]);
        assert_eq!(found[0].line, 2);
        assert_eq!(found[0].text, "read the config");
        assert_eq!(found[2].text, "until the API is fixed");
        assert_eq!(found[3].text, "remove");
    }

    #[test]
    fn reads_cargo_manifests() {
        let manifest = r#"
[package]
name = "demo"
version = "0.1.0"

[dependencies]
serde = { version = "1.0", features = ["derive"] }
regex = "1"
core = { path = "../core" }
shared.workspace = true
web = { package = "actix-web", version = "4" }

[target.'cfg(windows)'.dependencies]
winreg = "0.52" # registry

[dev-dependencies]
tempfile = "3"
"#;
        let found = cargo_dependencies(manifest);
        let names: Vec<&str> = found.iter().map(|dep| dep.name.as_str()).collect();
        assert_eq!(names, ["serde", "regex", "actix-web", "winreg", "tempfile"]);
        assert_eq!(found[3].requirement, "0.52");
        assert!(found[4].dev);
        assert!(!found[0].dev);
    }

    #[test]
    fn reads_package_json() {
        let manifest = r#"{"dependencies": {"react": "^18.2.0", "local": "file:../x"},
            "devDependencies": {"vite": "~5.0.0"}}"#;
        let found = npm_dependencies(manifest);
        assert_eq!(found.len(), 2);
        assert_eq!(found[1].name, "vite");
        assert!(found[1].dev);
    }

    #[test]
    fn tells_new_major_versions() {
        assert!(!is_outdated("1", "1.4.2"));
        assert!(is_outdated("1", "2.0.0"));
        assert!(is_outdated("^18.2.0", "19.0.1"));
        assert!(!is_outdated("~5.0.0", "5.4.0"));
        assert!(is_outdated("0.52", "0.53.0"));
        assert!(!is_outdated("0.52.1", "0.52.9"));
        assert!(!is_outdated(">=1.0", "9.0.0"));
        assert!(!is_outdated("2", "1.9.0"));
    }

    #[test]
    fn languages_add_up() {
        let mut languages = Languages::default();
        languages.add("Rust", 120);
        languages.add("Rust", 30);
        languages.add("TOML", 12);
        assert_eq!(languages.sorted()[0], ("Rust".to_owned(), 2, 150));
        assert_eq!(code_lines("a\n\n  \nb\n"), 2);
    }
}
