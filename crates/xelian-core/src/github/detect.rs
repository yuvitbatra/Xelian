//! Project language detection for `xelian add` (SPEC.md §12.2 step 2, H-111).
//!
//! SPEC §12.2 fixes the mechanism — an ordered marker-file table, extended by
//! appending rows rather than by redesigning the precedence — and this module
//! implements exactly that. Two things it adds over a plain table:
//!
//! - Markers for the Python projects that predate `pyproject.toml`
//!   (`setup.py`, `requirements.txt`). Without these, a large share of
//!   published agents are simply undetectable.
//! - A tiebreak for repositories carrying *both* a Python marker and a
//!   `package.json`. A bare table would call these Node purely because of
//!   table order, which mis-detects Python projects that merely ship a
//!   `package.json` for frontend assets or tooling.

use std::path::Path;

use super::GithubError;
use crate::manifest::Language;

/// What a marker file implies about the project.
pub enum DetectionOutcome {
    /// A language Xelian has a runtime manager for.
    Language(Language),
    /// A language Xelian recognizes but cannot run in V1 (§22). Detected
    /// explicitly so the user gets "Go is not supported" rather than the
    /// misleading "could not detect project language".
    Unsupported(&'static str),
}

/// Marker table, checked in order; first match wins. Extend by appending a
/// row (SPEC.md §12.2 step 2).
///
/// `pyproject.toml` precedes `package.json` per SPEC. The remaining Python
/// markers follow `package.json` and are reached via the tiebreak below.
const LANGUAGE_MARKERS: &[(&str, DetectionOutcome)] = &[
    (
        "pyproject.toml",
        DetectionOutcome::Language(Language::Python),
    ),
    ("package.json", DetectionOutcome::Language(Language::Node)),
    ("setup.py", DetectionOutcome::Language(Language::Python)),
    ("setup.cfg", DetectionOutcome::Language(Language::Python)),
    (
        "requirements.txt",
        DetectionOutcome::Language(Language::Python),
    ),
    ("Cargo.toml", DetectionOutcome::Unsupported("rust")),
    ("go.mod", DetectionOutcome::Unsupported("go")),
    ("pom.xml", DetectionOutcome::Unsupported("java")),
    ("Gemfile", DetectionOutcome::Unsupported("ruby")),
];

/// Python marker files that, when present alongside `package.json`, make a
/// repository ambiguous.
const PYTHON_MARKERS: &[&str] = &[
    "pyproject.toml",
    "setup.py",
    "setup.cfg",
    "requirements.txt",
];

/// Detect a checked-out project's language (SPEC.md §12.2 step 2).
pub fn detect_language(checkout: &Path) -> Result<Language, GithubError> {
    // Ambiguity first: a repo with both a Python marker and package.json is
    // Python unless package.json actually describes a runnable Node project.
    if checkout.join("package.json").is_file() {
        let has_python = PYTHON_MARKERS.iter().any(|m| checkout.join(m).is_file());
        if has_python && !package_json_looks_runnable(checkout) {
            return Ok(Language::Python);
        }
    }

    for (marker, outcome) in LANGUAGE_MARKERS {
        if checkout.join(marker).is_file() {
            return match outcome {
                DetectionOutcome::Language(lang) => Ok(*lang),
                DetectionOutcome::Unsupported(name) => Err(GithubError::UnsupportedLanguage {
                    language: (*name).to_string(),
                }),
            };
        }
    }

    Err(GithubError::UndetectedLanguage {
        path: checkout.display().to_string(),
    })
}

/// Whether `package.json` describes something Xelian could actually launch,
/// as opposed to a bag of frontend/tooling dependencies sitting next to a
/// Python project.
///
/// A `main`, a `bin`, or a `build` script means the package.json is the
/// runnable artifact. Only `dependencies`/`devDependencies` means it is not.
fn package_json_looks_runnable(checkout: &Path) -> bool {
    let Ok(contents) = std::fs::read_to_string(checkout.join("package.json")) else {
        return false;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&contents) else {
        return false;
    };

    if v.get("main").and_then(|m| m.as_str()).is_some() {
        return true;
    }
    if v.get("bin").is_some() {
        return true;
    }
    v.get("scripts")
        .and_then(|s| s.get("build"))
        .and_then(|b| b.as_str())
        .is_some()
}

pub fn language_label(language: Language) -> &'static str {
    match language {
        Language::Python => "python",
        Language::Node => "node",
    }
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

