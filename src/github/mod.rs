mod graphql;
mod rest;

use std::collections::HashMap;

use anyhow::{Context, bail};
use futures::future::join_all;
use reqwest::{
    Client, StatusCode,
    header::{
        ACCEPT, AUTHORIZATION, ETAG, HeaderMap, HeaderValue, IF_MODIFIED_SINCE, IF_NONE_MATCH,
        LAST_MODIFIED, USER_AGENT,
    },
};
use serde_json::{Value, json};
use url::Url;

use crate::{
    cache::{CacheConfig, CacheMode, CacheStore, CachedResponse},
    cli::{ResolvedTarget, ResourceKind},
    model::{
        Actor, BranchRef, ChangedFile, CheckStatus, Comment, CommitAuthor, CommitSummary,
        ExportDocument, Label, MetadataField, Milestone, RawPayload, Reaction, Review,
        ReviewComment, ReviewThread, TimelineEntry,
    },
};

use self::{
    graphql::{DiscussionFetcher, IssueGraphQlFetcher, PullRequestGraphQlFetcher},
    rest::{
        RestActor, RestCheckRunsResponse, RestCombinedStatus, RestCommit, RestIssue,
        RestIssueComment, RestMilestone, RestPullRequest, RestPullRequestComment,
        RestPullRequestFile, RestPullRequestReview, RestReaction, RestReactions, RestTimelineEvent,
    },
};

const GITHUB_API_VERSION: &str = "2026-03-10";

#[derive(Clone, Debug)]
pub struct GitHubConfig {
    pub api_base_url: String,
    pub graphql_url: String,
    pub user_agent: String,
    pub cache: CacheConfig,
}

#[derive(Clone)]
pub struct GitHubClient {
    client: Client,
    rest_base_url: Url,
    graphql_url: Url,
    token_available: bool,
    cache: CacheStore,
}

impl GitHubClient {
    pub fn new(config: GitHubConfig) -> anyhow::Result<Self> {
        Self::with_token(config, std::env::var("GITHUB_TOKEN").ok())
    }

    fn with_token(config: GitHubConfig, token: Option<String>) -> anyhow::Result<Self> {
        let token = token.filter(|token| !token.trim().is_empty());
        let mut headers = HeaderMap::new();
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/vnd.github+json"),
        );
        headers.insert(
            "X-GitHub-Api-Version",
            HeaderValue::from_static(GITHUB_API_VERSION),
        );
        headers.insert(
            USER_AGENT,
            HeaderValue::from_str(&config.user_agent).context("invalid user-agent header")?,
        );

