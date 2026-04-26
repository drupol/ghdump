use std::{fmt, path::PathBuf, str::FromStr};

use anyhow::{Context, bail};
use clap::{Parser, ValueEnum};
use url::Url;

#[derive(Debug, Parser)]
#[command(
    name = "ghdump",
    version,
    about = env!("CARGO_PKG_DESCRIPTION")
)]
pub struct Cli {
    #[arg(
        value_name = "URL",
        help = "GitHub issue, pull request, or discussion URL"
    )]
    pub target: String,

    #[arg(long, short, help = "Write the rendered output to this file")]
    pub output: Option<PathBuf>,

    #[arg(
        long,
        value_name = "PATH",
        help = "Render the export with a custom MiniJinja template"
    )]
    pub template: Option<PathBuf>,

    #[arg(
        long,
        value_name = "PATH",
        help = "Write the serialized template context JSON to this file",
        help_heading = "Advanced"
    )]
    pub dump_context: Option<PathBuf>,

    #[arg(
        long,
        default_value = "https://api.github.com",
        help = "Base URL for the GitHub REST API",
        help_heading = "Advanced"
    )]
    pub api_base_url: String,

    #[arg(
        long,
        default_value = "https://api.github.com/graphql",
        help = "Endpoint for the GitHub GraphQL API",
        help_heading = "Advanced"
    )]
    pub graphql_url: String,

    #[arg(
        long,
        default_value = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION")),
        help = "User-Agent header sent to GitHub",
        help_heading = "Advanced"
    )]
    pub user_agent: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum ResourceKind {
    Issue,
    PullRequest,
    Discussion,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedTarget {
    pub owner: String,
    pub repo: String,
    pub number: u64,
    pub kind: ResourceKind,
    pub original: String,
}

impl Cli {
    pub fn resolve(&self) -> anyhow::Result<ResolvedTarget> {
        let url = Url::parse(&self.target)
            .with_context(|| format!("TARGET must be a valid GitHub URL, got {}", self.target))?;
        resolve_github_url(&url)
    }
}

impl ResolvedTarget {
    pub fn display_name(&self) -> String {
        format!("{}/{}#{}", self.repo_locator(), self.kind, self.number)
    }

    pub fn repo_locator(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }
}

impl fmt::Display for ResourceKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Issue => f.write_str("issue"),
            Self::PullRequest => f.write_str("pull-request"),
            Self::Discussion => f.write_str("discussion"),
        }
    }
}

fn resolve_github_url(url: &Url) -> anyhow::Result<ResolvedTarget> {
    let host = url.host_str().context("URL has no host")?;
    if host != "github.com" {
        bail!("unsupported host {host}; expected github.com");
    }

    let segments: Vec<_> = url
        .path_segments()
        .context("URL path is not hierarchical")?
        .collect();

    if segments.len() < 4 {
        bail!("URL path is too short to identify a GitHub resource");
    }

    let kind = match segments[2] {
        "issues" => ResourceKind::Issue,
        "pull" => ResourceKind::PullRequest,
        "discussions" => ResourceKind::Discussion,
        other => bail!("unsupported GitHub URL segment {other}"),
    };

    let number = u64::from_str(segments[3])
        .with_context(|| format!("invalid resource number {}", segments[3]))?;

    Ok(ResolvedTarget {
        owner: segments[0].to_owned(),
        repo: segments[1].to_owned(),
        number,
        kind,
        original: url.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_issue_url() {
        let cli = Cli {
            target: "https://github.com/octocat/Hello-World/issues/42".to_owned(),
            output: None,
            template: None,
            dump_context: None,
            api_base_url: "https://api.github.com".to_owned(),
            graphql_url: "https://api.github.com/graphql".to_owned(),
            user_agent: "ghdump/test".to_owned(),
        };

        let target = cli.resolve().expect("URL should parse");

        assert_eq!(target.owner, "octocat");
        assert_eq!(target.repo, "Hello-World");
        assert_eq!(target.number, 42);
        assert_eq!(target.kind, ResourceKind::Issue);
    }

    #[test]
    fn parses_discussion_url() {
        let cli = Cli {
            target: "https://github.com/octocat/Hello-World/discussions/99".to_owned(),
            output: None,
            template: None,
            dump_context: None,
            api_base_url: "https://api.github.com".to_owned(),
            graphql_url: "https://api.github.com/graphql".to_owned(),
            user_agent: "ghdump/test".to_owned(),
        };

        let target = cli.resolve().expect("discussion URL should parse");

        assert_eq!(target.owner, "octocat");
        assert_eq!(target.repo, "Hello-World");
        assert_eq!(target.number, 99);
        assert_eq!(target.kind, ResourceKind::Discussion);
    }

    #[test]
    fn rejects_non_url_target() {
        let cli = Cli {
            target: "octocat/Hello-World".to_owned(),
            output: None,
            template: None,
            dump_context: None,
            api_base_url: "https://api.github.com".to_owned(),
            graphql_url: "https://api.github.com/graphql".to_owned(),
            user_agent: "ghdump/test".to_owned(),
        };

        let error = cli.resolve().expect_err("non-URL target should fail");
        assert!(
            error
                .to_string()
                .contains("TARGET must be a valid GitHub URL")
        );
    }
}