    #[test]
    fn pyproject_beats_package_json() {
        let d = tempdir().unwrap();
        write(d.path(), "pyproject.toml", "[project]\nname='x'\n");
        write(d.path(), "package.json", "{}");
        assert_eq!(detect_language(d.path()).unwrap(), Language::Python);
    }

    #[test]
    fn package_json_alone_is_node() {
        let d = tempdir().unwrap();
        write(d.path(), "package.json", "{}");
        assert_eq!(detect_language(d.path()).unwrap(), Language::Node);
    }

    #[test]
    fn setup_py_projects_are_detected() {
        // MetaGPT / Voyager shape: setup.py + requirements.txt, no pyproject.
        let d = tempdir().unwrap();
        write(d.path(), "setup.py", "from setuptools import setup\n");
        write(d.path(), "requirements.txt", "requests\n");
        assert_eq!(detect_language(d.path()).unwrap(), Language::Python);
    }

    #[test]
    fn requirements_only_projects_are_detected() {
        // devika shape.
        let d = tempdir().unwrap();
        write(d.path(), "requirements.txt", "flask\n");
        assert_eq!(detect_language(d.path()).unwrap(), Language::Python);
    }

    #[test]
    fn python_project_with_a_tooling_package_json_is_python() {
        // SuperAGI shape: main.py + requirements.txt + a package.json that
        // carries only dependencies. Table order alone would say Node.
        let d = tempdir().unwrap();
        write(d.path(), "requirements.txt", "fastapi\n");
        write(d.path(), "main.py", "print(1)\n");
        write(
            d.path(),
            "package.json",
            r#"{"dependencies":{"tailwindcss":"^3"}}"#,
        );
        assert_eq!(detect_language(d.path()).unwrap(), Language::Python);
    }

    #[test]
    fn python_project_with_a_runnable_package_json_is_node() {
        // A genuine Node package that also vendors a requirements.txt for a
        // helper script stays Node.
        let d = tempdir().unwrap();
        write(d.path(), "requirements.txt", "requests\n");
        write(
            d.path(),
            "package.json",
            r#"{"main":"index.js","scripts":{"build":"tsc"}}"#,
        );
        assert_eq!(detect_language(d.path()).unwrap(), Language::Node);
    }

    #[test]
    fn go_is_a_clear_unsupported_error_not_undetected() {
        let d = tempdir().unwrap();
        write(d.path(), "go.mod", "module example.com/x\n");
        match detect_language(d.path()).unwrap_err() {
            GithubError::UnsupportedLanguage { language } => assert_eq!(language, "go"),
            other => panic!("expected UnsupportedLanguage, got {other:?}"),
        }
    }

    #[test]
    fn rust_remains_a_clear_unsupported_error() {
        let d = tempdir().unwrap();
        write(d.path(), "Cargo.toml", "[package]\nname='x'\n");
        match detect_language(d.path()).unwrap_err() {
            GithubError::UnsupportedLanguage { language } => assert_eq!(language, "rust"),
            other => panic!("expected UnsupportedLanguage, got {other:?}"),
        }
    }

    #[test]
    fn empty_dir_is_undetected() {
        let d = tempdir().unwrap();
        assert!(matches!(
            detect_language(d.path()).unwrap_err(),
            GithubError::UndetectedLanguage { .. }
        ));
    }
}