        if let Some(token) = token.as_ref() {
            let value = format!("Bearer {token}");
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&value).context("invalid authorization header")?,
            );
        }

        let client = Client::builder()
            .default_headers(headers)
            .build()
            .context("failed to build HTTP client")?;

        Ok(Self {
            client,
            rest_base_url: {
                let mut base = config.api_base_url.clone();
                if !base.ends_with('/') {
                    base.push('/');
                }
                Url::parse(&base).context("invalid GitHub REST API base URL")?
            },
            graphql_url: Url::parse(&config.graphql_url)
                .context("invalid GitHub GraphQL API URL")?,
            token_available: token.is_some(),
            cache: CacheStore::new(config.cache, token.as_deref()),
        })
    }

    pub async fn fetch_document(&self, target: &ResolvedTarget) -> anyhow::Result<ExportDocument> {
        match target.kind {
            ResourceKind::Issue => self.fetch_issue(target).await,
            ResourceKind::PullRequest => self.fetch_pull_request(target).await,
            ResourceKind::Discussion => {
                if !self.token_available {
                    bail!(
                        "discussion export requires GITHUB_TOKEN because GitHub Discussions are accessed via GraphQL"
                    );
                }
                self.fetch_discussion(target).await
            }
        }
    }

    async fn fetch_issue(&self, target: &ResolvedTarget) -> anyhow::Result<ExportDocument> {
        if self.token_available {
            let fetcher = IssueGraphQlFetcher::new(self);
            match fetcher
                .fetch_issue(&target.owner, &target.repo, target.number)
                .await
            {
                Ok(mut document) => {
                    match self
                        .get_rest_paginated_with_urls::<RestTimelineEvent>(&format!(
                            "repos/{}/{}/issues/{}/timeline",
                            target.owner, target.repo, target.number
                        ))
                        .await
                    {
                        Ok((timeline, timeline_request_urls)) => {
                            document.timeline = timeline.iter().map(to_timeline_entry).collect();
                            document.raw_payloads.push(RawPayload {
                                name: "rest.issue_timeline".to_owned(),
                                payload: serde_json::to_value(&timeline)?,
                                request_urls: timeline_request_urls,
                                graphql_requests: Vec::new(),
                            });
                        }
                        Err(error) => {
                            document.metadata.push(MetadataField {
                                name: "Timeline enrichment".to_owned(),
                                value: format!(
                                    "issue #{} timeline skipped: {error:#}",
                                    target.number
                                ),
                            });
                        }
                    }

                    return Ok(document);
                }
                Err(error) => {
                    // On pourrait logguer l'erreur ici, mais on retombe sur le mode REST.
                    eprintln!(
                        "warning: GraphQL issue fetch failed, falling back to REST: {error:#}"
                    );
                }
            }
        }

        let (issue, issue_request_url): (RestIssue, String) = self
            .get_rest_with_url(&format!(
                "repos/{}/{}/issues/{}",
                target.owner, target.repo, target.number
            ))
            .await?;
        let (raw_comments, issue_comment_request_urls): (Vec<RestIssueComment>, Vec<String>) = self
            .get_rest_paginated_with_urls(&format!(
                "repos/{}/{}/issues/{}/comments",
                target.owner, target.repo, target.number
            ))
            .await?;
        let (timeline, timeline_request_urls): (Vec<RestTimelineEvent>, Vec<String>) = self
            .get_rest_paginated_with_urls(&format!(
                "repos/{}/{}/issues/{}/timeline",
                target.owner, target.repo, target.number
            ))
            .await?;
        let (comments, comment_warnings, issue_comment_reaction_urls) = self
            .build_issue_comments(&target.owner, &target.repo, &raw_comments)
            .await;

        let mut document = ExportDocument {
            kind: ResourceKind::Issue,
            owner: target.owner.clone(),
            repo: target.repo.clone(),
            number: target.number,
            title: issue.title.clone(),
            url: issue.html_url.clone(),
            state: issue.state.to_lowercase(),
            body: issue.body.clone().unwrap_or_default(),
            author: issue.user.as_ref().map(to_actor),
            author_association: issue.author_association.clone(),
            created_at: Some(issue.created_at),
            updated_at: Some(issue.updated_at),
            closed_at: issue.closed_at,
            state_reason: issue.state_reason.clone(),
            locked: issue.locked,
            active_lock_reason: issue.active_lock_reason.clone(),
            labels: issue.labels.iter().map(to_label).collect(),
            assignees: issue.assignees.iter().map(to_actor).collect(),
            reactions: to_reactions(issue.reactions.as_ref()),
            milestone: issue.milestone.as_ref().map(to_milestone),
            metadata: issue_metadata(&issue),
            comments,
            timeline: timeline.iter().map(to_timeline_entry).collect(),
            ..ExportDocument::default()
        };
        document.metadata.extend(comment_warnings);

        match self
            .fetch_issue_reactions(&target.owner, &target.repo, target.number)
            .await
        {
            Ok((reactions, _urls)) => {
                document.reactions = reactions;
            }
            Err(error) => document.metadata.push(MetadataField {
                name: "Reaction enrichment".to_owned(),
                value: format!(
                    "issue #{} top-level reactions skipped: {error:#}",
                    target.number
                ),
            }),
        }

        document.raw_payloads.extend([
            RawPayload {
                name: "rest.issue".to_owned(),
                payload: serde_json::to_value(&issue)?,
                request_urls: vec![issue_request_url],
                graphql_requests: Vec::new(),
            },
            RawPayload {
                name: "rest.issue_comments".to_owned(),
                payload: serde_json::to_value(&raw_comments)?,
                request_urls: [issue_comment_request_urls, issue_comment_reaction_urls].concat(),
                graphql_requests: Vec::new(),
            },
            RawPayload {
                name: "rest.issue_timeline".to_owned(),
                payload: serde_json::to_value(&timeline)?,
                request_urls: timeline_request_urls,
                graphql_requests: Vec::new(),
            },
        ]);

        Ok(document)
    }

    async fn fetch_pull_request(&self, target: &ResolvedTarget) -> anyhow::Result<ExportDocument> {
        if self.token_available {
            let fetcher = PullRequestGraphQlFetcher::new(self);
            match fetcher
                .fetch_pull_request(&target.owner, &target.repo, target.number)
                .await
            {
                Ok(mut document) => {
                    match self
                        .get_rest_paginated_with_urls::<RestTimelineEvent>(&format!(
                            "repos/{}/{}/issues/{}/timeline",
                            target.owner, target.repo, target.number
                        ))
                        .await
                    {
                        Ok((timeline, timeline_request_urls)) => {
                            let mut enriched_timeline =
                                timeline.iter().map(to_timeline_entry).collect::<Vec<_>>();
                            let _ = self
                                .enrich_timeline(
                                    &mut enriched_timeline,
                                    &target.owner,
                                    &target.repo,
                                )
                                .await;
                            document.timeline = enriched_timeline;
                            document.raw_payloads.push(RawPayload {
                                name: "rest.pull_request_timeline".to_owned(),
                                payload: serde_json::to_value(&timeline)?,
                                request_urls: timeline_request_urls,
                                graphql_requests: Vec::new(),
                            });
                        }
                        Err(error) => {
                            document.metadata.push(MetadataField {
                                name: "Timeline enrichment".to_owned(),
                                value: format!(
                                    "pull request #{} timeline skipped: {error:#}",
                                    target.number
                                ),
                            });
                        }
                    }

                    let _ = self
                        .enrich_commits(&mut document.commits, &target.owner, &target.repo)
                        .await;

                    if let Some(head_sha) = document
                        .head
                        .as_ref()
                        .and_then(|head| head.sha.as_ref())
                        .cloned()
                    {
                        match self
                            .fetch_pull_request_checks(&target.owner, &target.repo, &head_sha)
                            .await
                        {
                            Ok((checks, check_payloads)) => {
                                document.checks = checks;
                                document.raw_payloads.extend(check_payloads);
                            }
                            Err(error) => document.metadata.push(MetadataField {
                                name: "Checks enrichment".to_owned(),
                                value: format!(
                                    "pull request #{} checks skipped: {error:#}",
                                    target.number
                                ),
                            }),
                        }
                    }

                    return Ok(document);
                }
                Err(error) => {
                    eprintln!(
                        "warning: GraphQL pull request fetch failed, falling back to REST: {error:#}"
                    );
                }
            }
        }

        let (pull_request, pull_request_request_url): (RestPullRequest, String) = self
            .get_rest_with_url(&format!(
                "repos/{}/{}/pulls/{}",
                target.owner, target.repo, target.number
            ))
            .await?;
        let (issue, issue_request_url): (RestIssue, String) = self
            .get_rest_with_url(&format!(
                "repos/{}/{}/issues/{}",
                target.owner, target.repo, target.number
            ))
            .await?;
        let (raw_issue_comments, issue_comment_request_urls): (Vec<RestIssueComment>, Vec<String>) =
            self.get_rest_paginated_with_urls(&format!(
                "repos/{}/{}/issues/{}/comments",
                target.owner, target.repo, target.number
            ))
            .await?;
        let (timeline, timeline_request_urls): (Vec<RestTimelineEvent>, Vec<String>) = self
            .get_rest_paginated_with_urls(&format!(
                "repos/{}/{}/issues/{}/timeline",
                target.owner, target.repo, target.number
            ))
            .await?;
        let (reviews, review_request_urls): (Vec<RestPullRequestReview>, Vec<String>) = self
            .get_rest_paginated_with_urls(&format!(
                "repos/{}/{}/pulls/{}/reviews",
                target.owner, target.repo, target.number
            ))
            .await?;
        let (raw_review_comments, review_comment_request_urls): (
            Vec<RestPullRequestComment>,
            Vec<String>,
        ) = self
            .get_rest_paginated_with_urls(&format!(
                "repos/{}/{}/pulls/{}/comments",
                target.owner, target.repo, target.number
            ))
            .await?;
        let (files, file_request_urls): (Vec<RestPullRequestFile>, Vec<String>) = self
            .get_rest_paginated_with_urls(&format!(
                "repos/{}/{}/pulls/{}/files",
                target.owner, target.repo, target.number
            ))
            .await?;
        let (commits, commit_request_urls): (Vec<RestCommit>, Vec<String>) = self
            .get_rest_paginated_with_urls(&format!(
                "repos/{}/{}/pulls/{}/commits",
                target.owner, target.repo, target.number
            ))
            .await?;
        let (issue_comments, issue_comment_warnings, issue_comment_reaction_urls) = self
            .build_issue_comments(&target.owner, &target.repo, &raw_issue_comments)
            .await;
        let (rendered_reviews, review_warnings, review_reaction_urls) = self
            .build_reviews(&target.owner, &target.repo, target.number, &reviews)
            .await;
        let (review_threads, review_comment_warnings, review_comment_reaction_urls) = self
            .build_review_threads(&target.owner, &target.repo, &raw_review_comments)
            .await;

        let mut document = ExportDocument {
            kind: ResourceKind::PullRequest,
            owner: target.owner.clone(),
            repo: target.repo.clone(),
            number: target.number,
            title: pull_request.title.clone(),
            url: pull_request.html_url.clone(),
            state: pull_request.state.to_lowercase(),
            body: pull_request.body.clone().unwrap_or_default(),
            draft: pull_request.draft,
            merged_by: pull_request.merged_by.as_ref().map(to_actor),
            merge_commit_sha: pull_request.merge_commit_sha.clone(),
            mergeable_state: pull_request
                .mergeable_state
                .as_deref()
                .map(|s| s.to_lowercase()),
            base: Some(BranchRef {
                ref_name: pull_request.base.ref_field.clone(),
                sha: Some(pull_request.base.sha.clone()),
                repo_full_name: pull_request.base.repo.as_ref().map(|r| r.full_name.clone()),
            }),
            head: Some(BranchRef {
                ref_name: pull_request.head.ref_field.clone(),
                sha: Some(pull_request.head.sha.clone()),
                repo_full_name: pull_request.head.repo.as_ref().map(|r| r.full_name.clone()),
            }),
            author: pull_request.user.as_ref().map(to_actor),
            author_association: issue.author_association.clone(),
            created_at: Some(pull_request.created_at),
            updated_at: Some(pull_request.updated_at),
            closed_at: pull_request.closed_at,
            state_reason: issue.state_reason.clone(),
            locked: issue.locked,
            active_lock_reason: issue.active_lock_reason.clone(),
            labels: issue.labels.iter().map(to_label).collect(),
            assignees: issue.assignees.iter().map(to_actor).collect(),
            requested_reviewers: pull_request
                .requested_reviewers
                .iter()
                .map(to_actor)
                .collect(),
            reactions: to_reactions(issue.reactions.as_ref()),
            milestone: issue.milestone.as_ref().map(to_milestone),
            metadata: pull_request_metadata(&pull_request, &issue),
            comments: issue_comments,
            reviews: rendered_reviews,
            review_threads,
            timeline: timeline.iter().map(to_timeline_entry).collect(),
            files: files.iter().map(to_changed_file).collect(),
            commits: commits.iter().map(to_commit_summary).collect(),
            ..ExportDocument::default()
        };

        match self
            .fetch_pull_request_checks(&target.owner, &target.repo, &pull_request.head.sha)
            .await
        {
            Ok((checks, check_payloads)) => {
                document.checks = checks;
                document.raw_payloads.extend(check_payloads);
            }
            Err(error) => document.metadata.push(MetadataField {
                name: "Checks enrichment".to_owned(),
                value: format!("pull request #{} checks skipped: {error:#}", target.number),
            }),
        }

        document.metadata.extend(issue_comment_warnings);
        document.metadata.extend(review_warnings);
        let _ = self
            .enrich_timeline(&mut document.timeline, &target.owner, &target.repo)
            .await;
        let _ = self
            .enrich_commits(&mut document.commits, &target.owner, &target.repo)
            .await;
        let mut using_rest_review_threads = true;

        match self
            .fetch_issue_reactions(&target.owner, &target.repo, target.number)
            .await
        {
            Ok((reactions, _urls)) => {
                document.reactions = reactions;
            }
            Err(error) => document.metadata.push(MetadataField {
                name: "Reaction enrichment".to_owned(),
                value: format!(
                    "pull request #{} top-level reactions skipped: {error:#}",
                    target.number
                ),
            }),
        }

        if self.token_available {
            let graph = PullRequestGraphQlFetcher::new(self);
            match graph
                .fetch_review_threads(&target.owner, &target.repo, target.number)
                .await
            {
                Ok((threads, raw_payload, request_urls, graphql_requests)) => {
                    if !threads.is_empty() {
                        document.review_threads = threads;
                        using_rest_review_threads = false;
                    }
                    document.raw_payloads.push(RawPayload {
                        name: "graphql.pull_request_review_threads".to_owned(),
                        payload: raw_payload,
                        request_urls,
                        graphql_requests,
                    });
                }
                Err(error) => {
                    document.metadata.push(MetadataField {
                        name: "GraphQL enrichment".to_owned(),
                        value: format!("review thread enrichment skipped: {error:#}"),
                    });
                }
            }
        }

        if using_rest_review_threads {
            document.metadata.extend(review_comment_warnings);
        }

        document.raw_payloads.extend([
            RawPayload {
                name: "rest.pull_request".to_owned(),
                payload: serde_json::to_value(&pull_request)?,
                request_urls: vec![pull_request_request_url],
                graphql_requests: Vec::new(),
            },
            RawPayload {
                name: "rest.pull_request_issue".to_owned(),
                payload: serde_json::to_value(&issue)?,
                request_urls: vec![issue_request_url],
                graphql_requests: Vec::new(),
            },
            RawPayload {
                name: "rest.pull_request_issue_comments".to_owned(),
                payload: serde_json::to_value(&raw_issue_comments)?,
                request_urls: [issue_comment_request_urls, issue_comment_reaction_urls].concat(),
                graphql_requests: Vec::new(),
            },
            RawPayload {
                name: "rest.pull_request_timeline".to_owned(),
                payload: serde_json::to_value(&timeline)?,
                request_urls: timeline_request_urls,
                graphql_requests: Vec::new(),
            },
            RawPayload {
                name: "rest.pull_request_reviews".to_owned(),
                payload: serde_json::to_value(&reviews)?,
                request_urls: [review_request_urls, review_reaction_urls].concat(),
                graphql_requests: Vec::new(),
            },
            RawPayload {
                name: "rest.pull_request_review_comments".to_owned(),
                payload: serde_json::to_value(&raw_review_comments)?,
                request_urls: [review_comment_request_urls, review_comment_reaction_urls].concat(),
                graphql_requests: Vec::new(),
            },
            RawPayload {
                name: "rest.pull_request_files".to_owned(),
                payload: serde_json::to_value(&files)?,
                request_urls: file_request_urls,
                graphql_requests: Vec::new(),
            },
            RawPayload {
                name: "rest.pull_request_commits".to_owned(),
                payload: serde_json::to_value(&commits)?,
                request_urls: commit_request_urls,
                graphql_requests: Vec::new(),
            },
        ]);

        Ok(document)
    }

    async fn fetch_discussion(&self, target: &ResolvedTarget) -> anyhow::Result<ExportDocument> {
        let fetcher = DiscussionFetcher::new(self);
        let document = fetcher
            .fetch_discussion(&target.owner, &target.repo, target.number)
            .await?;

        Ok(document)
    }

    async fn get_rest_with_url<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
    ) -> anyhow::Result<(T, String)> {
        let url = self.rest_url(path, None);
        let url_string = url.to_string();
        let cache_key = self.cache.key("GET", &url_string, GITHUB_API_VERSION);

        if self.cache.mode() == CacheMode::Offline {
            let payload = self.cache.require_cached(&cache_key, &url_string)?;
            return Ok((payload, url_string));
        }

        let cached = if self.cache.mode() == CacheMode::Refresh {
            None
        } else {
            self.cache.read(&cache_key)?
        };

        let mut request = self.client.get(url.clone());
        if let Some(cached) = cached.as_ref() {
            if let Some(etag) = cached.etag.as_ref() {
                request = request.header(IF_NONE_MATCH, etag);
            } else if let Some(last_modified) = cached.last_modified.as_ref() {
                request = request.header(IF_MODIFIED_SINCE, last_modified);
            }
        }

        let response = request
            .send()
            .await
            .with_context(|| format!("REST request failed for {url}"))?;

        if response.status() == StatusCode::NOT_MODIFIED {
            let cached = cached.context("REST response returned 304 without a cached body")?;
            let payload = serde_json::from_value(cached.body)
                .with_context(|| format!("failed to decode cached REST response for {url}"))?;
            return Ok((payload, url_string));
        }

        let response = response
            .error_for_status()
            .with_context(|| format!("REST request returned an error for {url}"))?;
        let headers = response.headers().clone();
        let body: Value = response
            .json()
            .await
            .with_context(|| format!("failed to decode REST response for {url}"))?;
        self.cache.write(
            &cache_key,
            &CachedResponse::new(
                header_to_string(&headers, ETAG.as_str()),
                header_to_string(&headers, LAST_MODIFIED.as_str()),
                body.clone(),
            ),
        )?;
        let payload = serde_json::from_value(body)
            .with_context(|| format!("failed to decode REST response for {url}"))?;

        Ok((payload, url_string))
    }

    pub(crate) async fn get_rest_paginated_with_urls<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
    ) -> anyhow::Result<(Vec<T>, Vec<String>)> {
        let mut page = 1_u32;
        let mut out = Vec::new();
        let mut request_urls = Vec::new();

        loop {
            let url = self.rest_url(path, Some(page));
            let url_string = url.to_string();
            request_urls.push(url_string.clone());
            let cache_key = self.cache.key("GET", &url_string, GITHUB_API_VERSION);

            if self.cache.mode() == CacheMode::Offline {
                let batch: Vec<T> = self.cache.require_cached(&cache_key, &url_string)?;
                let batch_len = batch.len();
                out.extend(batch);

                if batch_len < 100 {
                    break;
                }

                page += 1;
                continue;
            }

            let cached = if self.cache.mode() == CacheMode::Refresh {
                None
            } else {
                self.cache.read(&cache_key)?
            };

            let mut request = self.client.get(url.clone());
            if let Some(cached) = cached.as_ref() {
                if let Some(etag) = cached.etag.as_ref() {
                    request = request.header(IF_NONE_MATCH, etag);
                } else if let Some(last_modified) = cached.last_modified.as_ref() {
                    request = request.header(IF_MODIFIED_SINCE, last_modified);
                }
            }

            let response = request
                .send()
                .await
                .with_context(|| format!("REST request failed for {url}"))?;

            if response.status() == StatusCode::NOT_MODIFIED {
                let cached = cached.context("REST response returned 304 without a cached body")?;
                let batch: Vec<T> = serde_json::from_value(cached.body)
                    .with_context(|| format!("failed to decode cached REST response for {url}"))?;
                let batch_len = batch.len();
                out.extend(batch);

                if batch_len < 100 {
                    break;
                }

                page += 1;
                continue;
            }

            let response = response
                .error_for_status()
                .with_context(|| format!("REST request returned an error for {url}"))?;
            let headers = response.headers().clone();
            let body: Value = response
                .json()
                .await
                .with_context(|| format!("failed to decode REST response for {url}"))?;
            self.cache.write(
                &cache_key,
                &CachedResponse::new(
                    header_to_string(&headers, ETAG.as_str()),
                    header_to_string(&headers, LAST_MODIFIED.as_str()),
                    body.clone(),
                ),
            )?;
            let batch: Vec<T> = serde_json::from_value(body)
                .with_context(|| format!("failed to decode REST response for {url}"))?;

            let batch_len = batch.len();
            out.extend(batch);

            if batch_len < 100 {
                break;
            }

            page += 1;
        }

        Ok((out, request_urls))
    }

    async fn enrich_timeline(
        &self,
        timeline: &mut [TimelineEntry],
        owner: &str,
        repo: &str,
    ) -> anyhow::Result<()> {
        let mut futures = Vec::new();

        for (i, entry) in timeline.iter().enumerate() {
            if entry.event_type == "committed"
                && let Some(sha) = entry
                    .details
                    .iter()
                    .find(|d| d.name == "Commit")
                    .map(|d| &d.value)
            {
                let url = format!("repos/{owner}/{repo}/commits/{sha}");
                futures.push(async move {
                    let result = self.get_rest_with_url::<RestCommit>(&url).await;
                    (i, result)
                });
            }
        }

        let results = join_all(futures).await;

        for (i, result) in results {
            if let Ok((commit, _)) = result
                && let Some(files) = commit.files
            {
                timeline[i].files = files.iter().map(to_changed_file).collect();
            }
        }

        Ok(())
    }

    async fn enrich_commits(
        &self,
        commits: &mut [CommitSummary],
        owner: &str,
        repo: &str,
    ) -> anyhow::Result<()> {
        let mut futures = Vec::new();

        for (i, commit) in commits.iter().enumerate() {
            let sha = &commit.sha;
            let url = format!("repos/{owner}/{repo}/commits/{sha}");
            futures.push(async move {
                let result = self.get_rest_with_url::<RestCommit>(&url).await;
                (i, result)
            });
        }

        let results = join_all(futures).await;

        for (i, result) in results {
            match result {
                Ok((commit, _)) => {
                    if let Some(files) = commit.files {
                        commits[i].files = files.iter().map(to_changed_file).collect();
                    } else {
                        commits[i].files = Vec::new();
                    }
                }
                Err(e) => {
                    eprintln!("Failed to enrich commit {}: {}", commits[i].sha, e);
                }
            }
        }

        Ok(())
    }

    async fn graphql_query<T: serde::de::DeserializeOwned>(
        &self,
        query: &str,
        variables: Value,
    ) -> anyhow::Result<T> {
        let payload = json!({
            "query": query,
            "variables": variables,
        });
        let payload_string =
            serde_json::to_string(&payload).context("failed to serialize GraphQL request")?;
        let graphql_url = self.graphql_url.to_string();
        let cache_key = self.cache.key("POST", &graphql_url, &payload_string);

        if self.cache.mode() == CacheMode::Offline {
            let Some(cached) = self.cache.read(&cache_key)? else {
                bail!("cache miss for GraphQL request while running with --offline");
            };
            return decode_graphql_body(cached.body);
        }

        if self.cache.mode() == CacheMode::Auto
            && let Some(cached) = self.cache.read(&cache_key)?
        {
            return decode_graphql_body(cached.body);
        }

        let response = self
            .client
            .post(self.graphql_url.clone())
            .json(&payload)
            .send()
            .await
            .context("GraphQL request failed")?;
        let response = response
            .error_for_status()
            .context("GraphQL request returned an error")?;

        let body: Value = response
            .json()
            .await
            .context("failed to decode GraphQL response")?;
        self.cache
            .write(&cache_key, &CachedResponse::new(None, None, body.clone()))?;

        decode_graphql_body(body)
    }

    pub(crate) fn has_token(&self) -> bool {
        self.token_available
    }

    pub(crate) fn graphql_endpoint_url(&self) -> String {
        self.graphql_url.to_string()
    }

    async fn build_issue_comments(
        &self,
        owner: &str,
        repo: &str,
        comments: &[RestIssueComment],
    ) -> (Vec<Comment>, Vec<MetadataField>, Vec<String>) {
        let mut rendered_comments = Vec::with_capacity(comments.len());
        let mut warnings = Vec::new();
        let mut request_urls = Vec::new();

        for comment in comments {
            let mut rendered = to_comment(comment);
            if comment.reactions.is_none() || !rendered.reactions.is_empty() {
                match self
                    .fetch_issue_comment_reactions(owner, repo, comment.id)
                    .await
                {
                    Ok((reactions, urls)) => {
                        request_urls.extend(urls);
                        rendered.reactions = reactions;
                    }
                    Err(error) => warnings.push(MetadataField {
                        name: "Reaction enrichment".to_owned(),
                        value: format!("issue comment {} reactions skipped: {error:#}", comment.id),
                    }),
                }
            }
            rendered_comments.push(rendered);
        }

        (rendered_comments, warnings, request_urls)
    }

    async fn build_review_threads(
        &self,
        owner: &str,
        repo: &str,
        comments: &[RestPullRequestComment],
    ) -> (Vec<ReviewThread>, Vec<MetadataField>, Vec<String>) {
        let mut rendered_comments = Vec::with_capacity(comments.len());
        let mut warnings = Vec::new();
        let mut request_urls = Vec::new();

        for comment in comments {
            let mut rendered = to_review_comment(comment);
            if comment.reactions.is_none() || !rendered.reactions.is_empty() {
                match self
                    .fetch_review_comment_reactions(owner, repo, comment.id)
                    .await
                {
                    Ok((reactions, urls)) => {
                        request_urls.extend(urls);
                        rendered.reactions = reactions;
                    }
                    Err(error) => warnings.push(MetadataField {
                        name: "Reaction enrichment".to_owned(),
                        value: format!(
                            "review comment {} reactions skipped: {error:#}",
                            comment.id
                        ),
                    }),
                }
            }
            rendered_comments.push(rendered);
        }

        (
            group_review_comments(rendered_comments),
            warnings,
            request_urls,
        )
    }

    async fn build_reviews(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
        reviews: &[RestPullRequestReview],
    ) -> (Vec<Review>, Vec<MetadataField>, Vec<String>) {
        let mut rendered_reviews = Vec::with_capacity(reviews.len());
        let mut warnings = Vec::new();
        let mut request_urls = Vec::new();

        for review in reviews {
            let mut rendered = to_review(review);
            if review.reactions.is_none() || !rendered.reactions.is_empty() {
                match self
                    .fetch_review_reactions(owner, repo, number, review.id)
                    .await
                {
                    Ok((reactions, urls)) => {
                        request_urls.extend(urls);
                        rendered.reactions = reactions;
                    }
                    Err(error) => warnings.push(MetadataField {
                        name: "Reaction enrichment".to_owned(),
                        value: format!("review {} reactions skipped: {error:#}", review.id),
                    }),
                }
            }
            rendered_reviews.push(rendered);
        }

        (rendered_reviews, warnings, request_urls)
    }

    async fn fetch_issue_comment_reactions(
        &self,
        owner: &str,
        repo: &str,
        comment_id: u64,
    ) -> anyhow::Result<(Vec<Reaction>, Vec<String>)> {
        self.fetch_reactions(&format!(
            "repos/{owner}/{repo}/issues/comments/{comment_id}/reactions"
        ))
        .await
    }

    async fn fetch_issue_reactions(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
    ) -> anyhow::Result<(Vec<Reaction>, Vec<String>)> {
        self.fetch_reactions(&format!("repos/{owner}/{repo}/issues/{number}/reactions"))
            .await
    }

    async fn fetch_review_comment_reactions(
        &self,
        owner: &str,
        repo: &str,
        comment_id: u64,
    ) -> anyhow::Result<(Vec<Reaction>, Vec<String>)> {
        self.fetch_reactions(&format!(
            "repos/{owner}/{repo}/pulls/comments/{comment_id}/reactions"
        ))
        .await
    }

    async fn fetch_review_reactions(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
        review_id: u64,
    ) -> anyhow::Result<(Vec<Reaction>, Vec<String>)> {
        self.fetch_reactions(&format!(
            "repos/{owner}/{repo}/pulls/{number}/reviews/{review_id}/reactions"
        ))
        .await
    }

    async fn fetch_reactions(&self, path: &str) -> anyhow::Result<(Vec<Reaction>, Vec<String>)> {
        let (reactions, request_urls): (Vec<RestReaction>, Vec<String>) =
            self.get_rest_paginated_with_urls(path).await?;
        Ok((aggregate_reactions(reactions), request_urls))
    }

    async fn fetch_pull_request_checks(
        &self,
        owner: &str,
        repo: &str,
        head_sha: &str,
    ) -> anyhow::Result<(Vec<CheckStatus>, Vec<RawPayload>)> {
        let status_path = format!("repos/{owner}/{repo}/commits/{head_sha}/status");
        let (combined_status, status_url): (RestCombinedStatus, String) =
            self.get_rest_with_url(&status_path).await?;

        let checks_path = format!("repos/{owner}/{repo}/commits/{head_sha}/check-runs");
        let (check_runs, checks_url): (RestCheckRunsResponse, String) =
            self.get_rest_with_url(&checks_path).await?;

        let mut checks = Vec::new();

        for status in combined_status.statuses.iter() {
            checks.push(CheckStatus {
                kind: "status".to_owned(),
                name: status
                    .context
                    .clone()
                    .unwrap_or_else(|| "status".to_owned()),
                status: status.state.clone(),
                conclusion: None,
                url: status.target_url.clone(),
                description: status.description.clone(),
            });
        }

        for run in check_runs.check_runs.iter() {
            checks.push(CheckStatus {
                kind: "check".to_owned(),
                name: run.name.clone(),
                status: run.status.clone(),
                conclusion: run.conclusion.clone(),
                url: run.details_url.clone().or_else(|| run.html_url.clone()),
                description: run.app.as_ref().map(|app| format!("App: {}", app.name)),
            });
        }

        let payloads = vec![
            RawPayload {
                name: "rest.pull_request_combined_status".to_owned(),
                payload: serde_json::to_value(&combined_status)?,
                request_urls: vec![status_url],
                graphql_requests: Vec::new(),
            },
            RawPayload {
                name: "rest.pull_request_check_runs".to_owned(),
                payload: serde_json::to_value(&check_runs)?,
                request_urls: vec![checks_url],
                graphql_requests: Vec::new(),
            },
        ];

        Ok((checks, payloads))
    }

    fn rest_url(&self, path: &str, page: Option<u32>) -> Url {
        let mut url = self
            .rest_base_url
            .join(path.trim_start_matches('/'))
            .expect("invalid API path");
        if let Some(page) = page {
            url.query_pairs_mut()
                .append_pair("per_page", "100")
                .append_pair("page", &page.to_string());
        }
        url
    }
}

