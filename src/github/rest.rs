use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RestActor {
    pub login: String,
    pub html_url: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RestLabel {
    pub name: String,
    pub color: Option<String>,
    pub description: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RestReactions {
    #[serde(rename = "+1")]
    pub plus_one: u64,
    #[serde(rename = "-1")]
    pub minus_one: u64,
    pub laugh: u64,
    pub hooray: u64,
    pub confused: u64,
    pub heart: u64,
    pub rocket: u64,
    pub eyes: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RestReaction {
    pub content: String,
    pub user: Option<RestActor>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RestMilestone {
    pub title: String,
    pub state: Option<String>,
    pub due_on: Option<DateTime<Utc>>,
    pub html_url: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RestIssue {
    pub html_url: String,
    pub title: String,
    pub body: Option<String>,
    pub state: String,
    pub state_reason: Option<String>,
    pub user: Option<RestActor>,
    pub author_association: Option<String>,
    #[serde(default)]
    pub labels: Vec<RestLabel>,
    #[serde(default)]
    pub assignees: Vec<RestActor>,
    pub comments: u64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
    pub milestone: Option<RestMilestone>,
    pub reactions: Option<RestReactions>,
    pub locked: bool,
    pub active_lock_reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RestIssueComment {
    pub id: u64,
    pub html_url: String,
    pub body: Option<String>,
    pub user: Option<RestActor>,
    pub author_association: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub reactions: Option<RestReactions>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RestTimelineLabel {
    pub name: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RestTimelineRename {
    pub from: String,
    pub to: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RestTimelineSourceIssue {
    pub number: u64,
    pub title: String,
    pub html_url: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RestTimelineSource {
    pub issue: Option<RestTimelineSourceIssue>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RestTimelineCommitIdentity {
    pub name: Option<String>,
    pub email: Option<String>,
    pub date: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RestTimelineEvent {
    pub event: String,
    pub actor: Option<RestActor>,
    pub created_at: Option<DateTime<Utc>>,
    pub submitted_at: Option<DateTime<Utc>>,
    pub body: Option<String>,
    pub state: Option<String>,
    pub label: Option<RestTimelineLabel>,
    pub commit_id: Option<String>,
    pub sha: Option<String>,
    pub message: Option<String>,
    pub author: Option<RestTimelineCommitIdentity>,
    pub committer: Option<RestTimelineCommitIdentity>,
    pub rename: Option<RestTimelineRename>,
    pub source: Option<RestTimelineSource>,
    pub reviewed: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RestRepoRef {
    pub full_name: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RestBranchRef {
    #[serde(rename = "ref")]
    pub ref_field: String,
    pub sha: String,
    pub repo: Option<RestRepoRef>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RestPullRequest {
    pub html_url: String,
    pub title: String,
    pub body: Option<String>,
    pub state: String,
    pub draft: bool,
    pub merged: Option<bool>,
    pub merged_at: Option<DateTime<Utc>>,
    pub merged_by: Option<RestActor>,
    pub user: Option<RestActor>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
    pub additions: i64,
    pub deletions: i64,
    pub changed_files: i64,
    pub commits: i64,
    pub mergeable_state: Option<String>,
    pub merge_commit_sha: Option<String>,
    pub base: RestBranchRef,
    pub head: RestBranchRef,
    #[serde(default)]
    pub requested_reviewers: Vec<RestActor>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RestPullRequestReview {
    pub id: u64,
    pub html_url: String,
    pub body: Option<String>,
    pub state: String,
    pub user: Option<RestActor>,
    pub author_association: Option<String>,
    pub submitted_at: Option<DateTime<Utc>>,
    pub commit_id: Option<String>,
    pub reactions: Option<RestReactions>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RestPullRequestComment {
    pub id: u64,
    pub html_url: String,
    pub body: Option<String>,
    pub user: Option<RestActor>,
    pub author_association: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub path: String,
    pub line: Option<i64>,
    pub start_line: Option<i64>,
    pub diff_hunk: Option<String>,
    pub in_reply_to_id: Option<u64>,
    pub pull_request_review_id: Option<u64>,
    pub reactions: Option<RestReactions>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RestPullRequestFile {
    pub sha: String,
    pub filename: String,
    pub status: String,
    pub additions: i64,
    pub deletions: i64,
    pub changes: i64,
    pub previous_filename: Option<String>,
    pub blob_url: Option<String>,
    pub patch: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RestGitCommitAuthor {
    pub name: String,
    pub date: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RestGitCommit {
    pub message: String,
    pub author: Option<RestGitCommitAuthor>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RestCommit {
    pub sha: String,
    pub html_url: Option<String>,
    pub author: Option<RestActor>,
    pub commit: RestGitCommit,
    pub files: Option<Vec<RestPullRequestFile>>,
}
