//! Entrypoint inference for `xelian add` (SPEC.md §12.2 step 3, H-112).
//!
//! This is the step that decides whether an imported repository is runnable
//! at all. `launch.rs` executes the entrypoint as a file path, so inference
//! must resolve to a real relative path within the package — it cannot fall
//! back to "run this module somehow".
//!
//! Inference is a list of ordered strategies per language, most authoritative
//! first. Each strategy yields a candidate; the first candidate that either
//! exists on disk, or is a declared build output, wins.
//!
//! The build-output case matters: TypeScript MCP servers overwhelmingly
//! declare `dist/index.js`, which does not exist in a fresh checkout and only
//! appears after `npm run build`. Rejecting candidates that don't yet exist
//! would make those repos permanently un-importable.

use std::path::Path;

use serde::Deserialize;

use crate::manifest::Language;

/// An inferred entrypoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entrypoint {
    /// Package-relative path, e.g. `dist/index.js` or `src/agent/__main__.py`.
    pub path: String,
    /// Whether the file is present in the checkout right now. `false` means
    /// it is a declared build output that a build step must produce.
    pub exists: bool,
}

/// Infer the entrypoint for a checked-out project, or `None` when no strategy
/// produces a candidate.
///
/// `name_hint` is the repository (or monorepo subdirectory) name. It matters
/// because the checkout directory is named after the commit SHA, so the
/// directory itself carries no clue about what the project is called — and
/// several conventions key off the project name (`devika.py`, `metagpt/`).
pub fn infer(checkout: &Path, language: Language, name_hint: &str) -> Option<Entrypoint> {
    match language {
        Language::Python => infer_python(checkout, name_hint),
        Language::Node => infer_node(checkout),
    }
}

// ---------------------------------------------------------------------------
// Python
// ---------------------------------------------------------------------------

#[derive(Deserialize, Default)]
struct PyProject {
    project: Option<PyProjectTable>,
    tool: Option<ToolTable>,
}

#[derive(Deserialize, Default)]
struct PyProjectTable {
    name: Option<String>,
    #[serde(default)]
    scripts: std::collections::BTreeMap<String, String>,
}

#[derive(Deserialize, Default)]
struct ToolTable {
    poetry: Option<PoetryTable>,
}

#[derive(Deserialize, Default)]
struct PoetryTable {
    name: Option<String>,
    #[serde(default)]
    scripts: std::collections::BTreeMap<String, String>,
}

fn read_pyproject(checkout: &Path) -> PyProject {
    std::fs::read_to_string(checkout.join("pyproject.toml"))
        .ok()
        .and_then(|s| toml::from_str::<PyProject>(&s).ok())
        .unwrap_or_default()
}

/// Console-script targets declared anywhere a Python project declares them:
/// PEP 621 `[project.scripts]`, Poetry's `[tool.poetry.scripts]`, and
/// `setup.py`'s `entry_points`.
///
/// Returned in declaration order within each source, with test/benchmark
/// helpers filtered out — `gpte_test_application` and `bench` are not how a
/// user runs the tool.
fn console_script_targets(checkout: &Path, project: &PyProjectTable) -> Vec<String> {
    let pyproject = read_pyproject(checkout);
    let poetry = pyproject
        .tool
        .unwrap_or_default()
        .poetry
        .unwrap_or_default();

    let mut out: Vec<(String, String)> = Vec::new();
    out.extend(project.scripts.iter().map(|(k, v)| (k.clone(), v.clone())));
    out.extend(poetry.scripts.iter().map(|(k, v)| (k.clone(), v.clone())));
    out.extend(setup_py_entry_points(checkout));

    out.retain(|(name, target)| !is_auxiliary_script(name, target));
    out.into_iter().map(|(_, target)| target).collect()
}

/// Whether a console script is a development helper rather than the program.
fn is_auxiliary_script(name: &str, target: &str) -> bool {
    const MARKERS: &[&str] = &["test", "bench", "lint", "docs", "dev", "migrate"];
    let n = name.to_lowercase();
    let t = target.to_lowercase();
    MARKERS
        .iter()
        .any(|m| n.contains(m) || t.split(':').next().unwrap_or("").contains(m))
}