fn header_to_string(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

fn decode_graphql_body<T: serde::de::DeserializeOwned>(body: Value) -> anyhow::Result<T> {
    let body: graphql::GraphQlEnvelope<T> =
        serde_json::from_value(body).context("failed to decode GraphQL response")?;

    if let Some(errors) = body.errors {
        let messages = errors
            .into_iter()
            .map(|error| error.message)
            .collect::<Vec<_>>()
            .join("; ");
        bail!("GraphQL returned errors: {messages}");
    }

    body.data.context("GraphQL response contained no data")
}

fn issue_metadata(issue: &RestIssue) -> Vec<MetadataField> {
    let mut fields = vec![MetadataField {
        name: "Comments".to_owned(),
        value: issue.comments.to_string(),
    }];

    if issue.locked {
        fields.push(MetadataField {
            name: "Locked".to_owned(),
            value: "true".to_owned(),
        });
    }
    if let Some(reason) = issue.active_lock_reason.as_ref() {
        fields.push(MetadataField {
            name: "Lock reason".to_owned(),
            value: reason.clone(),
        });
    }
    if let Some(reason) = issue.state_reason.as_ref() {
        fields.push(MetadataField {
            name: "State reason".to_owned(),
            value: reason.clone(),
        });
    }

    fields
}

fn pull_request_metadata(pull: &RestPullRequest, issue: &RestIssue) -> Vec<MetadataField> {
    let mut fields = issue_metadata(issue);
    fields.extend([
        MetadataField {
            name: "Draft".to_owned(),
            value: pull.draft.to_string(),
        },
        MetadataField {
            name: "Merged".to_owned(),
            value: pull.merged.unwrap_or(false).to_string(),
        },
        MetadataField {
            name: "Mergeable state".to_owned(),
            value: pull
                .mergeable_state
                .as_deref()
                .map(|s| s.to_lowercase())
                .unwrap_or_else(|| "unknown".to_owned()),
        },
        MetadataField {
            name: "Base".to_owned(),
            value: format!(
                "{}:{}",
                pull.base
                    .repo
                    .as_ref()
                    .map(|repo| repo.full_name.clone())
                    .unwrap_or_else(|| "unknown".to_owned()),
                pull.base.ref_field
            ),
        },
        MetadataField {
            name: "Head".to_owned(),
            value: format!(
                "{}:{}",
                pull.head
                    .repo
                    .as_ref()
                    .map(|repo| repo.full_name.clone())
                    .unwrap_or_else(|| "unknown".to_owned()),
                pull.head.ref_field
            ),
        },
        MetadataField {
            name: "Commits".to_owned(),
            value: pull.commits.to_string(),
        },
        MetadataField {
            name: "Changed files".to_owned(),
            value: pull.changed_files.to_string(),
        },
        MetadataField {
            name: "Additions".to_owned(),
            value: pull.additions.to_string(),
        },
        MetadataField {
            name: "Deletions".to_owned(),
            value: pull.deletions.to_string(),
        },
    ]);

    if let Some(merged_at) = pull.merged_at {
        fields.push(MetadataField {
            name: "Merged at".to_owned(),
            value: merged_at.to_rfc3339(),
        });
    }
    if let Some(actor) = pull.merged_by.as_ref() {
        fields.push(MetadataField {
            name: "Merged by".to_owned(),
            value: actor.login.clone(),
        });
    }
    if let Some(sha) = pull.merge_commit_sha.as_ref() {
        fields.push(MetadataField {
            name: "Merge commit".to_owned(),
            value: sha.clone(),
        });
    }

    fields
}

fn to_actor(actor: &RestActor) -> Actor {
    Actor {
        login: actor.login.clone(),
        url: actor.html_url.clone(),
    }
}

fn to_label(label: &rest::RestLabel) -> Label {
    Label {
        name: label.name.clone(),
        color: label.color.clone(),
        description: label.description.clone(),
    }
}

fn to_milestone(milestone: &RestMilestone) -> Milestone {
    Milestone {
        title: milestone.title.clone(),
        state: milestone.state.as_deref().map(|s| s.to_lowercase()),
        due_on: milestone.due_on,
        url: milestone.html_url.clone(),
    }
}

fn to_reactions(reactions: Option<&RestReactions>) -> Vec<Reaction> {
    let Some(reactions) = reactions else {
        return Vec::new();
    };

    [
        ("+1", reactions.plus_one),
        ("-1", reactions.minus_one),
        ("laugh", reactions.laugh),
        ("hooray", reactions.hooray),
        ("confused", reactions.confused),
        ("heart", reactions.heart),
        ("rocket", reactions.rocket),
        ("eyes", reactions.eyes),
    ]
    .into_iter()
    .filter(|(_, count)| *count > 0)
    .map(|(content, count)| Reaction {
        content: content.to_owned(),
        count,
        users: Vec::new(),
    })
    .collect()
}

fn aggregate_reactions(reactions: Vec<RestReaction>) -> Vec<Reaction> {
    let mut grouped_users: HashMap<String, Vec<Actor>> = HashMap::new();
    let mut grouped_counts: HashMap<String, u64> = HashMap::new();

    for reaction in reactions {
        *grouped_counts.entry(reaction.content.clone()).or_insert(0) += 1;

        let entry = grouped_users.entry(reaction.content).or_default();
        if let Some(user) = reaction.user.as_ref().map(to_actor)
            && !entry.iter().any(|existing| existing.login == user.login)
        {
            entry.push(user);
        }
    }

    let mut aggregated = Vec::new();
    for content in [
        "+1", "-1", "laugh", "hooray", "confused", "heart", "rocket", "eyes",
    ] {
        if let Some(count) = grouped_counts.remove(content) {
            let users = grouped_users.remove(content).unwrap_or_default();
            aggregated.push(Reaction {
                content: content.to_owned(),
                count,
                users,
            });
        }
    }

    let mut remaining = grouped_counts.into_iter().collect::<Vec<_>>();
    remaining.sort_by(|left, right| left.0.cmp(&right.0));
    aggregated.extend(remaining.into_iter().map(|(content, count)| {
        let users = grouped_users.remove(&content).unwrap_or_default();
        Reaction {
            content,
            count,
            users,
        }
    }));

    aggregated
}

fn to_comment(comment: &RestIssueComment) -> Comment {
    Comment {
        id: comment.id.to_string(),
        url: comment.html_url.clone(),
        author: comment.user.as_ref().map(to_actor),
        author_association: comment.author_association.clone(),
        body: comment.body.clone().unwrap_or_default(),
        created_at: Some(comment.created_at),
        updated_at: Some(comment.updated_at),
        reactions: to_reactions(comment.reactions.as_ref()),
        ..Comment::default()
    }
}

fn to_review(review: &RestPullRequestReview) -> Review {
    Review {
        id: review.id.to_string(),
        url: review.html_url.clone(),
        author: review.user.as_ref().map(to_actor),
        author_association: review.author_association.clone(),
        state: review.state.to_lowercase(),
        body: review.body.clone().unwrap_or_default(),
        submitted_at: review.submitted_at,
        commit_id: review.commit_id.clone(),
        reactions: to_reactions(review.reactions.as_ref()),
    }
}

fn to_review_comment(comment: &RestPullRequestComment) -> ReviewComment {
    ReviewComment {
        id: comment.id.to_string(),
        url: comment.html_url.clone(),
        author: comment.user.as_ref().map(to_actor),
        author_association: comment.author_association.clone(),
        body: comment.body.clone().unwrap_or_default(),
        created_at: Some(comment.created_at),
        updated_at: Some(comment.updated_at),
        path: Some(comment.path.clone()),
        line: comment.line,
        start_line: comment.start_line,
        diff_hunk: comment.diff_hunk.clone(),
        in_reply_to: comment.in_reply_to_id.map(|value| value.to_string()),
        review_id: comment
            .pull_request_review_id
            .map(|value| value.to_string()),
        is_minimized: None,
        minimized_reason: None,
        reactions: to_reactions(comment.reactions.as_ref()),
    }
}

fn group_review_comments<I>(comments: I) -> Vec<ReviewThread>
where
    I: IntoIterator<Item = ReviewComment>,
{
    let comments: Vec<_> = comments.into_iter().collect();
    let comments_by_id: HashMap<_, _> = comments
        .iter()
        .cloned()
        .map(|comment| (comment.id.clone(), comment))
        .collect();
    let comment_positions: HashMap<_, _> = comments
        .iter()
        .enumerate()
        .map(|(index, comment)| (comment.id.clone(), index))
        .collect();
    let mut thread_comment_ids: HashMap<String, Vec<String>> = HashMap::new();
    let mut thread_order = Vec::new();

    for comment in &comments {
        let thread_id = review_thread_root_id(&comment.id, &comments_by_id);

        if !thread_comment_ids.contains_key(&thread_id) {
            thread_order.push(thread_id.clone());
        }

        thread_comment_ids
            .entry(thread_id)
            .or_default()
            .push(comment.id.clone());
    }

    thread_order
        .into_iter()
        .filter_map(|thread_id| {
            let mut ids = thread_comment_ids.remove(&thread_id)?;
            ids.sort_by_key(|comment_id| {
                (
                    review_comment_depth(comment_id, &comments_by_id),
                    comment_positions
                        .get(comment_id)
                        .copied()
                        .unwrap_or(usize::MAX),
                )
            });

            let grouped_comments = ids
                .into_iter()
                .filter_map(|comment_id| comments_by_id.get(&comment_id).cloned())
                .collect::<Vec<_>>();
            let path = comments_by_id
                .get(&thread_id)
                .and_then(|comment| comment.path.clone())
                .or_else(|| {
                    grouped_comments
                        .iter()
                        .find_map(|comment| comment.path.clone())
                });

            Some(ReviewThread {
                id: thread_id,
                path,
                comments: grouped_comments,
                ..ReviewThread::default()
            })
        })
        .collect()
}

fn review_thread_root_id(
    comment_id: &str,
    comments_by_id: &HashMap<String, ReviewComment>,
) -> String {
    let mut current_id = comment_id;
    let mut seen = std::collections::HashSet::new();

    while seen.insert(current_id.to_owned()) {
        let Some(parent_id) = comments_by_id
            .get(current_id)
            .and_then(|comment| comment.in_reply_to.as_deref())
        else {
            break;
        };

        if !comments_by_id.contains_key(parent_id) {
            break;
        }

        current_id = parent_id;
    }

    current_id.to_owned()
}

fn review_comment_depth(
    comment_id: &str,
    comments_by_id: &HashMap<String, ReviewComment>,
) -> usize {
    let mut depth = 0;
    let mut current_id = comment_id;
    let mut seen = std::collections::HashSet::new();

    while seen.insert(current_id.to_owned()) {
        let Some(parent_id) = comments_by_id
            .get(current_id)
            .and_then(|comment| comment.in_reply_to.as_deref())
        else {
            break;
        };

        if !comments_by_id.contains_key(parent_id) {
            break;
        }

        depth += 1;
        current_id = parent_id;
    }

    depth
}

fn to_timeline_entry(event: &RestTimelineEvent) -> TimelineEntry {
    let mut details = Vec::new();
    let body = event.body.clone().or_else(|| event.message.clone());

    if let Some(state) = event.state.as_ref() {
        details.push(MetadataField {
            name: "State".to_owned(),
            value: state.to_lowercase(),
        });
    }
    if let Some(label) = event.label.as_ref() {
        details.push(MetadataField {
            name: "Label".to_owned(),
            value: label.name.clone(),
        });
    }
    if let Some(commit_id) = event.commit_id.as_ref().or(event.sha.as_ref()) {
        details.push(MetadataField {
            name: "Commit".to_owned(),
            value: commit_id.clone(),
        });
    }
    let mut source_issue = None;
    if let Some(source) = event
        .source
        .as_ref()
        .and_then(|source| source.issue.as_ref())
    {
        source_issue = Some(crate::model::SourceIssue {
            number: source.number,
            title: source.title.clone(),
            url: source.html_url.clone(),
        });
    }

    if let Some(rename) = event.rename.as_ref() {
        details.push(MetadataField {
            name: "From".to_owned(),
            value: rename.from.clone(),
        });
        details.push(MetadataField {
            name: "To".to_owned(),
            value: rename.to.clone(),
        });
    }
    if let Some(review_state) = event.reviewed.as_ref() {
        details.push(MetadataField {
            name: "Review state".to_owned(),
            value: review_state.to_lowercase(),
        });
    }

    TimelineEntry {
        event_type: event.event.clone(),
        actor: event.actor.as_ref().map(to_actor),
        created_at: event
            .created_at
            .or(event.submitted_at)
            .or_else(|| {
                event
                    .author
                    .as_ref()
                    .and_then(|author| author.date.as_ref().cloned())
            })
            .or_else(|| {
                event
                    .committer
                    .as_ref()
                    .and_then(|committer| committer.date.as_ref().cloned())
            }),
        body,
        commit_author: event.author.as_ref().map(|author| CommitAuthor {
            name: author.name.clone(),
            email: author.email.clone(),
        }),
        source_issue,
        details,
        files: Vec::new(),
    }
}

pub(crate) fn to_changed_file(file: &RestPullRequestFile) -> ChangedFile {
    ChangedFile {
        sha: file.sha.clone(),
        path: file.filename.clone(),
        status: file.status.clone(),
        additions: file.additions,
        deletions: file.deletions,
        changes: file.changes,
        previous_path: file.previous_filename.clone(),
        blob_url: file.blob_url.clone(),
        patch: file.patch.clone(),
    }
}

fn to_commit_summary(commit: &RestCommit) -> CommitSummary {
    let authored_at = commit.commit.author.as_ref().and_then(|author| author.date);
    CommitSummary {
        sha: commit.sha.clone(),
        url: commit.html_url.clone(),
        message: commit.commit.message.clone(),
        author_name: commit
            .commit
            .author
            .as_ref()
            .map(|author| author.name.clone()),
        authored_at,
        author_user: commit.author.as_ref().map(to_actor),
        files: commit
            .files
            .as_ref()
            .map(|files| files.iter().map(to_changed_file).collect())
            .unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        cache::{CacheConfig, CacheMode},
        cli::ResourceKind,
        model::{ExportDocument, ReviewComment},
        template,
        testsupport::load_json_fixture,
    };

    use super::{
        GitHubClient, GitHubConfig, RestActor, RestCommit, RestIssue, RestIssueComment,
        RestPullRequest, RestPullRequestComment, RestPullRequestFile, RestPullRequestReview,
        RestReaction, RestTimelineEvent, aggregate_reactions, group_review_comments,
        issue_metadata, pull_request_metadata, to_actor, to_changed_file, to_comment,
        to_commit_summary, to_label, to_milestone, to_reactions, to_review, to_review_comment,
        to_timeline_entry,
    };

    fn test_cache_config() -> CacheConfig {
        CacheConfig {
            mode: CacheMode::Bypass,
            root: std::env::temp_dir().join("ghdump-test-cache"),
            ttl_seconds: 300,
        }
    }

    fn fixture_issue_document() -> ExportDocument {
        let issue: RestIssue = load_json_fixture("issue/rest.issue.json");
        let comments: Vec<RestIssueComment> = load_json_fixture("issue/rest.issue_comments.json");
        let timeline: Vec<RestTimelineEvent> = load_json_fixture("issue/rest.timeline.json");

        ExportDocument {
            kind: ResourceKind::Issue,
            owner: "acme".to_owned(),
            repo: "widgets".to_owned(),
            number: 7,
            title: issue.title.clone(),
            url: issue.html_url.clone(),
            state: issue.state.to_lowercase(),
            body: issue.body.clone().unwrap_or_default(),
            author: issue.user.as_ref().map(to_actor),
            author_association: issue.author_association.clone(),
            created_at: Some(issue.created_at),
            updated_at: Some(issue.updated_at),
            closed_at: issue.closed_at,
            labels: issue.labels.iter().map(to_label).collect(),
            assignees: issue.assignees.iter().map(to_actor).collect(),
            reactions: to_reactions(issue.reactions.as_ref()),
            metadata: issue_metadata(&issue),
            milestone: issue.milestone.as_ref().map(to_milestone),
            comments: comments.iter().map(to_comment).collect(),
            timeline: timeline.iter().map(to_timeline_entry).collect(),
            ..ExportDocument::default()
        }
    }

    fn fixture_pull_request_document() -> ExportDocument {
        let pull_request: RestPullRequest =
            load_json_fixture("pull_request/rest.pull_request.json");
        let issue: RestIssue = load_json_fixture("pull_request/rest.issue.json");
        let issue_comments: Vec<RestIssueComment> =
            load_json_fixture("pull_request/rest.issue_comments.json");
        let timeline: Vec<RestTimelineEvent> = load_json_fixture("pull_request/rest.timeline.json");
        let reviews: Vec<RestPullRequestReview> =
            load_json_fixture("pull_request/rest.reviews.json");
        let review_comments: Vec<RestPullRequestComment> =
            load_json_fixture("pull_request/rest.review_comments.json");
        let files: Vec<RestPullRequestFile> = load_json_fixture("pull_request/rest.files.json");
        let commits: Vec<RestCommit> = load_json_fixture("pull_request/rest.commits.json");

        ExportDocument {
            kind: ResourceKind::PullRequest,
            owner: "acme".to_owned(),
            repo: "widgets".to_owned(),
            number: 11,
            title: pull_request.title.clone(),
            url: pull_request.html_url.clone(),
            state: pull_request.state.to_lowercase(),
            body: pull_request.body.clone().unwrap_or_default(),
            author: pull_request.user.as_ref().map(to_actor),
            author_association: issue.author_association.clone(),
            created_at: Some(pull_request.created_at),
            updated_at: Some(pull_request.updated_at),
            closed_at: pull_request.closed_at,
            labels: issue.labels.iter().map(to_label).collect(),
            assignees: issue.assignees.iter().map(to_actor).collect(),
            requested_reviewers: pull_request
                .requested_reviewers
                .iter()
                .map(to_actor)
                .collect(),
            reactions: to_reactions(issue.reactions.as_ref()),
            metadata: pull_request_metadata(&pull_request, &issue),
            milestone: issue.milestone.as_ref().map(to_milestone),
            comments: issue_comments.iter().map(to_comment).collect(),
            reviews: reviews.iter().map(to_review).collect(),
            review_threads: group_review_comments(review_comments.iter().map(to_review_comment)),
            timeline: timeline.iter().map(to_timeline_entry).collect(),
            files: files.iter().map(to_changed_file).collect(),
            commits: commits.iter().map(to_commit_summary).collect(),
            ..ExportDocument::default()
        }
    }

    #[test]
    fn aggregate_reactions_counts_and_orders_known_reactions() {
        let reactions = vec![
            RestReaction {
                content: "heart".to_owned(),
                user: Some(RestActor {
                    login: "alice".to_owned(),
                    html_url: Some("https://github.com/alice".to_owned()),
                }),
            },
            RestReaction {
                content: "+1".to_owned(),
                user: Some(RestActor {
                    login: "bob".to_owned(),
                    html_url: Some("https://github.com/bob".to_owned()),
                }),
            },
            RestReaction {
                content: "heart".to_owned(),
                user: Some(RestActor {
                    login: "carol".to_owned(),
                    html_url: Some("https://github.com/carol".to_owned()),
                }),
            },
            RestReaction {
                content: "eyes".to_owned(),
                user: Some(RestActor {
                    login: "dave".to_owned(),
                    html_url: Some("https://github.com/dave".to_owned()),
                }),
            },
        ];

        assert_eq!(
            aggregate_reactions(reactions)
                .into_iter()
                .map(|reaction| (reaction.content, reaction.count))
                .collect::<Vec<_>>(),
            vec![
                ("+1".to_owned(), 1),
                ("heart".to_owned(), 2),
                ("eyes".to_owned(), 1),
            ]
        );
    }

    #[test]
    fn aggregate_reactions_preserves_unknown_reactions_after_known_ones() {
        let reactions = vec![
            RestReaction {
                content: "tada".to_owned(),
                user: Some(RestActor {
                    login: "erin".to_owned(),
                    html_url: Some("https://github.com/erin".to_owned()),
                }),
            },
            RestReaction {
                content: "confused".to_owned(),
                user: Some(RestActor {
                    login: "frank".to_owned(),
                    html_url: Some("https://github.com/frank".to_owned()),
                }),
            },
            RestReaction {
                content: "tada".to_owned(),
                user: Some(RestActor {
                    login: "grace".to_owned(),
                    html_url: Some("https://github.com/grace".to_owned()),
                }),
            },
        ];

        assert_eq!(
            aggregate_reactions(reactions)
                .into_iter()
                .map(|reaction| (reaction.content, reaction.count))
                .collect::<Vec<_>>(),
            vec![("confused".to_owned(), 1), ("tada".to_owned(), 2),]
        );
    }

    #[test]
    fn issue_fixture_renders_without_network() {
        let document = fixture_issue_document();

        assert_eq!(document.kind, ResourceKind::Issue);
        assert_eq!(document.comments.len(), 1);
        assert_eq!(document.timeline.len(), 1);
        assert_eq!(document.comments[0].reactions[0].content, "heart");

        let markdown = template::render(&document, None).expect("issue fixture should render");

        assert!(markdown.contains("# Issue #7: Fixture issue title"));
        assert!(markdown.contains("## Milestone"));
        assert!(markdown.contains("- `bug` (`#d73a4a`): Something is broken"));
        assert!(
            markdown
                .contains("### Comment 1 by [issue-commenter](https://github.com/issue-commenter)")
        );
        assert!(markdown.contains("## Timeline (2)"));
        assert!(markdown.contains("- Label: bug"));
    }

    #[test]
    fn committed_timeline_event_uses_sha_message_and_author_date() {
        let event: RestTimelineEvent = serde_json::from_value(serde_json::json!({
            "event": "committed",
            "sha": "1a50928ab60d17848aa8bd427334033c0fac1c5f",
            "message": "gitbutler: migrate from fetcherVersion = 2 to fetcherVersion = 3",
            "author": {
                "name": "Aliaksandr",
                "email": "aliaksandr@example.org",
                "date": "2026-04-27T15:16:12Z"
            }
        }))
        .expect("fixture committed event should deserialize");

        let entry = to_timeline_entry(&event);

        assert_eq!(entry.event_type, "committed");
        assert_eq!(
            entry.created_at.as_ref().map(chrono::DateTime::to_rfc3339),
            Some("2026-04-27T15:16:12+00:00".to_owned())
        );
        assert_eq!(
            entry.body.as_deref(),
            Some("gitbutler: migrate from fetcherVersion = 2 to fetcherVersion = 3")
        );
        assert!(entry.details.iter().any(|field| {
            field.name == "Commit" && field.value == "1a50928ab60d17848aa8bd427334033c0fac1c5f"
        }));
        let commit_author = entry
            .commit_author
            .as_ref()
            .expect("commit author should be present");
        assert_eq!(commit_author.name.as_deref(), Some("Aliaksandr"));
        assert_eq!(
            commit_author.email.as_deref(),
            Some("aliaksandr@example.org")
        );
    }

    #[test]
    fn pull_request_fixture_renders_without_network() {
        let document = fixture_pull_request_document();

        assert_eq!(document.kind, ResourceKind::PullRequest);
        assert_eq!(document.reviews.len(), 1);
        assert_eq!(document.review_threads.len(), 1);
        assert_eq!(document.review_threads[0].comments.len(), 2);
        assert_eq!(document.files.len(), 1);
        assert_eq!(document.commits.len(), 1);

        let markdown =
            template::render(&document, None).expect("pull request fixture should render");

        assert!(markdown.contains("# Pull Request #11: Fixture pull request"));
        assert!(markdown.contains("## Timeline"));
        assert!(markdown.contains(
            "#### Review thread on `src/parser.rs` by [reviewer](https://github.com/reviewer)"
        ));
        assert!(
            markdown.contains("##### Review Comment by [reviewer](https://github.com/reviewer)")
        );
        assert!(markdown.contains("### `src/parser.rs`"));
        assert!(markdown.contains("Support offline fixtures"));
    }

    #[test]
    fn default_template_includes_review_comment_file_stats() {
        let document = fixture_pull_request_document();

        let markdown = template::render(&document, None).expect("default template should render");

        assert!(markdown.contains("<summary>src/parser.rs</summary>"));
    }

    #[test]
    fn groups_review_comments_when_child_precedes_parent() {
        let threads = group_review_comments(vec![
            ReviewComment {
                id: "child".to_owned(),
                in_reply_to: Some("parent".to_owned()),
                path: Some("src/lib.rs".to_owned()),
                body: "reply".to_owned(),
                ..ReviewComment::default()
            },
            ReviewComment {
                id: "parent".to_owned(),
                path: Some("src/lib.rs".to_owned()),
                body: "top-level".to_owned(),
                ..ReviewComment::default()
            },
        ]);

        assert_eq!(threads.len(), 1);
        assert_eq!(threads[0].id, "parent");
        assert_eq!(threads[0].comments.len(), 2);
        assert_eq!(threads[0].comments[0].id, "parent");
        assert_eq!(threads[0].comments[1].id, "child");
        assert_eq!(threads[0].path.as_deref(), Some("src/lib.rs"));
    }

    #[test]
    fn treats_blank_github_token_as_missing() {
        let empty_token_client = GitHubClient::with_token(
            GitHubConfig {
                api_base_url: "https://api.github.com".to_owned(),
                graphql_url: "https://api.github.com/graphql".to_owned(),
                user_agent: "ghdump/test".to_owned(),
                cache: test_cache_config(),
            },
            Some(String::new()),
        )
        .expect("client with empty token should build");

        let whitespace_token_client = GitHubClient::with_token(
            GitHubConfig {
                api_base_url: "https://api.github.com".to_owned(),
                graphql_url: "https://api.github.com/graphql".to_owned(),
                user_agent: "ghdump/test".to_owned(),
                cache: test_cache_config(),
            },
            Some("   ".to_owned()),
        )
        .expect("client with whitespace token should build");

        let valid_token_client = GitHubClient::with_token(
            GitHubConfig {
                api_base_url: "https://api.github.com".to_owned(),
                graphql_url: "https://api.github.com/graphql".to_owned(),
                user_agent: "ghdump/test".to_owned(),
                cache: test_cache_config(),
            },
            Some("fixture-token".to_owned()),
        )
        .expect("client with valid token should build");

        assert!(!empty_token_client.has_token());
        assert!(!whitespace_token_client.has_token());
        assert!(valid_token_client.has_token());
    }
}
