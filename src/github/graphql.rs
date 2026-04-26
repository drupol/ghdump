use anyhow::{Context, bail};
use chrono::{DateTime, Utc};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::model::{
    Actor, Comment, ExportDocument, Label, MetadataField, RawGraphQlRequest, RawPayload, Reaction,
    ReviewComment, ReviewThread,
};

use super::{GitHubClient, RestPullRequestFile, to_changed_file as to_changed_file_rest};

pub(crate) struct DiscussionFetcher<'a> {
    client: &'a GitHubClient,
}

pub(crate) struct IssueGraphQlFetcher<'a> {
    client: &'a GitHubClient,
}

pub(crate) struct PullRequestGraphQlFetcher<'a> {
    client: &'a GitHubClient,
}

impl<'a> DiscussionFetcher<'a> {
    pub(crate) fn new(client: &'a GitHubClient) -> Self {
        Self { client }
    }

    pub(crate) async fn fetch_discussion(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
    ) -> anyhow::Result<ExportDocument> {
        if !self.client.has_token() {
            bail!("discussion export requires authentication");
        }

        let variables = json!({
            "owner": owner,
            "repo": repo,
            "number": number as i64,
            "cursor": Value::Null,
        });
        let graphql_url = self.client.graphql_endpoint_url();
        let discussion_request_urls = vec![graphql_url.clone()];
        let mut comment_request_urls = vec![graphql_url.clone()];
        let discussion_graphql_requests = vec![graphql_request(DISCUSSION_QUERY, &variables)];
        let mut comment_graphql_requests = vec![graphql_request(DISCUSSION_QUERY, &variables)];
        let data: DiscussionRoot = self
            .client
            .graphql_query(DISCUSSION_QUERY, variables)
            .await?;

        let repository = data.repository.context("repository not found")?;
        let discussion = repository.discussion.context("discussion not found")?;
        let mut comments = discussion.comments.nodes.clone();
        let mut cursor = discussion.comments.page_info.end_cursor.clone();
        let mut has_next_page = discussion.comments.page_info.has_next_page;

        while has_next_page {
            let variables = json!({
                "owner": owner,
                "repo": repo,
                "number": number as i64,
                "cursor": cursor,
            });
            comment_request_urls.push(graphql_url.clone());
            comment_graphql_requests.push(graphql_request(DISCUSSION_COMMENTS_QUERY, &variables));
            let page: DiscussionCommentsOnlyRoot = self
                .client
                .graphql_query(DISCUSSION_COMMENTS_QUERY, variables)
                .await?;

            let repository = page.repository.context("repository not found")?;
            let discussion_page = repository.discussion.context("discussion not found")?;
            has_next_page = discussion_page.comments.page_info.has_next_page;
            cursor = discussion_page.comments.page_info.end_cursor.clone();
            comments.extend(discussion_page.comments.nodes);
        }

        let mut rendered_comments = Vec::with_capacity(comments.len());
        let mut comment_futures = futures::stream::iter(comments)
            .map(|comment| async move { self.expand_discussion_comment(comment).await })
            .buffered(5);

        while let Some(result) = comment_futures.next().await {
            let (comment_item, comment_urls, comment_requests) = result?;
            rendered_comments.push(comment_item);
            comment_request_urls.extend(comment_urls);
            comment_graphql_requests.extend(comment_requests);
        }

        let mut metadata = vec![
            MetadataField {
                name: "Category".to_owned(),
                value: discussion.category.name.clone(),
            },
            MetadataField {
                name: "Category slug".to_owned(),
                value: discussion.category.slug.clone(),
            },
            MetadataField {
                name: "Closed".to_owned(),
                value: discussion.closed.to_string(),
            },
            MetadataField {
                name: "Locked".to_owned(),
                value: discussion.locked.to_string(),
            },
            MetadataField {
                name: "Comment count".to_owned(),
                value: rendered_comments.len().to_string(),
            },
        ];

        if let Some(answer_chosen_at) = discussion.answer_chosen_at.as_ref() {
            metadata.push(MetadataField {
                name: "Answer chosen at".to_owned(),
                value: answer_chosen_at.to_rfc3339(),
            });
        }
        if let Some(answer_chosen_by) = discussion.answer_chosen_by.as_ref() {
            metadata.push(MetadataField {
                name: "Answer chosen by".to_owned(),
                value: answer_chosen_by.login.clone(),
            });
        }

        let raw_payloads = vec![
            RawPayload {
                name: "graphql.discussion".to_owned(),
                payload: serde_json::to_value(&discussion)?,
                request_urls: discussion_request_urls,
                graphql_requests: discussion_graphql_requests,
            },
            RawPayload {
                name: "graphql.discussion_comments".to_owned(),
                payload: serde_json::to_value(rendered_comments_to_raw(&rendered_comments))?,
                request_urls: comment_request_urls,
                graphql_requests: comment_graphql_requests,
            },
        ];

        Ok(ExportDocument {
            kind: crate::cli::ResourceKind::Discussion,
            owner: owner.to_owned(),
            repo: repo.to_owned(),
            number,
            title: discussion.title,
            url: discussion.url,
            state: if discussion.closed {
                "closed".to_owned()
            } else {
                "open".to_owned()
            },
            body: discussion.body,
            author: discussion.author.map(to_actor),
            author_association: discussion.author_association,
            created_at: Some(discussion.created_at),
            updated_at: Some(discussion.updated_at),
            closed_at: discussion.closed_at,
            labels: discussion.labels.nodes.into_iter().map(to_label).collect(),
            reactions: to_reactions(discussion.reaction_groups.as_deref()),
            metadata,
            comments: rendered_comments,
            raw_payloads,
            ..ExportDocument::default()
        })
    }

