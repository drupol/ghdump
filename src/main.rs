mod cache;
mod cli;
mod github;
mod model;
mod template;
#[cfg(test)]
mod testsupport;

use std::{fs, process::ExitCode};

use anyhow::Context;
use clap::Parser;

use crate::{
    cache::{CacheConfig, CacheMode},
    cli::Cli,
    github::{GitHubClient, GitHubConfig},
};

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let resolved = cli.resolve().context("failed to resolve CLI arguments")?;

    let selected_cache_modes = [cli.no_cache, cli.refresh, cli.offline]
        .into_iter()
        .filter(|enabled| *enabled)
        .count();
    if selected_cache_modes > 1 {
        anyhow::bail!("--no-cache, --refresh, and --offline are mutually exclusive");
    }

    let cache_mode = if cli.no_cache {
        CacheMode::Bypass
    } else if cli.refresh {
        CacheMode::Refresh
    } else if cli.offline {
        CacheMode::Offline
    } else {
        CacheMode::Auto
    };

    let client = GitHubClient::new(GitHubConfig {
        api_base_url: cli.api_base_url.clone(),
        graphql_url: cli.graphql_url.clone(),
        user_agent: cli.user_agent.clone(),
        cache: CacheConfig {
            mode: cache_mode,
            root: cli
                .cache_dir
                .clone()
                .unwrap_or_else(CacheConfig::default_root),
            ttl_seconds: cli.cache_ttl,
        },
    })?;

    let export = client
        .fetch_document(&resolved)
        .await
        .with_context(|| format!("failed to export {}", resolved.display_name()))?;

    if let Some(path) = cli.dump_context.as_ref() {
        let context = template::dump_context(&export)?;
        fs::write(path, context).with_context(|| format!("failed to write {}", path.display()))?;
    }

    let rendered_output = template::render(&export, cli.template.as_deref())?;

    if let Some(path) = cli.output.as_ref() {
        fs::write(path, rendered_output)
            .with_context(|| format!("failed to write {}", path.display()))?;
    } else {
        print!("{rendered_output}");
    }

    Ok(())
}
