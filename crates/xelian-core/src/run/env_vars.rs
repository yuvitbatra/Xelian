use std::collections::BTreeMap;
use std::io::{self, IsTerminal, Write};
use std::path::Path;
use thiserror::Error;

use crate::manifest::EnvVarSpec;
use crate::secrets::{looks_secret, SecretStore};

#[derive(Debug, Error)]
pub enum EnvVarError {
    #[error("required environment variable {var:?} is not set; cannot launch")]
    MissingRequired { var: String },
}

/// Resolve environment variables from the manifest against the process
/// environment (SPEC.md §6.2.1, §9.10).
///
/// For each declared variable:
/// - If the var is set in the process environment, its value is used as-is.
/// - If the var is `required = true` and not set, [`EnvVarError::MissingRequired`]
///   is returned (launch is aborted).
/// - If the var has a `default` and is not set, the default is used.
/// - Unset non-required vars without a default are silently omitted.
pub fn resolve_env_vars(
    environment: &BTreeMap<String, EnvVarSpec>,
) -> Result<Vec<(String, String)>, EnvVarError> {
    resolve_env_vars_with(environment, |name| std::env::var(name))
}

/// Like [`resolve_env_vars`] but accepts a custom function for looking up
/// environment variables, making it testable without modifying global state.
pub fn resolve_env_vars_with<F>(
    environment: &BTreeMap<String, EnvVarSpec>,
    get_env: F,
) -> Result<Vec<(String, String)>, EnvVarError>
where
    F: Fn(&str) -> Result<String, std::env::VarError>,
{
    let mut result = Vec::new();
    for (name, spec) in environment {
        match get_env(name) {
            Ok(val) => {
                result.push((name.clone(), val));
            }
            Err(_) if spec.required => {
                return Err(EnvVarError::MissingRequired { var: name.clone() });
            }
            Err(_) => {
                if let Some(default) = &spec.default {
                    result.push((name.clone(), default.clone()));
                }
            }
        }
    }
    Ok(result)
}

/// Resolve env vars, but instead of aborting on a missing required variable,
/// **ask the user for it** (on a terminal), store it in the global secrets
/// store for reuse, and inject it — the "ridiculously easy setup" path so a
/// package that needs an API key just asks once and then runs.
///
/// Precedence per variable: process env → saved secret → manifest default →
/// interactive prompt (required only). Non-interactively (no TTY — CI, or an
/// MCP client driving stdin) a missing required variable is still a hard error,
/// since there is no human to ask.
pub fn resolve_env_vars_interactive(
    environment: &BTreeMap<String, EnvVarSpec>,
    secrets_path: &Path,
) -> Result<Vec<(String, String)>, EnvVarError> {
    let mut store = SecretStore::load(secrets_path).unwrap_or_default();
    let interactive = io::stdin().is_terminal();

    let (result, entered) = resolve_with_sources(
        environment,
        |name| std::env::var(name).ok(),
        |name| store.get(name).map(str::to_string),
        |name, _spec| {
            if interactive {
                prompt_for_var(name)
            } else {
                None
            }
        },
    )?;

    if !entered.is_empty() {
        for (k, v) in &entered {
            store.set(k, v);
        }
        match store.save(secrets_path) {
            Ok(()) => eprintln!(
                "  saved {} value{} to {} — you won't be asked again.",
                entered.len(),
                if entered.len() == 1 { "" } else { "s" },
                secrets_path.display()
            ),
            Err(e) => eprintln!("warning: could not save for reuse: {e}"),
        }
    }
    Ok(result)
}

/// Pure resolution with injectable sources (testable without a TTY or disk).
/// Returns the resolved pairs and any values newly obtained from `prompt`
/// (which the caller persists).
fn resolve_with_sources<E, S, P>(
    environment: &BTreeMap<String, EnvVarSpec>,
    env_get: E,
    stored_get: S,
    mut prompt: P,
) -> Result<(Vec<(String, String)>, Vec<(String, String)>), EnvVarError>
where
    E: Fn(&str) -> Option<String>,
    S: Fn(&str) -> Option<String>,
    P: FnMut(&str, &EnvVarSpec) -> Option<String>,
{
    let mut result = Vec::new();
    let mut entered = Vec::new();
    for (name, spec) in environment {
        if let Some(v) = env_get(name) {
            result.push((name.clone(), v));
        } else if let Some(v) = stored_get(name) {
            result.push((name.clone(), v));
        } else if let Some(default) = &spec.default {
            result.push((name.clone(), default.clone()));
        } else if spec.required {
            match prompt(name, spec) {
                Some(v) => {
                    result.push((name.clone(), v.clone()));
                    entered.push((name.clone(), v));
                }
                None => return Err(EnvVarError::MissingRequired { var: name.clone() }),
            }
        }
        // else: optional, unset, no default — omit.
    }
    Ok((result, entered))
}

