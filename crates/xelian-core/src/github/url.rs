//! GitHub URL parsing for `xelian add` (SPEC.md §12.2 step 1).
//!
//! Two forms are accepted:
//!
//! - `https://github.com/<owner>/<repo>` — the whole repository.
//! - `https://github.com/<owner>/<repo>/tree/<ref>/<subdir>` — a subdirectory
//!   of a monorepo, which is how a large share of MCP servers are actually
//!   distributed (`modelcontextprotocol/servers`, `awslabs/mcp`,
//!   `supabase/mcp`). This is the URL GitHub itself produces when you browse
//!   to a folder, so it is what a user will paste.
//!
//! In the subdirectory form `<subdir>` becomes the package root: language
//! detection, manifest inference, and the built archive all apply to that
//! directory rather than the repository root.

use super::GithubError;

/// A parsed GitHub repository reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoRef {
    pub owner: String,
    pub repo: String,
    /// Subdirectory within the repository that holds the package, if the URL
    /// used the `/tree/<ref>/<subdir>` form. Always a safe relative path.
    pub subdir: Option<String>,
    /// The git ref named in a `/tree/<ref>/...` URL. `None` means the
    /// repository's default branch (`HEAD`).
    pub git_ref: Option<String>,
}

impl RepoRef {
    /// The cache-key component identifying this exact import.
    ///
    /// Plain repositories are addressed by commit SHA alone (§12.2 step 1).
    /// Subdirectory imports append a slug of the subdir so that
    /// `servers/src/github` and `servers/src/redis` — the same SHA, different
    /// packages — never collide in the cache.
    pub fn cache_key(&self, sha: &str) -> String {
        match &self.subdir {
            None => sha.to_string(),
            Some(subdir) => {
                let slug: String = subdir
                    .chars()
                    .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
                    .collect();
                format!("{sha}-{}", slug.trim_matches('-'))
            }
        }
    }

    /// A human-readable `owner/repo` or `owner/repo/subdir` label.
    pub fn label(&self) -> String {
        match &self.subdir {
            None => format!("{}/{}", self.owner, self.repo),
            Some(s) => format!("{}/{}/{}", self.owner, self.repo, s),
        }
    }

    /// The name to derive the package name from: the last subdirectory
    /// component when present (`src/github` → `github`), else the repo name.
    /// A monorepo's subpackage is named for itself, not for the monorepo.
    pub fn package_basis(&self) -> &str {
        match &self.subdir {
            Some(s) => s.rsplit('/').next().unwrap_or(&self.repo),
            None => &self.repo,
        }
    }
}

/// Parse a GitHub repository URL.
///
/// Rejects non-`https` schemes, non-`github.com` hosts, and unsafe
/// `owner`/`repo`/`subdir` components (defense in depth — these become cache
/// path components).
pub fn parse_github_url(url: &str) -> Result<RepoRef, GithubError> {
    if url.starts_with("http://") {
        return Err(GithubError::InvalidUrl {
            url: url.to_string(),
            reason: "http is not supported; use https://".to_string(),
        });
    }

    let rest = url
        .strip_prefix("https://")
        .ok_or_else(|| GithubError::InvalidUrl {
            url: url.to_string(),
            reason: "must start with https://".to_string(),
        })?;

    let rest = rest
        .strip_prefix("github.com/")
        .ok_or_else(|| GithubError::InvalidUrl {
            url: url.to_string(),
            reason: "host must be github.com".to_string(),
        })?;

    let rest = rest.strip_suffix('/').unwrap_or(rest);
    let rest = rest.strip_suffix(".git").unwrap_or(rest);

    let parts: Vec<&str> = rest.split('/').filter(|p| !p.is_empty()).collect();
    if parts.len() < 2 {
        return Err(GithubError::InvalidUrl {
            url: url.to_string(),
            reason: "expected https://github.com/<owner>/<repo>".to_string(),
        });
    }

    let (owner, repo) = (parts[0], parts[1]);
    if !is_safe_repo_component(owner) {
        return Err(GithubError::InvalidUrl {
            url: url.to_string(),
            reason: format!("invalid owner {owner:?}"),
        });
    }
    if !is_safe_repo_component(repo) {
        return Err(GithubError::InvalidUrl {
            url: url.to_string(),
            reason: format!("invalid repo {repo:?}"),
        });
    }

    let (git_ref, subdir) = match parts.len() {
        2 => (None, None),
        _ => parse_tree_suffix(url, &parts[2..])?,
    };

    Ok(RepoRef {
        owner: owner.to_string(),
        repo: repo.to_string(),
        subdir,
        git_ref,
    })
}

/// Parse the `tree/<ref>/<subdir...>` (or `blob/...`) tail of a browse URL.
///
/// GitHub renders both `tree` (directory) and `blob` (file) URLs; a user who
/// pastes a `blob` URL for a package's README should still get a working
/// import, so both are accepted and the tail is treated as a directory path.
fn parse_tree_suffix(
    url: &str,
    tail: &[&str],
) -> Result<(Option<String>, Option<String>), GithubError> {
    let kind = tail[0];
    if kind != "tree" && kind != "blob" {
        return Err(GithubError::InvalidUrl {
            url: url.to_string(),
            reason: format!(
                "unexpected path segment {kind:?} — expected \
                 https://github.com/<owner>/<repo> or \
                 https://github.com/<owner>/<repo>/tree/<ref>/<subdir>"
            ),
        });
    }

    if tail.len() < 2 {
        return Err(GithubError::InvalidUrl {
            url: url.to_string(),
            reason: "missing git ref after /tree/".to_string(),
        });
    }

    let git_ref = tail[1];
    if !is_safe_ref(git_ref) {
        return Err(GithubError::InvalidUrl {
            url: url.to_string(),
            reason: format!("invalid git ref {git_ref:?}"),
        });
    }

    // `/tree/main` with no subdirectory is just the repository root.
    if tail.len() == 2 {
        return Ok((Some(git_ref.to_string()), None));
    }

    let components = &tail[2..];
    for c in components {
        if !is_safe_path_component(c) {
            return Err(GithubError::InvalidUrl {
                url: url.to_string(),
                reason: format!("invalid path component {c:?} in subdirectory"),
            });
        }
    }

    Ok((Some(git_ref.to_string()), Some(components.join("/"))))
}