/// Extract `console_scripts` entries from a `setup.py`.
///
/// `setup.py` is executable Python, so this reads the literal
/// `"name=module:func"` strings rather than trying to evaluate it. That
/// covers the overwhelmingly common case (a literal list) and simply finds
/// nothing for anything cleverer, which falls through to the next strategy.
fn setup_py_entry_points(checkout: &Path) -> Vec<(String, String)> {
    let Ok(contents) = std::fs::read_to_string(checkout.join("setup.py")) else {
        return Vec::new();
    };
    let Some(idx) = contents.find("console_scripts") else {
        return Vec::new();
    };
    // Bound the scan to the entry_points block so unrelated strings later in
    // the file cannot be mistaken for script declarations.
    let window: String = contents[idx..].chars().take(2000).collect();

    let mut out = Vec::new();
    for raw in window.split(['"', '\'']) {
        let Some((name, target)) = raw.split_once('=') else {
            continue;
        };
        let (name, target) = (name.trim(), target.trim());
        if name.is_empty() || !target.contains(':') {
            continue;
        }
        if name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
        {
            out.push((name.to_string(), target.to_string()));
        }
    }
    out
}

fn infer_python(checkout: &Path, name_hint: &str) -> Option<Entrypoint> {
    let pyproject = read_pyproject(checkout);
    let project = pyproject.project.unwrap_or_default();

    // 1. A console script is the author's own statement of "this is how you
    //    run it" — the most authoritative signal there is.
    for target in console_script_targets(checkout, &project) {
        if let Some(found) = resolve_python_target(checkout, &target) {
            return Some(found);
        }
    }

    // 2. `<pkg>/__main__.py`, derived from the declared project name. This is
    //    the `python -m <pkg>` convention that most modern agents ship.
    let declared_names: Vec<String> = [
        project.name.clone(),
        read_pyproject(checkout)
            .tool
            .unwrap_or_default()
            .poetry
            .unwrap_or_default()
            .name,
        Some(name_hint.to_string()),
    ]
    .into_iter()
    .flatten()
    .collect();

    for name in &declared_names {
        if let Some(found) = find_module_main(checkout, &module_name(name)) {
            return Some(found);
        }
    }

    // 3. Conventional single-file entrypoints at the package root.
    const CANDIDATES: &[&str] = &[
        "main.py",
        "app.py",
        "run.py",
        "cli.py",
        "server.py",
        "__main__.py",
        "src/main.py",
        "src/app.py",
        "src/run.py",
        "src/cli.py",
        "src/server.py",
        "src/__main__.py",
    ];
    for candidate in CANDIDATES {
        if checkout.join(candidate).is_file() {
            return Some(Entrypoint {
                path: (*candidate).to_string(),
                exists: true,
            });
        }
    }

    // 4. A root script named for the project itself (`devika.py`).
    for name in &declared_names {
        let candidate = format!("{}.py", module_name(name));
        if checkout.join(&candidate).is_file() {
            return Some(Entrypoint {
                path: candidate,
                exists: true,
            });
        }
    }

    // 5. A single top-level package containing `__main__.py` or a
    //    conventionally named runner (`metagpt/main.py`).
    single_package_entrypoint(checkout, &declared_names)
}

/// Normalize a distribution name to its import name: `mcp-atlassian` →
/// `mcp_atlassian` (PEP 503 in reverse, as actually practiced).
fn module_name(dist_name: &str) -> String {
    dist_name.trim().to_lowercase().replace(['-', '.'], "_")
}

/// Resolve a `[project.scripts]` target (`pkg.module:func` or `pkg:func`) to
/// a file, under both flat and `src/` layouts.
fn resolve_python_target(checkout: &Path, target: &str) -> Option<Entrypoint> {
    let module_path = target.split(':').next()?.trim();
    if module_path.is_empty() {
        return None;
    }
    let rel = module_path.replace('.', "/");

    for prefix in ["", "src/"] {
        // `pkg.cli:main` → pkg/cli.py
        let as_module = format!("{prefix}{rel}.py");
        if checkout.join(&as_module).is_file() {
            return Some(Entrypoint {
                path: as_module,
                exists: true,
            });
        }
        // `pkg:main` where pkg is a package → pkg/__main__.py
        let as_package = format!("{prefix}{rel}/__main__.py");
        if checkout.join(&as_package).is_file() {
            return Some(Entrypoint {
                path: as_package,
                exists: true,
            });
        }
    }
    None
}