    async fn expand_discussion_comment(
        &self,
        mut comment: GraphQlDiscussionComment,
    ) -> anyhow::Result<(Comment, Vec<String>, Vec<RawGraphQlRequest>)> {
        let mut request_urls = Vec::new();
        let mut graphql_requests = Vec::new();

        let (mut replies, mut cursor, mut has_next_page) = match comment.replies.take() {
            Some(replies) => (
                replies.nodes,
                replies.page_info.end_cursor,
                replies.page_info.has_next_page,
            ),
            None => {
                let replies = self
                    .fetch_discussion_replies_page(
                        &comment.id,
                        None,
                        &mut request_urls,
                        &mut graphql_requests,
                    )
                    .await?;
                (
                    replies.nodes,
                    replies.page_info.end_cursor,
                    replies.page_info.has_next_page,
                )
            }
        };

        while has_next_page {
            let page = self
                .fetch_discussion_replies_page(
                    &comment.id,
                    cursor.clone(),
                    &mut request_urls,
                    &mut graphql_requests,
                )
                .await?;
            has_next_page = page.page_info.has_next_page;
            cursor = page.page_info.end_cursor;
            replies.extend(page.nodes);
        }

        let mut rendered_replies = Vec::with_capacity(replies.len());

        let mut reply_futures = futures::stream::iter(replies)
            .map(|reply| async move { self.expand_discussion_comment(reply).await })
            .buffered(5);

        while let Some(result) = reply_futures.next().await {
            let (reply_comment, reply_urls, reply_requests) = result?;
            rendered_replies.push(reply_comment);
            request_urls.extend(reply_urls);
            graphql_requests.extend(reply_requests);
        }

        let mut metadata = vec![MetadataField {
            name: "Upvotes".to_owned(),
            value: comment.upvote_count.to_string(),
        }];
        if let Some(published_at) = comment.published_at.as_ref() {
            metadata.push(MetadataField {
                name: "Published".to_owned(),
                value: published_at.to_rfc3339(),
            });
        }

        Ok((
            Comment {
                id: comment.id,
                url: comment.url,
                author: comment.author.map(to_actor),
                author_association: comment.author_association,
                body: comment.body,
                created_at: Some(comment.created_at),
                updated_at: Some(comment.updated_at),
                reactions: to_reactions(comment.reaction_groups.as_deref()),
                metadata,
                replies: rendered_replies,
                is_answer: comment.is_answer,
            },
            request_urls,
            graphql_requests,
        ))
    }

    async fn fetch_discussion_replies_page(
        &self,
        id: &str,
        cursor: Option<String>,
        request_urls: &mut Vec<String>,
        graphql_requests: &mut Vec<RawGraphQlRequest>,
    ) -> anyhow::Result<GraphQlDiscussionCommentConnection> {
        let variables = json!({
            "id": id,
            "cursor": cursor,
        });
        request_urls.push(self.client.graphql_endpoint_url());
        graphql_requests.push(graphql_request(DISCUSSION_REPLIES_QUERY, &variables));
        let page: DiscussionRepliesRoot = self
            .client
            .graphql_query(DISCUSSION_REPLIES_QUERY, variables)
            .await?;

        let node = page.node.context("discussion comment not found")?;
        Ok(node.replies)
    }
}

impl<'a> IssueGraphQlFetcher<'a> {
    pub(crate) fn new(client: &'a GitHubClient) -> Self {
        Self { client }
    }

    pub(crate) async fn fetch_issue(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
    ) -> anyhow::Result<ExportDocument> {
        if !self.client.has_token() {
            bail!("issue export via GraphQL requires authentication");
        }

        let variables = json!({
            "owner": owner,
            "repo": repo,
            "number": number as i64,
            "cursor": Value::Null,
        });

        let graphql_url = self.client.graphql_endpoint_url();
        let mut request_urls = vec![graphql_url.clone()];
        let mut graphql_requests = vec![graphql_request(ISSUE_QUERY, &variables)];

        let data: IssueRoot = self.client.graphql_query(ISSUE_QUERY, variables).await?;
        let repository = data.repository.context("repository not found")?;
        let issue = repository.issue.context("issue not found")?;

        let mut comments = issue.comments.nodes.clone();
        let mut cursor = issue.comments.page_info.end_cursor.clone();
        let mut has_next_page = issue.comments.page_info.has_next_page;

        while has_next_page {
            let variables = json!({
                "owner": owner,
                "repo": repo,
                "number": number as i64,
                "cursor": cursor,
            });
            request_urls.push(graphql_url.clone());
            graphql_requests.push(graphql_request(ISSUE_COMMENTS_QUERY, &variables));

            let page: IssueCommentsOnlyRoot = self
                .client
                .graphql_query(ISSUE_COMMENTS_QUERY, variables)
                .await?;

            let repository = page.repository.context("repository not found")?;
            let issue_page = repository.issue.context("issue not found")?;
            has_next_page = issue_page.comments.page_info.has_next_page;
            cursor = issue_page.comments.page_info.end_cursor.clone();
            comments.extend(issue_page.comments.nodes);
        }

        let mut metadata = vec![
            MetadataField {
                name: "Comments".to_owned(),
                value: comments.len().to_string(),
            },
            MetadataField {
                name: "Locked".to_owned(),
                value: issue.locked.to_string(),
            },
        ];

        if let Some(reason) = issue.active_lock_reason.as_ref() {
            metadata.push(MetadataField {
                name: "Lock reason".to_owned(),
                value: reason.clone(),
            });
        }

        if let Some(reason) = issue.state_reason.as_ref() {
            metadata.push(MetadataField {
                name: "State reason".to_owned(),
                value: reason.clone(),
            });
        }

        let raw_payloads = vec![
            RawPayload {
                name: "graphql.issue".to_owned(),
                payload: serde_json::to_value(&issue)?,
                request_urls: vec![graphql_url.clone()],
                graphql_requests: vec![graphql_request(
                    ISSUE_QUERY,
                    &json!({
                        "owner": owner,
                        "repo": repo,
                        "number": number as i64,
                    }),
                )],
            },
            RawPayload {
                name: "graphql.issue_comments".to_owned(),
                payload: serde_json::to_value(&comments)?,
                request_urls,
                graphql_requests,
            },
        ];

        Ok(ExportDocument {
            kind: crate::cli::ResourceKind::Issue,
            owner: owner.to_owned(),
            repo: repo.to_owned(),
            number,
            title: issue.title,
            url: issue.url,
            state: issue.state.to_lowercase(),
            body: issue.body,
            author: issue.author.map(to_actor),
            author_association: issue.author_association,
            created_at: Some(issue.created_at),
            updated_at: Some(issue.updated_at),
            closed_at: issue.closed_at,
            labels: issue.labels.nodes.into_iter().map(to_label).collect(),
            assignees: issue.assignees.nodes.into_iter().map(to_actor).collect(),
            reactions: to_reactions(issue.reaction_groups.as_deref()),
            milestone: issue.milestone.map(|m| crate::model::Milestone {
                title: m.title,
                state: Some(m.state.to_lowercase()),
                due_on: m.due_on,
                url: Some(m.url),
            }),
            metadata,
            comments: comments.into_iter().map(to_comment).collect(),
            raw_payloads,
            ..ExportDocument::default()
        })
    }
}

impl<'a> PullRequestGraphQlFetcher<'a> {
    pub(crate) fn new(client: &'a GitHubClient) -> Self {
        Self { client }
    }