/// Prompt on stderr for one variable's value (stdout may be an MCP transport).
/// Returns `None` on EOF/empty input so the caller reports the missing var.
fn prompt_for_var(name: &str) -> Option<String> {
    let note = if looks_secret(name) {
        " (stored securely in ~/.xelian/secrets.toml, reused next time)"
    } else {
        ""
    };
    eprintln!("\n  This package needs {name}.{note}");
    let _ = write!(io::stderr(), "  Enter {name}: ");
    let _ = io::stderr().flush();

    let mut line = String::new();
    match io::stdin().read_line(&mut line) {
        Ok(0) => None, // EOF: no human at the other end
        Ok(_) => {
            let v = line.trim().to_string();
            (!v.is_empty()).then_some(v)
        }
        Err(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_var(required: bool, default: Option<&str>) -> EnvVarSpec {
        EnvVarSpec {
            required,
            default: default.map(|s| s.to_string()),
        }
    }

    fn mock_env<'a>(
        pairs: &'a [(&'a str, &'a str)],
    ) -> impl Fn(&str) -> Result<String, std::env::VarError> + 'a {
        move |name| {
            for (k, v) in pairs {
                if *k == name {
                    return Ok(v.to_string());
                }
            }
            Err(std::env::VarError::NotPresent)
        }
    }

    #[test]
    fn missing_required_var_returns_error() {
        let mut env = BTreeMap::new();
        env.insert("MUST_EXIST".to_string(), env_var(true, None));
        let err = resolve_env_vars_with(&env, mock_env(&[])).unwrap_err();
        assert!(matches!(err, EnvVarError::MissingRequired { var } if var == "MUST_EXIST"));
    }

    #[test]
    fn default_is_applied_when_var_is_unset() {
        let mut env = BTreeMap::new();
        env.insert("PORT".to_string(), env_var(false, Some("8080")));
        let pairs = resolve_env_vars_with(&env, mock_env(&[])).unwrap();
        assert_eq!(pairs, vec![("PORT".to_string(), "8080".to_string())]);
    }

    #[test]
    fn process_env_takes_precedence_over_default() {
        let mut env = BTreeMap::new();
        env.insert("PORT".to_string(), env_var(false, Some("8080")));
        let pairs = resolve_env_vars_with(&env, mock_env(&[("PORT", "9000")])).unwrap();
        assert_eq!(pairs, vec![("PORT".to_string(), "9000".to_string())]);
    }

    #[test]
    fn unset_non_required_var_without_default_is_omitted() {
        let mut env = BTreeMap::new();
        env.insert("OPTIONAL".to_string(), env_var(false, None));
        let pairs = resolve_env_vars_with(&env, mock_env(&[])).unwrap();
        assert!(pairs.is_empty());
    }

    #[test]
    fn required_var_from_process_env_succeeds() {
        let mut env = BTreeMap::new();
        env.insert("API_KEY".to_string(), env_var(true, None));
        let pairs = resolve_env_vars_with(&env, mock_env(&[("API_KEY", "secret")])).unwrap();
        assert_eq!(pairs, vec![("API_KEY".to_string(), "secret".to_string())]);
    }

    #[test]
    fn prompt_supplies_missing_required_and_is_recorded_as_entered() {
        let mut env = BTreeMap::new();
        env.insert("OPENAI_API_KEY".to_string(), env_var(true, None));
        let (pairs, entered) = resolve_with_sources(
            &env,
            |_| None, // not in process env
            |_| None, // not in store
            |name, _| Some(format!("typed-{name}")),
        )
        .unwrap();
        assert_eq!(
            pairs,
            vec![(
                "OPENAI_API_KEY".to_string(),
                "typed-OPENAI_API_KEY".to_string()
            )]
        );
        assert_eq!(
            entered,
            vec![(
                "OPENAI_API_KEY".to_string(),
                "typed-OPENAI_API_KEY".to_string()
            )]
        );
    }

    #[test]
    fn stored_secret_is_used_without_prompting() {
        let mut env = BTreeMap::new();
        env.insert("OPENAI_API_KEY".to_string(), env_var(true, None));
        let (pairs, entered) = resolve_with_sources(
            &env,
            |_| None,
            |name| (name == "OPENAI_API_KEY").then(|| "sk-stored".to_string()),
            |_, _| panic!("must not prompt when a stored value exists"),
        )
        .unwrap();
        assert_eq!(
            pairs,
            vec![("OPENAI_API_KEY".to_string(), "sk-stored".to_string())]
        );
        assert!(entered.is_empty());
    }

    #[test]
    fn process_env_beats_stored_and_no_prompt() {
        let mut env = BTreeMap::new();
        env.insert("OPENAI_API_KEY".to_string(), env_var(true, None));
        let (pairs, entered) = resolve_with_sources(
            &env,
            |name| (name == "OPENAI_API_KEY").then(|| "sk-env".to_string()),
            |_| Some("sk-stored".to_string()),
            |_, _| panic!("must not prompt when the process env has it"),
        )
        .unwrap();
        assert_eq!(
            pairs,
            vec![("OPENAI_API_KEY".to_string(), "sk-env".to_string())]
        );
        assert!(entered.is_empty());
    }

    #[test]
    fn declining_the_prompt_is_a_hard_error() {
        let mut env = BTreeMap::new();
        env.insert("NEEDED".to_string(), env_var(true, None));
        let err = resolve_with_sources(&env, |_| None, |_| None, |_, _| None).unwrap_err();
        assert!(matches!(err, EnvVarError::MissingRequired { var } if var == "NEEDED"));
    }

    #[test]
    fn multiple_vars_resolve_correctly() {
        let mut env = BTreeMap::new();
        env.insert("REQUIRED_ONE".to_string(), env_var(true, None));
        env.insert("DEFAULTED".to_string(), env_var(false, Some("dval")));
        env.insert("OPTIONAL".to_string(), env_var(false, None));

        let pairs = resolve_env_vars_with(&env, mock_env(&[("REQUIRED_ONE", "val1")])).unwrap();
        let mut pairs_sorted = pairs.clone();
        pairs_sorted.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(
            pairs_sorted,
            vec![
                ("DEFAULTED".to_string(), "dval".to_string()),
                ("REQUIRED_ONE".to_string(), "val1".to_string()),
            ]
        );
    }
}