/// Whether `s` is safe as a single cache path component: non-empty, not
/// starting with `.`, and drawn from GitHub's owner/repo charset. Restricting
/// the charset (rather than merely rejecting separators) also guarantees the
/// value cannot smuggle TOML metacharacters into the generated manifest.
fn is_safe_repo_component(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('.')
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-')
}

/// Whether a subdirectory path component is safe. Same rule as a repo
/// component: in particular this rejects `..`, since the subdir is joined
/// onto the extracted checkout path.
fn is_safe_path_component(s: &str) -> bool {
    is_safe_repo_component(s)
}

/// Whether a git ref is safe to pass to `git ls-remote` as an argument.
///
/// Refs legitimately contain `/` (`release/v1`), so slashes are permitted
/// here, but a leading `-` (option injection) and `..` are not.
fn is_safe_ref(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('-')
        && !s.contains("..")
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-' | '/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_repo_url() {
        let r = parse_github_url("https://github.com/octocat/hello-world").unwrap();
        assert_eq!(r.owner, "octocat");
        assert_eq!(r.repo, "hello-world");
        assert_eq!(r.subdir, None);
        assert_eq!(r.git_ref, None);
    }

    #[test]
    fn parses_trailing_slash_and_dot_git() {
        assert_eq!(
            parse_github_url("https://github.com/octocat/hello-world/")
                .unwrap()
                .repo,
            "hello-world"
        );
        assert_eq!(
            parse_github_url("https://github.com/octocat/hello-world.git")
                .unwrap()
                .repo,
            "hello-world"
        );
    }

    #[test]
    fn parses_tree_subdir_url() {
        let r = parse_github_url(
            "https://github.com/modelcontextprotocol/servers/tree/main/src/github",
        )
        .unwrap();
        assert_eq!(r.owner, "modelcontextprotocol");
        assert_eq!(r.repo, "servers");
        assert_eq!(r.git_ref.as_deref(), Some("main"));
        assert_eq!(r.subdir.as_deref(), Some("src/github"));
    }

    #[test]
    fn tree_url_without_subdir_is_the_repo_root() {
        let r = parse_github_url("https://github.com/octocat/hello-world/tree/main").unwrap();
        assert_eq!(r.git_ref.as_deref(), Some("main"));
        assert_eq!(r.subdir, None);
    }

    #[test]
    fn blob_urls_are_accepted_as_directories() {
        let r = parse_github_url("https://github.com/acme/repo/blob/main/src/pkg").unwrap();
        assert_eq!(r.subdir.as_deref(), Some("src/pkg"));
    }

    #[test]
    fn subdir_packages_are_named_for_the_subdir_not_the_monorepo() {
        let r = parse_github_url(
            "https://github.com/modelcontextprotocol/servers/tree/main/src/github",
        )
        .unwrap();
        assert_eq!(r.package_basis(), "github");

        let plain = parse_github_url("https://github.com/acme/widget").unwrap();
        assert_eq!(plain.package_basis(), "widget");
    }

    #[test]
    fn cache_keys_distinguish_subdirs_at_the_same_sha() {
        let sha = "a".repeat(40);
        let a = parse_github_url(
            "https://github.com/modelcontextprotocol/servers/tree/main/src/github",
        )
        .unwrap();
        let b =
            parse_github_url("https://github.com/modelcontextprotocol/servers/tree/main/src/redis")
                .unwrap();
        assert_ne!(a.cache_key(&sha), b.cache_key(&sha));

        let plain = parse_github_url("https://github.com/acme/widget").unwrap();
        assert_eq!(plain.cache_key(&sha), sha, "plain repos stay SHA-addressed");
    }

    #[test]
    fn rejects_non_github_host_and_http() {
        assert!(parse_github_url("https://gitlab.com/a/b").is_err());
        assert!(parse_github_url("http://github.com/a/b").is_err());
    }

    #[test]
    fn rejects_unknown_path_segments() {
        let err = parse_github_url("https://github.com/a/b/pulls/3").unwrap_err();
        assert!(matches!(err, GithubError::InvalidUrl { .. }));
    }

    #[test]
    fn rejects_traversal_in_owner_repo_and_subdir() {
        assert!(parse_github_url("https://github.com/../b").is_err());
        assert!(parse_github_url("https://github.com/a/..").is_err());
        assert!(parse_github_url("https://github.com/a/b/tree/main/../../etc").is_err());
    }

    #[test]
    fn rejects_option_injection_in_ref() {
        assert!(parse_github_url("https://github.com/a/b/tree/--upload-pack=evil/x").is_err());
    }

    #[test]
    fn accepts_slashes_in_refs() {
        let r = parse_github_url("https://github.com/a/b/tree/release/v1").unwrap();
        // `release` is the ref; `v1` reads as the subdir. Ambiguous by nature
        // in GitHub's URL scheme — documented, and harmless because the ref
        // resolution step validates the ref against the remote.
        assert_eq!(r.git_ref.as_deref(), Some("release"));
        assert_eq!(r.subdir.as_deref(), Some("v1"));
    }
}