/// Find `<module>/__main__.py` under a flat or `src/` layout.
fn find_module_main(checkout: &Path, module: &str) -> Option<Entrypoint> {
    if module.is_empty() {
        return None;
    }
    for prefix in ["", "src/"] {
        let p = format!("{prefix}{module}/__main__.py");
        if checkout.join(&p).is_file() {
            return Some(Entrypoint {
                path: p,
                exists: true,
            });
        }
    }
    None
}

/// Files inside a package directory that conventionally start the program.
const PACKAGE_RUNNERS: &[&str] = &["__main__.py", "main.py", "cli.py", "app.py", "server.py"];

/// Look inside a package directory for a conventional runner.
///
/// Prefers the package named after the project; otherwise accepts a single
/// unambiguous package. Ambiguity yields `None` rather than a guess — a wrong
/// entrypoint is worse than a clear failure the user can act on.
fn single_package_entrypoint(checkout: &Path, declared_names: &[String]) -> Option<Entrypoint> {
    for (prefix, base) in [("", checkout.to_path_buf()), ("src/", checkout.join("src"))] {
        let Ok(entries) = std::fs::read_dir(&base) else {
            continue;
        };
        let mut packages: Vec<String> = entries
            .flatten()
            .filter(|e| e.path().is_dir())
            .filter_map(|e| e.file_name().to_str().map(str::to_string))
            .filter(|n| !n.starts_with('.') && !matches!(n.as_str(), "tests" | "test" | "docs"))
            .filter(|n| base.join(n).join("__init__.py").is_file())
            .collect();
        packages.sort();

        // A package matching the project's own name is unambiguous even when
        // several packages exist (`metagpt/` beside `examples/`).
        let named: Vec<String> = declared_names
            .iter()
            .map(|n| module_name(n))
            .filter(|n| packages.contains(n))
            .collect();

        let chosen: Option<&String> = named.first().or(if packages.len() == 1 {
            packages.first()
        } else {
            None
        });

        if let Some(pkg) = chosen {
            for runner in PACKAGE_RUNNERS {
                let p = format!("{prefix}{pkg}/{runner}");
                if checkout.join(&p).is_file() {
                    return Some(Entrypoint {
                        path: p,
                        exists: true,
                    });
                }
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Node
// ---------------------------------------------------------------------------

#[derive(Deserialize, Default)]
struct PackageJson {
    name: Option<String>,
    main: Option<String>,
    bin: Option<serde_json::Value>,
    #[serde(default)]
    scripts: std::collections::BTreeMap<String, String>,
}

fn read_package_json(checkout: &Path) -> PackageJson {
    std::fs::read_to_string(checkout.join("package.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<PackageJson>(&s).ok())
        .unwrap_or_default()
}

/// Whether a `package.json` declares a build script — the signal that a
/// not-yet-existing entrypoint is legitimately a build output.
pub fn has_build_script(checkout: &Path) -> bool {
    read_package_json(checkout)
        .scripts
        .get("build")
        .is_some_and(|s| !s.trim().is_empty())
}

fn infer_node(checkout: &Path) -> Option<Entrypoint> {
    let pkg = read_package_json(checkout);
    let buildable = pkg
        .scripts
        .get("build")
        .is_some_and(|s| !s.trim().is_empty());

    // Accept a candidate if it exists, or if a build script could produce it.
    //
    // A TypeScript source is never acceptable as-is: `node foo.ts` fails, so
    // accepting one would produce a package that can never launch. When a
    // build exists, redirect to its compiled output instead; when it does
    // not, reject and let a later strategy (or a clear error) take over.
    let accept = |path: String| -> Option<Entrypoint> {
        if is_typescript(&path) {
            return buildable.then(|| Entrypoint {
                path: compiled_output_for(&path),
                exists: false,
            });
        }
        let exists = checkout.join(&path).is_file();
        if exists {
            Some(Entrypoint { path, exists: true })
        } else if buildable && looks_like_build_output(&path) {
            Some(Entrypoint {
                path,
                exists: false,
            })
        } else {
            None
        }
    };

    // 1. `bin` — for an MCP server or CLI this is the real entrypoint, and it
    //    is more specific than `main` (which often points at a library index).
    if let Some(path) = bin_entry(&pkg) {
        if let Some(found) = accept(normalize(&path)) {
            return Some(found);
        }
    }

    // 2. `main`.
    if let Some(main) = pkg.main.as_deref() {
        if let Some(found) = accept(normalize(main)) {
            return Some(found);
        }
    }

    // 3. Conventional locations that exist today.
    const CANDIDATES: &[&str] = &[
        "index.js",
        "index.mjs",
        "index.cjs",
        "src/index.js",
        "dist/index.js",
        "build/index.js",
        "server.js",
        "src/server.js",
    ];
    for candidate in CANDIDATES {
        if checkout.join(candidate).is_file() {
            return Some(Entrypoint {
                path: (*candidate).to_string(),
                exists: true,
            });
        }
    }

    // 4. A TypeScript source with a build script implies the conventional
    //    compiled output even when nothing declares it.
    if buildable {
        for src in ["src/index.ts", "src/main.ts", "index.ts"] {
            if checkout.join(src).is_file() {
                return Some(Entrypoint {
                    path: "dist/index.js".to_string(),
                    exists: false,
                });
            }
        }
    }

    None
}

/// Pick the `bin` entry to use: a string `bin` directly; for an object, the
/// entry whose key matches the package name, else the single entry, else the
/// first by sorted key so the choice is deterministic across runs.
fn bin_entry(pkg: &PackageJson) -> Option<String> {
    match pkg.bin.as_ref()? {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Object(map) => {
            if map.is_empty() {
                return None;
            }
            if let Some(name) = pkg.name.as_deref() {
                // `@scope/pkg` → match on the unscoped name too.
                let unscoped = name.rsplit('/').next().unwrap_or(name);
                for key in [name, unscoped] {
                    if let Some(v) = map.get(key).and_then(|v| v.as_str()) {
                        return Some(v.to_string());
                    }
                }
            }
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            map.get(keys[0])
                .and_then(|v| v.as_str())
                .map(str::to_string)
        }
        _ => None,
    }
}

/// Strip a leading `./` so paths compare equal to archive-relative paths.
fn normalize(p: &str) -> String {
    p.trim().trim_start_matches("./").to_string()
}

/// Whether a path is a TypeScript source. Node cannot execute these.
fn is_typescript(path: &str) -> bool {
    path.ends_with(".ts") || path.ends_with(".tsx") || path.ends_with(".mts")
}

/// The conventional compiled location for a TypeScript source:
/// `src/index.ts` → `dist/index.js`, `index.ts` → `dist/index.js`.
fn compiled_output_for(path: &str) -> String {
    let stem = path
        .rsplit('/')
        .next()
        .unwrap_or(path)
        .trim_end_matches(".tsx")
        .trim_end_matches(".mts")
        .trim_end_matches(".ts");
    format!("dist/{stem}.js")
}

/// Whether a path looks like compiled output rather than a source file. Used
/// to decide if a missing candidate is worth waiting for a build to produce.
fn looks_like_build_output(path: &str) -> bool {
    let is_js = path.ends_with(".js") || path.ends_with(".mjs") || path.ends_with(".cjs");
    is_js
        && (path.starts_with("dist/")
            || path.starts_with("build/")
            || path.starts_with("lib/")
            || path.starts_with("out/")
            || !path.contains('/'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn write(dir: &Path, name: &str, contents: &str) {
        if let Some(parent) = Path::new(name).parent() {
            fs::create_dir_all(dir.join(parent)).unwrap();
        }
        fs::write(dir.join(name), contents).unwrap();
    }

    // ---- Python ----

    #[test]
    fn console_script_resolves_to_a_module_file() {
        // SWE-agent / crewAI shape.
        let d = tempdir().unwrap();
        write(
            d.path(),
            "pyproject.toml",
            "[project]\nname = \"sweagent\"\n[project.scripts]\nsweagent = \"sweagent.cli:main\"\n",
        );
        write(d.path(), "sweagent/cli.py", "def main(): pass\n");

        let e = infer(d.path(), Language::Python, "widget").unwrap();
        assert_eq!(e.path, "sweagent/cli.py");
        assert!(e.exists);
    }

    #[test]
    fn console_script_resolves_under_src_layout() {
        // mcp-atlassian shape.
        let d = tempdir().unwrap();
        write(
            d.path(),
            "pyproject.toml",
            "[project]\nname = \"mcp-atlassian\"\n[project.scripts]\nmcp-atlassian = \"mcp_atlassian:main\"\n",
        );
        write(d.path(), "src/mcp_atlassian/__init__.py", "");
        write(d.path(), "src/mcp_atlassian/__main__.py", "print(1)\n");

        let e = infer(d.path(), Language::Python, "widget").unwrap();
        assert_eq!(e.path, "src/mcp_atlassian/__main__.py");
    }

    #[test]
    fn dashed_project_name_maps_to_underscored_module() {
        let d = tempdir().unwrap();
        write(
            d.path(),
            "pyproject.toml",
            "[project]\nname = \"my-agent\"\n",
        );
        write(d.path(), "my_agent/__init__.py", "");
        write(d.path(), "my_agent/__main__.py", "print(1)\n");

        let e = infer(d.path(), Language::Python, "widget").unwrap();
        assert_eq!(e.path, "my_agent/__main__.py");
    }

    #[test]
    fn falls_back_to_conventional_filenames() {
        // SuperAGI shape: no pyproject, plain main.py.
        let d = tempdir().unwrap();
        write(d.path(), "requirements.txt", "fastapi\n");
        write(d.path(), "main.py", "print(1)\n");

        let e = infer(d.path(), Language::Python, "widget").unwrap();
        assert_eq!(e.path, "main.py");
    }

    #[test]
    fn single_package_with_dunder_main_is_found() {
        let d = tempdir().unwrap();
        write(d.path(), "requirements.txt", "x\n");
        write(d.path(), "theagent/__init__.py", "");
        write(d.path(), "theagent/__main__.py", "print(1)\n");

        let e = infer(d.path(), Language::Python, "widget").unwrap();
        assert_eq!(e.path, "theagent/__main__.py");
    }

    #[test]
    fn ambiguous_multi_package_layout_yields_none_not_a_guess() {
        let d = tempdir().unwrap();
        write(d.path(), "requirements.txt", "x\n");
        write(d.path(), "alpha/__init__.py", "");
        write(d.path(), "alpha/__main__.py", "print(1)\n");
        write(d.path(), "beta/__init__.py", "");
        write(d.path(), "beta/__main__.py", "print(2)\n");

        assert_eq!(infer(d.path(), Language::Python, "widget"), None);
    }

    #[test]
    fn poetry_style_scripts_are_honoured() {
        // gpt-engineer shape: `[tool.poetry.scripts]`, no `[project.scripts]`.
        let d = tempdir().unwrap();
        write(
            d.path(),
            "pyproject.toml",
            "[tool.poetry]\nname = \"gpt-engineer\"\n\
             [tool.poetry.scripts]\ngpte = \"gpt_engineer.applications.cli.main:app\"\n",
        );
        write(
            d.path(),
            "gpt_engineer/applications/cli/main.py",
            "app = 1\n",
        );

        let e = infer(d.path(), Language::Python, "widget").unwrap();
        assert_eq!(e.path, "gpt_engineer/applications/cli/main.py");
    }

    #[test]
    fn test_and_benchmark_scripts_are_not_chosen_as_entrypoints() {
        // gpt-engineer also declares `bench` and `gpte_test_application`;
        // picking either would run the wrong program.
        let d = tempdir().unwrap();
        write(
            d.path(),
            "pyproject.toml",
            "[tool.poetry]\nname = \"x\"\n[tool.poetry.scripts]\n\
             bench = \"pkg.benchmark.__main__:app\"\n\
             gpte_test_application = \"tests.caching_main:app\"\n\
             gpte = \"pkg.cli:app\"\n",
        );
        write(d.path(), "pkg/benchmark/__main__.py", "1\n");
        write(d.path(), "tests/caching_main.py", "1\n");
        write(d.path(), "pkg/cli.py", "1\n");

        assert_eq!(
            infer(d.path(), Language::Python, "widget").unwrap().path,
            "pkg/cli.py"
        );
    }

    #[test]
    fn setup_py_console_scripts_are_parsed() {
        // MetaGPT shape: setup.py, no pyproject.
        let d = tempdir().unwrap();
        write(d.path(), "requirements.txt", "x\n");
        write(
            d.path(),
            "setup.py",
            "setup(\n  entry_points={\n    'console_scripts': [\n      \
             'metagpt=metagpt.software_company:app',\n    ],\n  },\n)\n",
        );
        write(d.path(), "metagpt/software_company.py", "app = 1\n");

        let e = infer(d.path(), Language::Python, "widget").unwrap();
        assert_eq!(e.path, "metagpt/software_company.py");
    }

    #[test]
    fn a_root_script_named_for_the_project_is_found() {
        // devika shape: devika.py at the repo root, many packages under src/.
        let d = tempdir().unwrap();
        write(d.path(), "requirements.txt", "flask\n");
        write(d.path(), "devika.py", "print(1)\n");
        write(d.path(), "src/agents/__init__.py", "");
        write(d.path(), "src/browser/__init__.py", "");

        // The repo name is the only clue: the checkout dir is a commit SHA.
        let e = infer(d.path(), Language::Python, "devika").unwrap();
        assert_eq!(e.path, "devika.py");
    }

    #[test]
    fn a_package_named_for_the_project_wins_over_sibling_packages() {
        // MetaGPT shape: metagpt/ beside examples/ and tests/.
        let d = tempdir().unwrap();
        write(
            d.path(),
            "pyproject.toml",
            "[project]\nname = \"metagpt\"\n",
        );
        write(d.path(), "metagpt/__init__.py", "");
        write(d.path(), "metagpt/main.py", "print(1)\n");
        write(d.path(), "examples/__init__.py", "");
        write(d.path(), "examples/main.py", "print(2)\n");

        let e = infer(d.path(), Language::Python, "widget").unwrap();
        assert_eq!(e.path, "metagpt/main.py");
    }

    #[test]
    fn library_with_no_entrypoint_yields_none() {
        // swarm / Voyager shape: importable library, nothing to run.
        let d = tempdir().unwrap();
        write(d.path(), "pyproject.toml", "[project]\nname = \"swarm\"\n");
        write(d.path(), "swarm/__init__.py", "");
        write(d.path(), "swarm/core.py", "class Swarm: pass\n");

        assert_eq!(infer(d.path(), Language::Python, "widget"), None);
    }

    // ---- Node ----

    #[test]
    fn bin_string_is_used() {
        // playwright-mcp shape.
        let d = tempdir().unwrap();
        write(
            d.path(),
            "package.json",
            r#"{"name":"playwright-mcp","bin":{"playwright-mcp":"cli.js"}}"#,
        );
        write(d.path(), "cli.js", "console.log(1)\n");

        let e = infer(d.path(), Language::Node, "widget").unwrap();
        assert_eq!(e.path, "cli.js");
        assert!(e.exists);
    }

    #[test]
    fn dist_entrypoint_is_accepted_as_a_build_output() {
        // Figma-Context-MCP shape: main points at dist/, which does not exist
        // until `npm run build` runs.
        let d = tempdir().unwrap();
        write(
            d.path(),
            "package.json",
            r#"{"name":"figma","main":"dist/index.js","scripts":{"build":"tsc"}}"#,
        );
        write(d.path(), "src/index.ts", "export {}\n");

        let e = infer(d.path(), Language::Node, "widget").unwrap();
        assert_eq!(e.path, "dist/index.js");
        assert!(!e.exists, "must be flagged as needing a build");
    }

    #[test]
    fn missing_dist_without_a_build_script_is_not_accepted() {
        let d = tempdir().unwrap();
        write(
            d.path(),
            "package.json",
            r#"{"name":"x","main":"dist/index.js"}"#,
        );
        assert_eq!(infer(d.path(), Language::Node, "widget"), None);
    }

    #[test]
    fn typescript_source_with_build_implies_dist_output() {
        // firecrawl shape: bin points at dist/, src/index.ts present.
        let d = tempdir().unwrap();
        write(
            d.path(),
            "package.json",
            r#"{"name":"fc","scripts":{"build":"tsc"}}"#,
        );
        write(d.path(), "src/index.ts", "export {}\n");

        let e = infer(d.path(), Language::Node, "widget").unwrap();
        assert_eq!(e.path, "dist/index.js");
        assert!(!e.exists);
    }

    #[test]
    fn a_typescript_main_without_a_build_is_rejected() {
        // sentry-mcp-stdio shape: `main: index.ts`, no build script. Node
        // cannot execute a .ts file, so accepting it would produce a package
        // that can never launch.
        let d = tempdir().unwrap();
        write(
            d.path(),
            "package.json",
            r#"{"name":"s","main":"index.ts","bin":{"s":"./index.ts"}}"#,
        );
        write(d.path(), "index.ts", "export {}\n");

        assert_eq!(infer(d.path(), Language::Node, "s"), None);
    }

    #[test]
    fn a_typescript_main_with_a_build_redirects_to_compiled_output() {
        let d = tempdir().unwrap();
        write(
            d.path(),
            "package.json",
            r#"{"name":"s","main":"src/index.ts","scripts":{"build":"tsc"}}"#,
        );
        write(d.path(), "src/index.ts", "export {}\n");

        let e = infer(d.path(), Language::Node, "s").unwrap();
        assert_eq!(e.path, "dist/index.js");
        assert!(!e.exists);
    }

    #[test]
    fn compiled_output_mapping() {
        assert_eq!(compiled_output_for("src/index.ts"), "dist/index.js");
        assert_eq!(compiled_output_for("index.ts"), "dist/index.js");
        assert_eq!(compiled_output_for("src/bin.mts"), "dist/bin.js");
    }

    #[test]
    fn bin_map_prefers_the_entry_matching_the_package_name() {
        let d = tempdir().unwrap();
        write(
            d.path(),
            "package.json",
            r#"{"name":"tool","bin":{"aaa":"a.js","tool":"t.js"}}"#,
        );
        write(d.path(), "a.js", "1\n");
        write(d.path(), "t.js", "1\n");

        assert_eq!(
            infer(d.path(), Language::Node, "widget").unwrap().path,
            "t.js"
        );
    }

    #[test]
    fn scoped_package_name_matches_unscoped_bin_key() {
        let d = tempdir().unwrap();
        write(
            d.path(),
            "package.json",
            r#"{"name":"@acme/tool","bin":{"tool":"t.js","zzz":"z.js"}}"#,
        );
        write(d.path(), "t.js", "1\n");
        write(d.path(), "z.js", "1\n");

        assert_eq!(
            infer(d.path(), Language::Node, "widget").unwrap().path,
            "t.js"
        );
    }

    #[test]
    fn leading_dot_slash_is_normalized() {
        let d = tempdir().unwrap();
        write(d.path(), "package.json", r#"{"main":"./index.js"}"#);
        write(d.path(), "index.js", "1\n");

        assert_eq!(
            infer(d.path(), Language::Node, "widget").unwrap().path,
            "index.js"
        );
    }

    #[test]
    fn workspace_root_with_nothing_runnable_yields_none() {
        // modelcontextprotocol/servers root shape.
        let d = tempdir().unwrap();
        write(
            d.path(),
            "package.json",
            r#"{"name":"servers","workspaces":["src/*"]}"#,
        );
        assert_eq!(infer(d.path(), Language::Node, "widget"), None);
    }

    #[test]
    fn has_build_script_detects_a_build() {
        let d = tempdir().unwrap();
        write(d.path(), "package.json", r#"{"scripts":{"build":"tsc"}}"#);
        assert!(has_build_script(d.path()));

        let d2 = tempdir().unwrap();
        write(d2.path(), "package.json", r#"{"scripts":{"test":"jest"}}"#);
        assert!(!has_build_script(d2.path()));
    }
}
