use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::cli::ResourceKind;

#[derive(Clone, Debug, Default)]
pub struct Actor {
    pub login: String,
    pub url: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Label {
    pub name: String,
    pub color: Option<String>,
    pub description: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Reaction {
    pub content: String,
    pub count: u64,
    pub users: Vec<Actor>,
}

#[derive(Clone, Debug, Default)]
pub struct MetadataField {
    pub name: String,
    pub value: String,
}

#[derive(Clone, Debug, Default)]
pub struct Milestone {
    pub title: String,
    pub state: Option<String>,
    pub due_on: Option<DateTime<Utc>>,
    pub url: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Comment {
    pub id: String,
    pub url: String,
    pub author: Option<Actor>,
    pub author_association: Option<String>,
    pub body: String,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
    pub reactions: Vec<Reaction>,
    pub metadata: Vec<MetadataField>,
    pub replies: Vec<Comment>,
    pub is_answer: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Review {
    pub id: String,
    pub url: String,
    pub author: Option<Actor>,
    pub author_association: Option<String>,
    pub state: String,
    pub body: String,
    pub submitted_at: Option<DateTime<Utc>>,
    pub commit_id: Option<String>,
    pub reactions: Vec<Reaction>,
}

#[derive(Clone, Debug, Default)]
pub struct ReviewComment {
    pub id: String,
    pub url: String,
    pub author: Option<Actor>,
    pub author_association: Option<String>,
    pub body: String,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
    pub path: Option<String>,
    pub line: Option<i64>,
    pub start_line: Option<i64>,
    pub diff_hunk: Option<String>,
    pub in_reply_to: Option<String>,
    pub review_id: Option<String>,
    pub is_minimized: Option<bool>,
    pub minimized_reason: Option<String>,
    pub reactions: Vec<Reaction>,
}

#[derive(Clone, Debug, Default)]
pub struct ReviewThread {
    pub id: String,
    pub path: Option<String>,
    pub is_resolved: Option<bool>,
    pub is_outdated: Option<bool>,
    pub comments: Vec<ReviewComment>,
}

#[derive(Clone, Debug, Default)]
pub struct SourceIssue {
    pub number: u64,
    pub title: String,
    pub url: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct TimelineEntry {
    pub event_type: String,
    pub actor: Option<Actor>,
    pub created_at: Option<DateTime<Utc>>,
    pub body: Option<String>,
    pub commit_author: Option<CommitAuthor>,
    pub source_issue: Option<SourceIssue>,
    pub details: Vec<MetadataField>,
    pub files: Vec<ChangedFile>,
}

#[derive(Clone, Debug, Default)]
pub struct CommitAuthor {
    pub name: Option<String>,
    pub email: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct ChangedFile {
    pub sha: String,
    pub path: String,
    pub status: String,
    pub additions: i64,
    pub deletions: i64,
    pub changes: i64,
    pub previous_path: Option<String>,
    pub blob_url: Option<String>,
    pub patch: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct CommitSummary {
    pub sha: String,
    pub url: Option<String>,
    pub message: String,
    pub author_name: Option<String>,
    pub authored_at: Option<DateTime<Utc>>,
    pub author_user: Option<Actor>,
    pub files: Vec<ChangedFile>,
}

#[derive(Clone, Debug, Default)]
pub struct RawGraphQlRequest {
    pub query: String,
    pub variables: Value,
}

#[derive(Clone, Debug, Default)]
pub struct RawPayload {
    pub name: String,
    pub payload: Value,
    pub request_urls: Vec<String>,
    pub graphql_requests: Vec<RawGraphQlRequest>,
}

#[derive(Clone, Debug)]
pub struct ExportDocument {
    pub kind: ResourceKind,
    pub owner: String,
    pub repo: String,
    pub number: u64,
    pub title: String,
    pub url: String,
    pub state: String,
    pub body: String,
    pub author: Option<Actor>,
    pub author_association: Option<String>,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
    pub closed_at: Option<DateTime<Utc>>,
    pub labels: Vec<Label>,
    pub assignees: Vec<Actor>,
    pub requested_reviewers: Vec<Actor>,
    pub reactions: Vec<Reaction>,
    pub metadata: Vec<MetadataField>,
    pub milestone: Option<Milestone>,
    pub comments: Vec<Comment>,
    pub reviews: Vec<Review>,
    pub review_threads: Vec<ReviewThread>,
    pub timeline: Vec<TimelineEntry>,
    pub files: Vec<ChangedFile>,
    pub commits: Vec<CommitSummary>,
    pub raw_payloads: Vec<RawPayload>,
}

impl Default for ExportDocument {
    fn default() -> Self {
        Self {
            kind: crate::cli::ResourceKind::Issue,
            owner: String::new(),
            repo: String::new(),
            number: 0,
            title: String::new(),
            url: String::new(),
            state: String::new(),
            body: String::new(),
            author: None,
            author_association: None,
            created_at: None,
            updated_at: None,
            closed_at: None,
            labels: Vec::new(),
            assignees: Vec::new(),
            requested_reviewers: Vec::new(),
            reactions: Vec::new(),
            metadata: Vec::new(),
            milestone: None,
            comments: Vec::new(),
            reviews: Vec::new(),
            review_threads: Vec::new(),
            timeline: Vec::new(),
            files: Vec::new(),
            commits: Vec::new(),
            raw_payloads: Vec::new(),
        }
    }
}
