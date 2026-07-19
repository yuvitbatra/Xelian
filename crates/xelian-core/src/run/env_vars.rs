use std::collections::BTreeMap;
use thiserror::Error;

use crate::manifest::EnvVarSpec;

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