    pub(crate) async fn fetch_pull_request(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
    ) -> anyhow::Result<ExportDocument> {
        if !self.client.has_token() {
            bail!("pull request export via GraphQL requires authentication");
        }

        let variables = json!({
          "owner": owner,
          "repo": repo,
          "number": number as i64,
          "cursor": Value::Null,
        });

        let graphql_url = self.client.graphql_endpoint_url();
        let mut request_urls = vec![graphql_url.clone()];
        let mut graphql_requests = vec![graphql_request(PULL_REQUEST_QUERY, &variables)];

        let data: PullRequestRoot = self
            .client
            .graphql_query(PULL_REQUEST_QUERY, variables)
            .await?;
        let repository = data.repository.context("repository not found")?;
        let mut pull = repository.pull_request.context("pull request not found")?;

        let mut comments = pull.comments.nodes.clone();
        let mut cursor = pull.comments.page_info.end_cursor.clone();
        let mut has_next_page = pull.comments.page_info.has_next_page;

        while has_next_page {
            let variables = json!({
              "owner": owner,
              "repo": repo,
              "number": number as i64,
              "cursor": cursor,
            });
            request_urls.push(graphql_url.clone());
            graphql_requests.push(graphql_request(PULL_REQUEST_COMMENTS_QUERY, &variables));

            let page: PullRequestCommentsOnlyRoot = self
                .client
                .graphql_query(PULL_REQUEST_COMMENTS_QUERY, variables)
                .await?;

            let repository = page.repository.context("repository not found")?;
            let pull_request_page = repository.pull_request.context("pull request not found")?;
            has_next_page = pull_request_page.comments.page_info.has_next_page;
            cursor = pull_request_page.comments.page_info.end_cursor.clone();
            comments.extend(pull_request_page.comments.nodes);
        }

        pull.comments.nodes = comments.clone();
        pull.comments.page_info.has_next_page = false;
        pull.comments.page_info.end_cursor = cursor;

        let mut review_threads = Vec::new();
        let mut thread_futures = futures::stream::iter(pull.review_threads.nodes.clone())
            .map(|thread| async move { self.expand_review_thread(thread).await })
            .buffered(5);

        while let Some(result) = thread_futures.next().await {
            let (thread_item, thread_urls, thread_requests) = result?;
            review_threads.push(thread_item);
            request_urls.extend(thread_urls);
            graphql_requests.extend(thread_requests);
        }

        let mut metadata = vec![
            MetadataField {
                name: "Draft".to_owned(),
                value: pull.is_draft.to_string(),
            },
            MetadataField {
                name: "Merged".to_owned(),
                value: pull.merged.to_string(),
            },
            MetadataField {
                name: "Mergeable state".to_owned(),
                value: pull.mergeable.to_lowercase(),
            },
            MetadataField {
                name: "Base".to_owned(),
                value: format!(
                    "{}:{}",
                    pull.base_repository
                        .as_ref()
                        .map(|r| r.name_with_owner.clone())
                        .unwrap_or_default(),
                    pull.base_ref_name
                ),
            },
            MetadataField {
                name: "Head".to_owned(),
                value: format!(
                    "{}:{}",
                    pull.head_repository
                        .as_ref()
                        .map(|r| r.name_with_owner.clone())
                        .unwrap_or_default(),
                    pull.head_ref_name
                ),
            },
            MetadataField {
                name: "Commits".to_owned(),
                value: pull.commits.total_count.to_string(),
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
        ];

        if let Some(merged_at) = pull.merged_at {
            metadata.push(MetadataField {
                name: "Merged at".to_owned(),
                value: merged_at.to_rfc3339(),
            });
        }
        if let Some(merged_by) = pull.merged_by.as_ref() {
            metadata.push(MetadataField {
                name: "Merged by".to_owned(),
                value: merged_by.login.clone(),
            });
        }
        if let Some(sha) = pull.potential_merge_commit.as_ref().map(|c| c.oid.clone()) {
            metadata.push(MetadataField {
                name: "Merge commit".to_owned(),
                value: sha,
            });
        }

        let (rest_files, file_request_urls): (Vec<RestPullRequestFile>, Vec<String>) = self
            .client
            .get_rest_paginated_with_urls(&format!("repos/{owner}/{repo}/pulls/{number}/files"))
            .await?;

        let raw_payloads = vec![
            RawPayload {
                name: "graphql.pull_request".to_owned(),
                payload: serde_json::to_value(&pull)?,
                request_urls: request_urls.clone(),
                graphql_requests: graphql_requests.clone(),
            },
            RawPayload {
                name: "rest.pull_request_files".to_owned(),
                payload: serde_json::to_value(&rest_files)?,
                request_urls: file_request_urls.clone(),
                graphql_requests: Vec::new(),
            },
        ];

        Ok(ExportDocument {
            kind: crate::cli::ResourceKind::PullRequest,
            owner: owner.to_owned(),
            repo: repo.to_owned(),
            number,
            title: pull.title,
            url: pull.url,
            state: pull.state.to_lowercase(),
            body: pull.body,
            author: pull.author.map(to_actor),
            author_association: pull.author_association,
            created_at: Some(pull.created_at),
            updated_at: Some(pull.updated_at),
            closed_at: pull.closed_at,
            labels: pull.labels.nodes.into_iter().map(to_label).collect(),
            assignees: pull.assignees.nodes.into_iter().map(to_actor).collect(),
            requested_reviewers: pull
                .review_requests
                .nodes
                .into_iter()
                .filter_map(|r| r.requested_reviewer)
                .filter_map(|rr| match rr {
                    GraphQlReviewer::User(u) => Some(to_actor(u)),
                    _ => None,
                })
                .collect(),
            reactions: to_reactions(pull.reaction_groups.as_deref()),
            milestone: pull.milestone.map(|m| crate::model::Milestone {
                title: m.title,
                state: Some(m.state.to_lowercase()),
                due_on: m.due_on,
                url: Some(m.url),
            }),
            metadata,
            comments: comments.into_iter().map(to_comment).collect(),
            reviews: pull.reviews.nodes.into_iter().map(to_review).collect(),
            review_threads,
            files: rest_files.iter().map(to_changed_file_rest).collect(),
            commits: pull
                .commits
                .nodes
                .into_iter()
                .map(|c| to_commit_summary(c.commit))
                .collect(),
            raw_payloads,
            ..ExportDocument::default()
        })
    }

    pub(crate) async fn fetch_review_threads(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
    ) -> anyhow::Result<(
        Vec<ReviewThread>,
        Value,
        Vec<String>,
        Vec<RawGraphQlRequest>,
    )> {
        if !self.client.has_token() {
            return Ok((Vec::new(), Value::Null, Vec::new(), Vec::new()));
        }

        let mut response_pages = Vec::new();
        let mut threads = Vec::new();
        let mut cursor = Value::Null;
        let mut has_next_page = true;
        let mut request_urls = Vec::new();
        let mut graphql_requests = Vec::new();

        while has_next_page {
            let variables = json!({
                "owner": owner,
                "repo": repo,
                "number": number as i64,
                "cursor": cursor,
            });
            request_urls.push(self.client.graphql_endpoint_url());
            graphql_requests.push(graphql_request(PULL_REQUEST_THREADS_QUERY, &variables));
            let data: PullRequestThreadsRoot = self
                .client
                .graphql_query(PULL_REQUEST_THREADS_QUERY, variables)
                .await?;

            let repository = data.repository.context("repository not found")?;
            let pull_request = repository.pull_request.context("pull request not found")?;
            has_next_page = pull_request.review_threads.page_info.has_next_page;
            cursor = pull_request
                .review_threads
                .page_info
                .end_cursor
                .clone()
                .map(Value::String)
                .unwrap_or(Value::Null);

            response_pages.push(serde_json::to_value(&pull_request.review_threads.nodes)?);

            let mut thread_futures = futures::stream::iter(pull_request.review_threads.nodes)
                .map(|thread| async move { self.expand_review_thread(thread).await })
                .buffered(5);

            while let Some(result) = thread_futures.next().await {
                let (thread_item, thread_urls, thread_requests) = result?;
                threads.push(thread_item);
                request_urls.extend(thread_urls);
                graphql_requests.extend(thread_requests);
            }
        }

        Ok((
            threads,
            Value::Array(response_pages),
            request_urls,
            graphql_requests,
        ))
    }

    async fn expand_review_thread(
        &self,
        mut thread: GraphQlReviewThread,
    ) -> anyhow::Result<(ReviewThread, Vec<String>, Vec<RawGraphQlRequest>)> {
        let mut request_urls = Vec::new();
        let mut graphql_requests = Vec::new();
        let mut comments = thread.comments.nodes.clone();
        let mut cursor = thread.comments.page_info.end_cursor.clone();
        let mut has_next_page = thread.comments.page_info.has_next_page;

        while has_next_page {
            let variables = json!({
                "id": thread.id,
                "cursor": cursor,
            });
            request_urls.push(self.client.graphql_endpoint_url());
            graphql_requests.push(graphql_request(
                PULL_REQUEST_THREAD_COMMENTS_QUERY,
                &variables,
            ));
            let page: PullRequestThreadCommentsRoot = self
                .client
                .graphql_query(PULL_REQUEST_THREAD_COMMENTS_QUERY, variables)
                .await?;

            let node = page.node.context("review thread not found")?;
            has_next_page = node.comments.page_info.has_next_page;
            cursor = node.comments.page_info.end_cursor.clone();
            comments.extend(node.comments.nodes);
        }

        thread.comments.nodes.clear();

        Ok((
            ReviewThread {
                id: thread.id,
                path: thread.path,
                is_resolved: Some(thread.is_resolved),
                is_outdated: Some(thread.is_outdated),
                comments: comments.into_iter().map(to_review_comment).collect(),
            },
            request_urls,
            graphql_requests,
        ))
    }
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct GraphQlEnvelope<T> {
    pub data: Option<T>,
    pub errors: Option<Vec<GraphQlError>>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct GraphQlError {
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct GraphQlPageInfo {
    pub has_next_page: bool,
    pub end_cursor: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct GraphQlActor {
    pub login: String,
    pub url: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct GraphQlLabelConnection {
    #[serde(default)]
    pub nodes: Vec<GraphQlLabel>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct GraphQlLabel {
    pub name: String,
    pub color: Option<String>,
    pub description: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct GraphQlReactionGroup {
    pub content: String,
    pub users: GraphQlUsersConnection,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct GraphQlUsersConnection {
    pub total_count: u64,
    #[serde(default)]
    pub nodes: Vec<GraphQlActor>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiscussionRoot {
    repository: Option<DiscussionRepository>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiscussionCommentsOnlyRoot {
    repository: Option<DiscussionCommentsRepository>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiscussionRepliesRoot {
    node: Option<DiscussionCommentNode>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiscussionRepository {
    discussion: Option<GraphQlDiscussion>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiscussionCommentsRepository {
    discussion: Option<DiscussionCommentsOnly>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiscussionCommentNode {
    replies: GraphQlDiscussionCommentConnection,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiscussionCommentsOnly {
    comments: GraphQlDiscussionCommentConnection,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct GraphQlDiscussion {
    id: String,
    number: i64,
    title: String,
    url: String,
    body: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    closed: bool,
    closed_at: Option<DateTime<Utc>>,
    locked: bool,
    author: Option<GraphQlActor>,
    author_association: Option<String>,
    category: GraphQlDiscussionCategory,
    answer_chosen_at: Option<DateTime<Utc>>,
    answer_chosen_by: Option<GraphQlActor>,
    labels: GraphQlLabelConnection,
    reaction_groups: Option<Vec<GraphQlReactionGroup>>,
    comments: GraphQlDiscussionCommentConnection,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct GraphQlDiscussionCategory {
    name: String,
    slug: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct GraphQlDiscussionCommentConnection {
    page_info: GraphQlPageInfo,
    #[serde(default)]
    nodes: Vec<GraphQlDiscussionComment>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct GraphQlDiscussionComment {
    id: String,
    url: String,
    body: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    published_at: Option<DateTime<Utc>>,
    author: Option<GraphQlActor>,
    author_association: Option<String>,
    is_answer: bool,
    upvote_count: i64,
    reaction_groups: Option<Vec<GraphQlReactionGroup>>,
    #[serde(default)]
    replies: Option<GraphQlDiscussionCommentConnection>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct IssueRoot {
    repository: Option<IssueRepository>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct IssueCommentsOnlyRoot {
    repository: Option<IssueCommentsRepository>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct IssueRepository {
    issue: Option<GraphQlIssue>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct IssueCommentsRepository {
    issue: Option<GraphQlIssueCommentsOnly>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct GraphQlIssue {
    id: String,
    number: i64,
    title: String,
    url: String,
    body: String,
    state: String,
    state_reason: Option<String>,
    author: Option<GraphQlActor>,
    author_association: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    closed_at: Option<DateTime<Utc>>,
    locked: bool,
    active_lock_reason: Option<String>,
    milestone: Option<GraphQlMilestone>,
    labels: GraphQlLabelConnection,
    assignees: GraphQlActorConnection,
    reaction_groups: Option<Vec<GraphQlReactionGroup>>,
    comments: GraphQlIssueCommentConnection,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct GraphQlIssueCommentsOnly {
    comments: GraphQlIssueCommentConnection,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct GraphQlIssueCommentConnection {
    page_info: GraphQlPageInfo,
    #[serde(default)]
    nodes: Vec<GraphQlIssueComment>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct GraphQlIssueComment {
    id: String,
    url: String,
    body: String,
    author: Option<GraphQlActor>,
    author_association: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    reaction_groups: Option<Vec<GraphQlReactionGroup>>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct GraphQlMilestone {
    title: String,
    state: String,
    due_on: Option<DateTime<Utc>>,
    url: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct GraphQlActorConnection {
    #[serde(default)]
    nodes: Vec<GraphQlActor>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PullRequestRoot {
    repository: Option<PullRequestRepository>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PullRequestCommentsOnlyRoot {
    repository: Option<PullRequestCommentsRepository>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PullRequestRepository {
    pull_request: Option<GraphQlPullRequest>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PullRequestCommentsRepository {
    pull_request: Option<GraphQlPullRequestCommentsOnly>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct GraphQlPullRequest {
    id: String,
    number: i64,
    title: String,
    url: String,
    body: String,
    state: String,
    is_draft: bool,
    merged: bool,
    merged_at: Option<DateTime<Utc>>,
    merged_by: Option<GraphQlActor>,
    mergeable: String,
    author: Option<GraphQlActor>,
    author_association: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    closed_at: Option<DateTime<Utc>>,
    labels: GraphQlLabelConnection,
    assignees: GraphQlActorConnection,
    milestone: Option<GraphQlMilestone>,
    reaction_groups: Option<Vec<GraphQlReactionGroup>>,
    comments: GraphQlIssueCommentConnection,
    review_requests: GraphQlReviewRequestConnection,
    reviews: GraphQlReviewConnection,
    review_threads: GraphQlReviewThreadConnection,
    commits: GraphQlCommitConnection,
    base_ref_name: String,
    base_repository: Option<GraphQlRepoRef>,
    head_ref_name: String,
    head_repository: Option<GraphQlRepoRef>,
    additions: i64,
    deletions: i64,
    changed_files: i64,
    potential_merge_commit: Option<GraphQlCommitRef>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct GraphQlPullRequestCommentsOnly {
    comments: GraphQlIssueCommentConnection,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct GraphQlRepoRef {
    name_with_owner: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct GraphQlCommitRef {
    oid: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct GraphQlReviewRequestConnection {
    #[serde(default)]
    nodes: Vec<GraphQlReviewRequest>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct GraphQlReviewRequest {
    requested_reviewer: Option<GraphQlReviewer>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "__typename")]
enum GraphQlReviewer {
    User(GraphQlActor),
    Team,
    Mannequin,
    Bot,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct GraphQlReviewConnection {
    #[serde(default)]
    nodes: Vec<GraphQlReview>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct GraphQlReview {
    id: String,
    url: String,
    state: String,
    body: String,
    author: Option<GraphQlActor>,
    author_association: Option<String>,
    submitted_at: Option<DateTime<Utc>>,
    commit: Option<GraphQlCommitRef>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct GraphQlCommitConnection {
    total_count: i64,
    #[serde(default)]
    nodes: Vec<GraphQlPullRequestCommit>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct GraphQlPullRequestCommit {
    commit: GraphQlFullCommit,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct GraphQlFullCommit {
    oid: String,
    url: String,
    message: String,
    author: GraphQlGitActor,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct GraphQlGitActor {
    name: String,
    date: Option<DateTime<Utc>>,
    user: Option<GraphQlActor>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PullRequestThreadsRoot {
    repository: Option<PullRequestThreadsRepository>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PullRequestThreadsRepository {
    pull_request: Option<GraphQlPullRequestThreads>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct GraphQlPullRequestThreads {
    review_threads: GraphQlReviewThreadConnection,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct GraphQlReviewThreadConnection {
    page_info: GraphQlPageInfo,
    #[serde(default)]
    nodes: Vec<GraphQlReviewThread>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct GraphQlReviewThread {
    id: String,
    is_resolved: bool,
    is_outdated: bool,
    path: Option<String>,
    comments: GraphQlReviewCommentConnection,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PullRequestThreadCommentsRoot {
    node: Option<GraphQlReviewThreadNode>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct GraphQlReviewThreadNode {
    comments: GraphQlReviewCommentConnection,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct GraphQlReviewCommentConnection {
    page_info: GraphQlPageInfo,
    #[serde(default)]
    nodes: Vec<GraphQlReviewComment>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct GraphQlReviewComment {
    id: String,
    url: String,
    body: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    author: Option<GraphQlActor>,
    author_association: Option<String>,
    diff_hunk: Option<String>,
    path: Option<String>,
    line: Option<i64>,
    start_line: Option<i64>,
    reaction_groups: Option<Vec<GraphQlReactionGroup>>,
    reply_to: Option<GraphQlReviewCommentRef>,
    pull_request_review: Option<GraphQlPullRequestReviewRef>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct GraphQlReviewCommentRef {
    id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct GraphQlPullRequestReviewRef {
    id: String,
}

fn to_actor(actor: GraphQlActor) -> Actor {
    Actor {
        login: actor.login,
        url: actor.url,
    }
}

fn to_label(label: GraphQlLabel) -> Label {
    Label {
        name: label.name,
        color: label.color,
        description: label.description,
    }
}

fn to_reactions(groups: Option<&[GraphQlReactionGroup]>) -> Vec<Reaction> {
    let mut normalized = groups
        .unwrap_or(&[])
        .iter()
        .filter(|group| group.users.total_count > 0)
        .map(|group| Reaction {
            content: normalize_graphql_reaction_content(&group.content).to_owned(),
            count: group.users.total_count,
            users: group.users.nodes.iter().cloned().map(to_actor).collect(),
        })
        .collect::<Vec<_>>();

    normalized.sort_by(|left, right| {
        reaction_rank(&left.content)
            .cmp(&reaction_rank(&right.content))
            .then_with(|| left.content.cmp(&right.content))
    });
    normalized
}

fn normalize_graphql_reaction_content(content: &str) -> &str {
    match content {
        "THUMBS_UP" => "+1",
        "THUMBS_DOWN" => "-1",
        "LAUGH" => "laugh",
        "HOORAY" => "hooray",
        "CONFUSED" => "confused",
        "HEART" => "heart",
        "ROCKET" => "rocket",
        "EYES" => "eyes",
        _ => content,
    }
}

fn reaction_rank(content: &str) -> usize {
    match content {
        "+1" => 0,
        "-1" => 1,
        "laugh" => 2,
        "hooray" => 3,
        "confused" => 4,
        "heart" => 5,
        "rocket" => 6,
        "eyes" => 7,
        _ => usize::MAX,
    }
}

fn to_review_comment(comment: GraphQlReviewComment) -> ReviewComment {
    ReviewComment {
        id: comment.id,
        url: comment.url,
        author: comment.author.map(to_actor),
        author_association: comment.author_association,
        body: comment.body,
        created_at: Some(comment.created_at),
        updated_at: Some(comment.updated_at),
        path: comment.path,
        line: comment.line,
        start_line: comment.start_line,
        diff_hunk: comment.diff_hunk,
        in_reply_to: comment.reply_to.map(|reply| reply.id),
        review_id: comment.pull_request_review.map(|review| review.id),
        reactions: to_reactions(comment.reaction_groups.as_deref()),
    }
}

fn to_comment(comment: GraphQlIssueComment) -> Comment {
    Comment {
        id: comment.id,
        url: comment.url,
        author: comment.author.map(to_actor),
        author_association: comment.author_association,
        body: comment.body,
        created_at: Some(comment.created_at),
        updated_at: Some(comment.updated_at),
        reactions: to_reactions(comment.reaction_groups.as_deref()),
        ..Comment::default()
    }
}

fn to_review(review: GraphQlReview) -> crate::model::Review {
    crate::model::Review {
        id: review.id,
        url: review.url,
        author: review.author.map(to_actor),
        author_association: review.author_association,
        state: review.state,
        body: review.body,
        submitted_at: review.submitted_at,
        commit_id: review.commit.map(|c| c.oid),
    }
}

fn to_commit_summary(commit: GraphQlFullCommit) -> crate::model::CommitSummary {
    crate::model::CommitSummary {
        sha: commit.oid,
        url: Some(commit.url),
        message: commit.message,
        author_name: Some(commit.author.name),
        authored_at: commit.author.date,
        author_user: commit.author.user.map(to_actor),
    }
}

fn rendered_comments_to_raw(comments: &[Comment]) -> Value {
    serde_json::to_value(
        comments
            .iter()
            .map(|comment| {
                json!({
                    "id": comment.id,
                    "url": comment.url,
                    "author": comment.author.as_ref().map(|actor| actor.login.clone()),
                    "body": comment.body,
                    "replies": rendered_comments_to_raw(&comment.replies),
                })
            })
            .collect::<Vec<_>>(),
    )
    .unwrap_or(Value::Null)
}

fn graphql_request(query: &str, variables: &Value) -> RawGraphQlRequest {
    RawGraphQlRequest {
        query: query.to_owned(),
        variables: variables.clone(),
    }
}

const DISCUSSION_QUERY: &str = r#"
query Discussion($owner: String!, $repo: String!, $number: Int!, $cursor: String) {
  repository(owner: $owner, name: $repo) {
    discussion(number: $number) {
      id
      number
      title
      url
      body
      createdAt
      updatedAt
      closed
      closedAt
      locked
      author {
        login
        url
      }
      authorAssociation
      category {
        name
        slug
      }
      answerChosenAt
      answerChosenBy {
        login
        url
      }
      labels(first: 100) {
        nodes {
          name
          color
          description
        }
      }
      reactionGroups {
        content
        users(first: 10) {
          totalCount
                    nodes {
                        login
                        url
                    }
        }
      }
      comments(first: 100, after: $cursor) {
        pageInfo {
          hasNextPage
          endCursor
        }
        nodes {
          id
          url
          body
          createdAt
          updatedAt
          publishedAt
          author {
            login
            url
          }
          authorAssociation
          isAnswer
          upvoteCount
          reactionGroups {
            content
            users(first: 10) {
              totalCount
                            nodes {
                                login
                                url
                            }
            }
          }
          replies(first: 100) {
            pageInfo {
              hasNextPage
              endCursor
            }
            nodes {
              id
              url
              body
              createdAt
              updatedAt
              publishedAt
              author {
                login
                url
              }
              authorAssociation
              isAnswer
              upvoteCount
              reactionGroups {
                content
                users(first: 10) {
                  totalCount
                                    nodes {
                                        login
                                        url
                                    }
                }
              }
            }
          }
        }
      }
    }
  }
}
"#;

const DISCUSSION_COMMENTS_QUERY: &str = r#"
query DiscussionComments($owner: String!, $repo: String!, $number: Int!, $cursor: String) {
  repository(owner: $owner, name: $repo) {
    discussion(number: $number) {
      comments(first: 100, after: $cursor) {
        pageInfo {
          hasNextPage
          endCursor
        }
        nodes {
          id
          url
          body
          createdAt
          updatedAt
          publishedAt
          author {
            login
            url
          }
          authorAssociation
          isAnswer
          upvoteCount
          reactionGroups {
            content
            users(first: 10) {
              totalCount
                            nodes {
                                login
                                url
                            }
            }
          }
          replies(first: 100) {
            pageInfo {
              hasNextPage
              endCursor
            }
            nodes {
              id
              url
              body
              createdAt
              updatedAt
              publishedAt
              author {
                login
                url
              }
              authorAssociation
              isAnswer
              upvoteCount
              reactionGroups {
                content
                users(first: 10) {
                  totalCount
                                    nodes {
                                        login
                                        url
                                    }
                }
              }
            }
          }
        }
      }
    }
  }
}
"#;

const DISCUSSION_REPLIES_QUERY: &str = r#"
query DiscussionReplies($id: ID!, $cursor: String) {
  node(id: $id) {
    ... on DiscussionComment {
      replies(first: 100, after: $cursor) {
        pageInfo {
          hasNextPage
          endCursor
        }
        nodes {
          id
          url
          body
          createdAt
          updatedAt
          publishedAt
          author {
            login
            url
          }
          authorAssociation
          isAnswer
          upvoteCount
        reactionGroups {
          content
          users(first: 10) {
            totalCount
                        nodes {
                            login
                            url
                        }
          }
        }
        }
      }
    }
  }
}
"#;

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use crate::{
        cli::ResourceKind,
        github::{GitHubClient, GitHubConfig},
        model::{ExportDocument, MetadataField},
        template,
        testsupport::load_json_fixture,
    };

    use super::{
        DISCUSSION_COMMENTS_QUERY, DISCUSSION_QUERY, DISCUSSION_REPLIES_QUERY, DiscussionFetcher,
        GraphQlDiscussion, GraphQlReactionGroup, GraphQlUsersConnection,
        PULL_REQUEST_COMMENTS_QUERY, PULL_REQUEST_QUERY, to_actor, to_label, to_reactions,
    };

    #[test]
    fn discussion_queries_only_expand_one_reply_level_per_request() {
        assert_eq!(DISCUSSION_QUERY.matches("replies(first: 100").count(), 1);
        assert_eq!(
            DISCUSSION_COMMENTS_QUERY
                .matches("replies(first: 100")
                .count(),
            1
        );
        assert_eq!(
            DISCUSSION_REPLIES_QUERY
                .matches("replies(first: 100")
                .count(),
            1
        );
    }

    #[test]
    fn graphql_reactions_are_normalized_for_template_rendering() {
        let reactions = to_reactions(Some(&[
            GraphQlReactionGroup {
                content: "HEART".to_owned(),
                users: GraphQlUsersConnection {
                    total_count: 2,
                    nodes: Vec::new(),
                },
            },
            GraphQlReactionGroup {
                content: "THUMBS_UP".to_owned(),
                users: GraphQlUsersConnection {
                    total_count: 1,
                    nodes: Vec::new(),
                },
            },
            GraphQlReactionGroup {
                content: "CUSTOM".to_owned(),
                users: GraphQlUsersConnection {
                    total_count: 3,
                    nodes: Vec::new(),
                },
            },
        ]));

        assert_eq!(
            reactions
                .into_iter()
                .map(|reaction| (reaction.content, reaction.count))
                .collect::<Vec<_>>(),
            vec![
                ("+1".to_owned(), 1),
                ("heart".to_owned(), 2),
                ("CUSTOM".to_owned(), 3),
            ]
        );
    }

    #[test]
    fn pull_request_queries_paginate_comments() {
        assert!(PULL_REQUEST_QUERY.contains("$cursor: String"));
        assert!(PULL_REQUEST_QUERY.contains("comments(first: 100, after: $cursor)"));
        assert!(PULL_REQUEST_COMMENTS_QUERY.contains("$cursor: String"));
        assert!(PULL_REQUEST_COMMENTS_QUERY.contains("comments(first: 100, after: $cursor)"));
    }

    #[test]
    fn graphql_expansions_preserve_input_order() {
        let source = include_str!("graphql.rs");
        let forbidden = ["buffer", "_unordered(5)"].concat();

        assert!(!source.contains(&forbidden));
        assert!(source.contains(".buffered(5)"));
    }

    #[tokio::test]
    async fn discussion_fixture_renders_without_network() {
        let client = GitHubClient::new(GitHubConfig {
            api_base_url: "https://api.github.com".to_owned(),
            graphql_url: "https://api.github.com/graphql".to_owned(),
            user_agent: "ghdump/test".to_owned(),
            token: Some("fixture-token".to_owned()),
        })
        .expect("fixture client should build");
        let fetcher = DiscussionFetcher::new(&client);
        let discussion: GraphQlDiscussion = load_json_fixture("discussion/graphql.discussion.json");

        let top_level_comments = discussion.comments.nodes.clone();
        let mut rendered_comments = Vec::with_capacity(top_level_comments.len());
        let mut request_urls = Vec::new();
        let mut graphql_requests = Vec::new();

        for comment in top_level_comments {
            let (rendered_comment, comment_request_urls, comment_graphql_requests) = fetcher
                .expand_discussion_comment(comment)
                .await
                .expect("fixture comment tree should expand");
            rendered_comments.push(rendered_comment);
            request_urls.extend(comment_request_urls);
            graphql_requests.extend(comment_graphql_requests);
        }

        let mut metadata = vec![
            MetadataField {
                name: "Category".to_owned(),
                value: discussion.category.name.clone(),
            },
            MetadataField {
                name: "Category slug".to_owned(),
                value: discussion.category.slug.clone(),
            },
            MetadataField {
                name: "Closed".to_owned(),
                value: discussion.closed.to_string(),
            },
            MetadataField {
                name: "Locked".to_owned(),
                value: discussion.locked.to_string(),
            },
            MetadataField {
                name: "Comment count".to_owned(),
                value: rendered_comments.len().to_string(),
            },
        ];
        if let Some(answer_chosen_at) = discussion.answer_chosen_at.as_ref() {
            metadata.push(MetadataField {
                name: "Answer chosen at".to_owned(),
                value: answer_chosen_at.to_rfc3339(),
            });
        }
        if let Some(answer_chosen_by) = discussion.answer_chosen_by.as_ref() {
            metadata.push(MetadataField {
                name: "Answer chosen by".to_owned(),
                value: answer_chosen_by.login.clone(),
            });
        }

        let document = ExportDocument {
            kind: ResourceKind::Discussion,
            owner: "acme".to_owned(),
            repo: "widgets".to_owned(),
            number: discussion.number as u64,
            title: discussion.title.clone(),
            url: discussion.url.clone(),
            state: if discussion.closed {
                "closed".to_owned()
            } else {
                "open".to_owned()
            },
            body: discussion.body.clone(),
            author: discussion.author.clone().map(to_actor),
            author_association: discussion.author_association.clone(),
            created_at: Some(discussion.created_at),
            updated_at: Some(discussion.updated_at),
            closed_at: discussion.closed_at,
            labels: discussion
                .labels
                .nodes
                .iter()
                .cloned()
                .map(to_label)
                .collect(),
            reactions: to_reactions(discussion.reaction_groups.as_deref()),
            metadata,
            comments: rendered_comments,
            ..ExportDocument::default()
        };

        assert!(request_urls.is_empty());
        assert!(graphql_requests.is_empty());
        assert_eq!(document.comments.len(), 1);
        assert_eq!(document.comments[0].replies.len(), 1);
        assert_eq!(document.reactions[0].content, "+1");

        let markdown = template::render(&document, None).expect("discussion fixture should render");

        assert!(markdown.contains("# Discussion #329: Fixture discussion"));
        assert!(markdown.contains("- 👍 `+1`: 2x"));
        assert!(markdown.contains("- ❤️ `heart`: 1x"));
        assert!(markdown.contains("- Marked answer: true"));
        assert!(markdown.contains("#### Comment 1 by [reply-user](https://github.com/reply-user)"));
    }
}

const PULL_REQUEST_THREADS_QUERY: &str = r#"
query PullRequestThreads($owner: String!, $repo: String!, $number: Int!, $cursor: String) {
  repository(owner: $owner, name: $repo) {
    pullRequest(number: $number) {
      reviewThreads(first: 100, after: $cursor) {
        pageInfo {
          hasNextPage
          endCursor
        }
        nodes {
          id
          isResolved
          isOutdated
          path
          comments(first: 100) {
            pageInfo {
              hasNextPage
              endCursor
            }
            nodes {
              id
              url
              body
              createdAt
              updatedAt
              author {
                login
                url
              }
              authorAssociation
              diffHunk
              path
              line
              startLine
              reactionGroups {
                content
                users(first: 10) {
                  totalCount
                                    nodes {
                                        login
                                        url
                                    }
                }
              }
              replyTo {
                id
              }
              pullRequestReview {
                id
              }
            }
          }
        }
      }
    }
  }
}
"#;

const PULL_REQUEST_QUERY: &str = r#"
query PullRequest($owner: String!, $repo: String!, $number: Int!, $cursor: String) {
  repository(owner: $owner, name: $repo) {
    pullRequest(number: $number) {
      id
      number
      title
      url
      body
      state
      isDraft
      merged
      mergedAt
      mergedBy {
        login
        url
      }
      mergeable
      author {
        login
        url
      }
      authorAssociation
      createdAt
      updatedAt
      closedAt
      additions
      deletions
      changedFiles
      baseRefName
      baseRepository {
        nameWithOwner
      }
      headRefName
      headRepository {
        nameWithOwner
      }
      potentialMergeCommit {
        oid
      }
      labels(first: 100) {
        nodes {
          name
          color
          description
        }
      }
      assignees(first: 100) {
        nodes {
          login
          url
        }
      }
      milestone {
        title
        state
        dueOn
        url
      }
      reactionGroups {
        content
        users(first: 10) {
          totalCount
          nodes {
            login
            url
          }
        }
      }
      comments(first: 100, after: $cursor) {
        pageInfo {
          hasNextPage
          endCursor
        }
        nodes {
          id
          url
          body
          author {
            login
            url
          }
          authorAssociation
          createdAt
          updatedAt
          reactionGroups {
            content
            users(first: 10) {
              totalCount
              nodes {
                login
                url
              }
            }
          }
        }
      }
      reviewRequests(first: 100) {
        nodes {
          requestedReviewer {
            __typename
            ... on User {
              login
              url
            }
          }
        }
      }
      reviews(first: 100) {
        nodes {
          id
          url
          state
          body
          author {
            login
            url
          }
          authorAssociation
          submittedAt
          commit {
            oid
          }
        }
      }
      reviewThreads(first: 100) {
        pageInfo {
          hasNextPage
          endCursor
        }
        nodes {
          id
          isResolved
          isOutdated
          path
          comments(first: 100) {
            pageInfo {
              hasNextPage
              endCursor
            }
            nodes {
              id
              url
              body
              createdAt
              updatedAt
              author {
                login
                url
              }
              authorAssociation
              diffHunk
              path
              line
              startLine
              reactionGroups {
                content
                users(first: 10) {
                  totalCount
                  nodes {
                    login
                    url
                  }
                }
              }
              replyTo {
                id
              }
              pullRequestReview {
                id
              }
            }
          }
        }
      }
      commits(first: 100) {
        totalCount
        nodes {
          commit {
            oid
            url
            message
            author {
              name
              date
              user {
                login
                url
              }
            }
          }
        }
      }
    }
  }
}
"#;

const PULL_REQUEST_COMMENTS_QUERY: &str = r#"
query PullRequestComments($owner: String!, $repo: String!, $number: Int!, $cursor: String) {
  repository(owner: $owner, name: $repo) {
    pullRequest(number: $number) {
      comments(first: 100, after: $cursor) {
        pageInfo {
          hasNextPage
          endCursor
        }
        nodes {
          id
          url
          body
          author {
            login
            url
          }
          authorAssociation
          createdAt
          updatedAt
          reactionGroups {
            content
            users(first: 10) {
              totalCount
              nodes {
                login
                url
              }
            }
          }
        }
      }
    }
  }
}
"#;

const PULL_REQUEST_THREAD_COMMENTS_QUERY: &str = r#"
query PullRequestThreadComments($id: ID!, $cursor: String) {
  node(id: $id) {
    ... on PullRequestReviewThread {
      comments(first: 100, after: $cursor) {
        pageInfo {
          hasNextPage
          endCursor
        }
        nodes {
          id
          url
          body
          createdAt
          updatedAt
          author {
            login
            url
          }
          authorAssociation
          diffHunk
          path
          line
          startLine
          reactionGroups {
            content
            users(first: 10) {
              totalCount
                            nodes {
                                login
                                url
                            }
            }
          }
          replyTo {
            id
          }
          pullRequestReview {
            id
          }
        }
      }
    }
  }
}
"#;

const ISSUE_QUERY: &str = r#"
query Issue($owner: String!, $repo: String!, $number: Int!, $cursor: String) {
  repository(owner: $owner, name: $repo) {
    issue(number: $number) {
      id
      number
      title
      url
      body
      state
      stateReason
      author {
        login
        url
      }
      authorAssociation
      createdAt
      updatedAt
      closedAt
      locked
      activeLockReason
      milestone {
        title
        state
        dueOn
        url
      }
      labels(first: 100) {
        nodes {
          name
          color
          description
        }
      }
      assignees(first: 100) {
        nodes {
          login
          url
        }
      }
      reactionGroups {
        content
        users(first: 10) {
          totalCount
          nodes {
            login
            url
          }
        }
      }
      comments(first: 100, after: $cursor) {
        pageInfo {
          hasNextPage
          endCursor
        }
        nodes {
          id
          url
          body
          author {
            login
            url
          }
          authorAssociation
          createdAt
          updatedAt
          reactionGroups {
            content
            users(first: 10) {
              totalCount
              nodes {
                login
                url
              }
            }
          }
        }
      }
    }
  }
}
"#;

const ISSUE_COMMENTS_QUERY: &str = r#"
query IssueComments($owner: String!, $repo: String!, $number: Int!, $cursor: String) {
  repository(owner: $owner, name: $repo) {
    issue(number: $number) {
      comments(first: 100, after: $cursor) {
        pageInfo {
          hasNextPage
          endCursor
        }
        nodes {
          id
          url
          body
          author {
            login
            url
          }
          authorAssociation
          createdAt
          updatedAt
          reactionGroups {
            content
            users(first: 10) {
              totalCount
              nodes {
                login
                url
              }
            }
          }
        }
      }
    }
  }
}
"#;
