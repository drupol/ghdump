use std::{borrow::Cow, collections::HashSet, fs, path::Path};

use anyhow::Context;
use chrono::{DateTime, Utc};
use minijinja::Environment;
use serde::Serialize;
use serde_json::Value;

use crate::model::{
    Actor, ChangedFile, Comment, CommitAuthor, CommitSummary, ExportDocument, Label, MetadataField,
    Milestone, RawGraphQlRequest, RawPayload, Reaction, Review, ReviewComment, ReviewThread,
    TimelineEntry,
};

const DEFAULT_TEMPLATE_NAME: &str = "document.md.j2";
const BUILTIN_TEMPLATE_NAME: &str = "default.md.j2";
const DEFAULT_TEMPLATE_SOURCE: &str = include_str!("../templates/default.md.j2");

pub fn render(document: &ExportDocument, template_path: Option<&Path>) -> anyhow::Result<String> {
    let context = TemplateContext::from_document(document);
    render_context(&context, template_path)
}

pub fn dump_context(document: &ExportDocument) -> anyhow::Result<String> {
    let context = TemplateContext::from_document(document);
    serde_json::to_string_pretty(&context).context("failed to serialize template context")
}

fn render_context(
    context: &TemplateContext,
    template_path: Option<&Path>,
) -> anyhow::Result<String> {
    let mut environment = Environment::new();
    let template_source = match template_path {
        Some(path) => Cow::Owned(
            fs::read_to_string(path)
                .with_context(|| format!("failed to read template {}", path.display()))?,
        ),
        None => Cow::Borrowed(DEFAULT_TEMPLATE_SOURCE),
    };

    environment
        .add_template(BUILTIN_TEMPLATE_NAME, DEFAULT_TEMPLATE_SOURCE)
        .context("failed to load built-in template")?;

    let template_name = if template_path.is_some() {
        environment
            .add_template(DEFAULT_TEMPLATE_NAME, template_source.as_ref())
            .context("failed to load template")?;
        DEFAULT_TEMPLATE_NAME
    } else {
        BUILTIN_TEMPLATE_NAME
    };

    let template = environment
        .get_template(template_name)
        .context("failed to compile template")?;

    template
        .render(context)
        .context("failed to render template")
}

#[derive(Debug, Serialize)]
struct TemplateContext {
    kind: String,
    owner: String,
    repo: String,
    number: u64,
    title: String,
    url: String,
    state: String,
    body: String,
    author: Option<ActorContext>,
    author_association: Option<String>,
    created_at: Option<String>,
    updated_at: Option<String>,
    closed_at: Option<String>,
    labels: Vec<LabelContext>,
    assignees: Vec<ActorContext>,
    requested_reviewers: Vec<ActorContext>,
    reactions: Vec<ReactionContext>,
    metadata: Vec<MetadataFieldContext>,
    milestone: Option<MilestoneContext>,
    comments: Vec<CommentContext>,
    reviews: Vec<ReviewContext>,
    review_threads: Vec<ReviewThreadContext>,
    timeline: Vec<TimelineEntryContext>,
    files: Vec<ChangedFileContext>,
    commits: Vec<CommitContext>,
    raw_payloads: Vec<RawPayloadContext>,
    merged_at: Option<String>,
    draft: bool,
}

#[derive(Clone, Debug, Serialize)]
struct ActorContext {
    login: String,
    url: Option<String>,
    name: Option<String>,
    email: Option<String>,
}

#[derive(Debug, Serialize)]
struct LabelContext {
    name: String,
    color: Option<String>,
    description: Option<String>,
}

#[derive(Debug, Serialize)]
struct ReactionContext {
    content: String,
    count: u64,
    users: Vec<ActorContext>,
}

#[derive(Debug, Serialize)]
struct MetadataFieldContext {
    name: String,
    value: String,
}

#[derive(Debug, Serialize)]
struct MilestoneContext {
    title: String,
    state: Option<String>,
    due_on: Option<String>,
    url: Option<String>,
}

#[derive(Debug, Serialize)]
struct CommentContext {
    level: usize,
    ordinal: usize,
    id: String,
    url: String,
    author: Option<ActorContext>,
    author_association: Option<String>,
    body: String,
    created_at: Option<String>,
    updated_at: Option<String>,
    reactions: Vec<ReactionContext>,
    metadata: Vec<MetadataFieldContext>,
    replies: Vec<CommentContext>,
    is_answer: bool,
}

#[derive(Debug, Serialize)]
struct ReviewContext {
    id: String,
    url: String,
    author: Option<ActorContext>,
    author_association: Option<String>,
    state: String,
    body: String,
    submitted_at: Option<String>,
    commit_id: Option<String>,
    reactions: Vec<ReactionContext>,
}

#[derive(Debug, Serialize)]
struct ReviewThreadContext {
    ordinal: usize,
    id: String,
    path: Option<String>,
    is_resolved: Option<bool>,
    is_outdated: Option<bool>,
    comments: Vec<ReviewCommentContext>,
}

#[derive(Debug, Serialize)]
struct ReviewCommentContext {
    id: String,
    url: String,
    author: Option<ActorContext>,
    author_association: Option<String>,
    body: String,
    created_at: Option<String>,
    updated_at: Option<String>,
    path: Option<String>,
    line: Option<i64>,
    start_line: Option<i64>,
    diff_hunk: Option<String>,
    in_reply_to: Option<String>,
    review_id: Option<String>,
    is_minimized: Option<bool>,
    minimized_reason: Option<String>,
    reactions: Vec<ReactionContext>,
}

#[derive(Debug, Serialize)]
struct SourceIssueContext {
    pub number: u64,
    pub title: String,
    pub url: Option<String>,
}

#[derive(Debug, Serialize)]
struct TimelineEntryContext {
    event_type: String,
    actor: Option<ActorContext>,
    created_at: Option<String>,
    body: Option<String>,
    commit_author: Option<ActorContext>,
    source_issue: Option<SourceIssueContext>,
    details: Vec<MetadataFieldContext>,
    files: Vec<ChangedFileContext>,
}

#[derive(Debug, Serialize)]
struct ChangedFileContext {
    sha: String,
    path: String,
    status: String,
    additions: i64,
    deletions: i64,
    changes: i64,
    previous_path: Option<String>,
    blob_url: Option<String>,
    patch: Option<String>,
}

#[derive(Debug, Serialize)]
struct CommitContext {
    sha: String,
    url: Option<String>,
    message: String,
    author_name: Option<String>,
    authored_at: Option<String>,
    author_user: Option<ActorContext>,
    files: Vec<ChangedFileContext>,
}

#[derive(Debug, Serialize)]
struct RawPayloadContext {
    name: String,
    request_urls: Vec<String>,
    request_count: usize,
    graphql_requests: Vec<GraphQlRequestContext>,
    payload: Value,
}

#[derive(Debug, Serialize)]
struct GraphQlRequestContext {
    query: String,
    variables: Value,
}

impl TemplateContext {
    fn from_document(document: &ExportDocument) -> Self {
        let author = document.author.as_ref().map(ActorContext::from_actor);

        Self {
            kind: document.kind.to_string(),
            owner: document.owner.clone(),
            repo: document.repo.clone(),
            number: document.number,
            title: document.title.clone(),
            url: document.url.clone(),
            state: document.state.clone(),
            body: document.body.clone(),
            author,
            author_association: document.author_association.clone(),
            created_at: format_optional_datetime(document.created_at.as_ref()),
            updated_at: format_optional_datetime(document.updated_at.as_ref()),
            closed_at: format_optional_datetime(document.closed_at.as_ref()),
            labels: document
                .labels
                .iter()
                .map(LabelContext::from_label)
                .collect(),
            assignees: document
                .assignees
                .iter()
                .map(ActorContext::from_actor)
                .collect(),
            requested_reviewers: document
                .requested_reviewers
                .iter()
                .map(ActorContext::from_actor)
                .collect(),
            reactions: document
                .reactions
                .iter()
                .map(ReactionContext::from_reaction)
                .collect(),
            metadata: document
                .metadata
                .iter()
                .map(MetadataFieldContext::from_field)
                .collect(),
            milestone: document
                .milestone
                .as_ref()
                .map(MilestoneContext::from_milestone),
            comments: document
                .comments
                .iter()
                .enumerate()
                .map(|(index, comment)| CommentContext::from_comment(comment, 3, index + 1))
                .collect(),
            reviews: document
                .reviews
                .iter()
                .map(ReviewContext::from_review)
                .collect(),
            review_threads: document
                .review_threads
                .iter()
                .enumerate()
                .map(|(index, thread)| ReviewThreadContext::from_thread(thread, index + 1))
                .collect(),
            timeline: document
                .timeline
                .iter()
                .map(TimelineEntryContext::from_timeline_entry)
                .collect(),
            files: document
                .files
                .iter()
                .map(ChangedFileContext::from_changed_file)
                .collect(),
            commits: document
                .commits
                .iter()
                .map(CommitContext::from_commit)
                .collect(),
            raw_payloads: document
                .raw_payloads
                .iter()
                .map(RawPayloadContext::from_raw_payload)
                .collect(),
            merged_at: document
                .metadata
                .iter()
                .find(|f| f.name == "Merged at")
                .map(|f| f.value.clone()),
            draft: document.draft,
        }
    }
}

impl ActorContext {
    fn from_actor(actor: &Actor) -> Self {
        Self {
            login: actor.login.clone(),
            url: actor.url.clone(),
            name: None,
            email: None,
        }
    }

    fn from_commit_author(author: Option<&CommitAuthor>) -> Option<Self> {
        let name = author.and_then(|a| a.name.as_deref());
        let email = author.and_then(|a| a.email.as_deref());
        let login = name.or(email)?;

        Some(Self {
            login: login.to_owned(),
            url: None,
            name: name.map(str::to_owned),
            email: email.map(str::to_owned),
        })
    }
}

impl LabelContext {
    fn from_label(label: &Label) -> Self {
        Self {
            name: label.name.clone(),
            color: label.color.clone(),
            description: label.description.clone(),
        }
    }
}

impl ReactionContext {
    fn from_reaction(reaction: &Reaction) -> Self {
        Self {
            content: reaction.content.clone(),
            count: reaction.count,
            users: reaction
                .users
                .iter()
                .map(ActorContext::from_actor)
                .collect(),
        }
    }
}

impl MetadataFieldContext {
    fn from_field(field: &MetadataField) -> Self {
        Self {
            name: field.name.clone(),
            value: field.value.clone(),
        }
    }
}

impl MilestoneContext {
    fn from_milestone(milestone: &Milestone) -> Self {
        Self {
            title: milestone.title.clone(),
            state: milestone.state.clone(),
            due_on: format_optional_datetime(milestone.due_on.as_ref()),
            url: milestone.url.clone(),
        }
    }
}

impl CommentContext {
    fn from_comment(comment: &Comment, level: usize, ordinal: usize) -> Self {
        let author = comment.author.as_ref().map(ActorContext::from_actor);

        Self {
            level,
            ordinal,
            id: comment.id.clone(),
            url: comment.url.clone(),
            author,
            author_association: comment.author_association.clone(),
            body: comment.body.clone(),
            created_at: format_optional_datetime(comment.created_at.as_ref()),
            updated_at: format_optional_datetime(comment.updated_at.as_ref()),
            reactions: comment
                .reactions
                .iter()
                .map(ReactionContext::from_reaction)
                .collect(),
            metadata: comment
                .metadata
                .iter()
                .map(MetadataFieldContext::from_field)
                .collect(),
            replies: comment
                .replies
                .iter()
                .enumerate()
                .map(|(index, reply)| CommentContext::from_comment(reply, level + 1, index + 1))
                .collect(),
            is_answer: comment.is_answer,
        }
    }
}

impl ReviewContext {
    fn from_review(review: &Review) -> Self {
        let author = review.author.as_ref().map(ActorContext::from_actor);

        Self {
            id: review.id.clone(),
            url: review.url.clone(),
            author,
            author_association: review.author_association.clone(),
            state: review.state.clone(),
            body: review.body.clone(),
            submitted_at: format_optional_datetime(review.submitted_at.as_ref()),
            commit_id: review.commit_id.clone(),
            reactions: review
                .reactions
                .iter()
                .map(ReactionContext::from_reaction)
                .collect(),
        }
    }
}

impl ReviewThreadContext {
    fn from_thread(thread: &ReviewThread, ordinal: usize) -> Self {
        Self {
            ordinal,
            id: thread.id.clone(),
            path: thread.path.clone(),
            is_resolved: thread.is_resolved,
            is_outdated: thread.is_outdated,
            comments: thread
                .comments
                .iter()
                .map(ReviewCommentContext::from_review_comment)
                .collect(),
        }
    }
}

impl ReviewCommentContext {
    fn from_review_comment(comment: &ReviewComment) -> Self {
        let author = comment.author.as_ref().map(ActorContext::from_actor);

        Self {
            id: comment.id.clone(),
            url: comment.url.clone(),
            author,
            author_association: comment.author_association.clone(),
            body: comment.body.clone(),
            created_at: format_optional_datetime(comment.created_at.as_ref()),
            updated_at: format_optional_datetime(comment.updated_at.as_ref()),
            path: comment.path.clone(),
            line: comment.line,
            start_line: comment.start_line,
            diff_hunk: comment.diff_hunk.clone(),
            in_reply_to: comment.in_reply_to.clone(),
            review_id: comment.review_id.clone(),
            is_minimized: comment.is_minimized,
            minimized_reason: comment.minimized_reason.clone(),
            reactions: comment
                .reactions
                .iter()
                .map(ReactionContext::from_reaction)
                .collect(),
        }
    }
}

impl SourceIssueContext {
    fn from_source_issue(source: &crate::model::SourceIssue) -> Self {
        Self {
            number: source.number,
            title: source.title.clone(),
            url: source.url.clone(),
        }
    }
}

impl TimelineEntryContext {
    fn from_timeline_entry(entry: &TimelineEntry) -> Self {
        let actor = entry.actor.as_ref().map(ActorContext::from_actor);
        let commit_author = ActorContext::from_commit_author(entry.commit_author.as_ref());
        let source_issue = entry
            .source_issue
            .as_ref()
            .map(SourceIssueContext::from_source_issue);

        Self {
            event_type: entry.event_type.clone(),
            actor,
            created_at: format_optional_datetime(entry.created_at.as_ref()),
            body: entry.body.clone(),
            commit_author,
            source_issue,
            details: entry
                .details
                .iter()
                .map(MetadataFieldContext::from_field)
                .collect(),
            files: entry
                .files
                .iter()
                .map(ChangedFileContext::from_changed_file)
                .collect(),
        }
    }
}

impl ChangedFileContext {
    fn from_changed_file(file: &ChangedFile) -> Self {
        Self {
            sha: file.sha.clone(),
            path: file.path.clone(),
            status: file.status.clone(),
            additions: file.additions,
            deletions: file.deletions,
            changes: file.changes,
            previous_path: file.previous_path.clone(),
            blob_url: file.blob_url.clone(),
            patch: file.patch.clone(),
        }
    }
}

impl CommitContext {
    fn from_commit(commit: &CommitSummary) -> Self {
        Self {
            sha: commit.sha.clone(),
            url: commit.url.clone(),
            message: commit.message.clone(),
            author_name: commit.author_name.clone(),
            authored_at: format_optional_datetime(commit.authored_at.as_ref()),
            author_user: commit.author_user.as_ref().map(ActorContext::from_actor),
            files: commit
                .files
                .iter()
                .map(ChangedFileContext::from_changed_file)
                .collect(),
        }
    }
}

impl RawPayloadContext {
    fn from_raw_payload(payload: &RawPayload) -> Self {
        Self {
            name: payload.name.clone(),
            request_urls: deduplicate_strings_preserve_order(&payload.request_urls),
            request_count: payload.request_urls.len(),
            graphql_requests: payload
                .graphql_requests
                .iter()
                .map(GraphQlRequestContext::from_graphql_request)
                .collect(),
            payload: payload.payload.clone(),
        }
    }
}

impl GraphQlRequestContext {
    fn from_graphql_request(request: &RawGraphQlRequest) -> Self {
        Self {
            query: request.query.clone(),
            variables: request.variables.clone(),
        }
    }
}

fn format_optional_datetime(value: Option<&DateTime<Utc>>) -> Option<String> {
    value.map(DateTime::to_rfc3339)
}

fn deduplicate_strings_preserve_order(values: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut deduplicated = Vec::new();

    for value in values {
        if seen.insert(value.clone()) {
            deduplicated.push(value.clone());
        }
    }

    deduplicated
}

#[cfg(test)]
mod tests {
    use super::{dump_context, render};
    use std::fs;

    use crate::{
        cli::ResourceKind,
        model::{
            Actor, ChangedFile, Comment, CommitSummary, ExportDocument, Label, MetadataField,
            RawGraphQlRequest, RawPayload, Reaction, Review, ReviewComment, ReviewThread,
        },
    };
    use serde_json::json;

    fn sample_document() -> ExportDocument {
        let reply = Comment {
            id: "reply-1".to_owned(),
            url: "https://example.com/reply".to_owned(),
            author: Some(Actor {
                login: "reply-user".to_owned(),
                url: Some("https://github.com/reply-user".to_owned()),
            }),
            body: "Nested reply".to_owned(),
            reactions: vec![Reaction {
                content: "heart".to_owned(),
                count: 1,
                users: vec![Actor {
                    login: "reply-user".to_owned(),
                    url: Some("https://github.com/reply-user".to_owned()),
                }],
            }],
            ..Comment::default()
        };

        let comment = Comment {
            id: "comment-1".to_owned(),
            url: "https://example.com/comment".to_owned(),
            author: Some(Actor {
                login: "octocat".to_owned(),
                url: Some("https://github.com/octocat".to_owned()),
            }),
            author_association: Some("OWNER".to_owned()),
            body: "Top-level comment".to_owned(),
            metadata: vec![MetadataField {
                name: "Source".to_owned(),
                value: "Unit test".to_owned(),
            }],
            reactions: vec![Reaction {
                content: "+1".to_owned(),
                count: 2,
                users: vec![Actor {
                    login: "octocat".to_owned(),
                    url: Some("https://github.com/octocat".to_owned()),
                }],
            }],
            replies: vec![reply],
            ..Comment::default()
        };

        let review_comment = ReviewComment {
            id: "review-comment-1".to_owned(),
            url: "https://example.com/review-comment".to_owned(),
            body: "Please rename this.".to_owned(),
            diff_hunk: Some("@@ -1 +1 @@".to_owned()),
            reactions: vec![Reaction {
                content: "eyes".to_owned(),
                count: 4,
                users: vec![Actor {
                    login: "reviewer".to_owned(),
                    url: Some("https://github.com/reviewer".to_owned()),
                }],
            }],
            ..ReviewComment::default()
        };

        ExportDocument {
            kind: ResourceKind::Issue,
            owner: "octocat".to_owned(),
            repo: "Hello-World".to_owned(),
            number: 42,
            title: "Improve export templates".to_owned(),
            url: "https://github.com/octocat/Hello-World/issues/42".to_owned(),
            state: "open".to_owned(),
            body: "Main body".to_owned(),
            labels: vec![
                Label {
                    name: "bug".to_owned(),
                    ..Label::default()
                },
                Label {
                    name: "enhancement".to_owned(),
                    ..Label::default()
                },
            ],
            metadata: vec![
                MetadataField {
                    name: "Comments".to_owned(),
                    value: "1".to_owned(),
                },
                MetadataField {
                    name: "Merged at".to_owned(),
                    value: "2024-04-28T10:00:00Z".to_owned(),
                },
            ],
            reactions: vec![
                Reaction {
                    content: "+1".to_owned(),
                    count: 2,
                    users: vec![Actor {
                        login: "octocat".to_owned(),
                        url: Some("https://github.com/octocat".to_owned()),
                    }],
                },
                Reaction {
                    content: "rocket".to_owned(),
                    count: 1,
                    users: vec![Actor {
                        login: "hubot".to_owned(),
                        url: Some("https://github.com/hubot".to_owned()),
                    }],
                },
            ],
            comments: vec![comment],
            reviews: vec![Review {
                id: "review-1".to_owned(),
                url: "https://example.com/review".to_owned(),
                state: "COMMENTED".to_owned(),
                body: "Review body".to_owned(),
                ..Review::default()
            }],
            review_threads: vec![ReviewThread {
                id: "thread-1".to_owned(),
                comments: vec![review_comment],
                ..ReviewThread::default()
            }],
            raw_payloads: vec![RawPayload {
                name: "rest.issue".to_owned(),
                payload: json!({ "number": 42 }),
                request_urls: vec![
                    "https://api.github.com/repos/octocat/Hello-World/issues/42".to_owned(),
                ],
                graphql_requests: vec![RawGraphQlRequest {
                    query: "query Issue($number: Int!) { issue(number: $number) { id } }"
                        .to_owned(),
                    variables: json!({ "number": 42 }),
                }],
            }],
            commits: vec![CommitSummary {
                sha: "abc1234".to_owned(),
                message: "Add feature".to_owned(),
                files: vec![ChangedFile {
                    sha: "def5678".to_owned(),
                    path: "src/lib.rs".to_owned(),
                    status: "added".to_owned(),
                    ..ChangedFile::default()
                }],
                ..CommitSummary::default()
            }],
            ..ExportDocument::default()
        }
    }

    #[test]
    fn dump_context_serializes_expected_sections() {
        let context = dump_context(&sample_document()).expect("context should serialize");

        assert!(context.contains("\"kind\": \"issue\""));
        assert!(context.contains("\"owner\": \"octocat\""));
        assert!(context.contains("\"repo\": \"Hello-World\""));
        assert!(context.contains("\"title\": \"Improve export templates\""));
        assert!(context.contains("\"merged_at\": \"2024-04-28T10:00:00Z\""));
        assert!(context.contains("\"sha\": \"abc1234\""));
        assert!(context.contains("\"path\": \"src/lib.rs\""));
        assert!(!context.contains("\"header\""));
        assert!(!context.contains("kind_label"));
        assert!(!context.contains("repository"));
        assert!(!context.contains("body_or_placeholder"));
        assert!(!context.contains("status_summary"));
        assert!(!context.contains("\"emoji\""));
        assert!(!context.contains("pretty_json"));
        assert!(!context.contains("variables_pretty_json"));
        assert!(!context.contains("display_markdown"));
        assert!(!context.contains("author_display"));
    }

    #[test]
    fn renders_default_template_with_nested_comments() {
        let output = render(&sample_document(), None).expect("default template should render");

        assert!(output.contains("# Issue #42: Improve export templates"));
        assert!(output.contains("## Stats"));
        assert!(output.contains("- Comments: 1"));
        assert!(output.contains("- Reactions: 10"));
        assert!(output.contains("- Participants: 2"));
        assert!(output.contains("- Review threads: 1"));
        assert!(output.contains("- Unresolved threads: 0"));
        assert!(output.contains("```md\nMain body\n```"));
        assert!(output.contains("## Timeline (2)"));
        assert!(output.contains("### Comment 1 by [octocat](https://github.com/octocat)"));
        assert!(output.contains("- Source: Unit test"));
        assert!(output.contains("Top-level comment"));
        assert!(output.contains("#### Comment 1 by [reply-user](https://github.com/reply-user)"));
        assert!(output.contains("Nested reply"));
        assert!(output.contains("- Reactions:"));
        assert!(output.contains("  - 👍 `+1`: 2x"));
        assert!(output.contains("Review body"));
        assert!(output.contains("## Raw API Payloads"));
        assert!(output.contains("### Request URLs"));
        assert!(output.contains("https://api.github.com/repos/octocat/Hello-World/issues/42"));
        assert!(output.contains("### GraphQL Requests"));
        assert!(output.contains("query Issue($number: Int!)"));
        assert!(output.contains("- Labels:\n  - `bug`\n  - `enhancement`"));
        assert!(output.contains("\"number\": 42"));
    }

    #[test]
    fn custom_template_can_extend_default_template_blocks() {
        let path = std::env::temp_dir().join(format!(
            "ghdump-extend-template-{}.md.j2",
            std::process::id()
        ));
        fs::write(
            &path,
            "{% extends \"default.md.j2\" %}\n{% block stats %}{{ render_heading(2, \"Custom Stats\") }}- Labels: {{ labels | length }}\n{% endblock %}\n",
        )
        .expect("custom template should be writable");

        let output = render(&sample_document(), Some(path.as_path()))
            .expect("custom template should extend default template");
        let _ = fs::remove_file(&path);

        assert!(output.contains("# Issue #42: Improve export templates"));
        assert!(output.contains("## Custom Stats"));
        assert!(output.contains("- Labels: 2"));
        assert!(!output.contains("## Stats"));
        assert!(output.contains("## Description"));
    }

    #[test]
    fn deduplicates_request_urls_in_template_context() {
        let document = ExportDocument {
            raw_payloads: vec![RawPayload {
                name: "graphql.discussion_comments".to_owned(),
                payload: json!({ "ok": true }),
                request_urls: vec![
                    "https://api.github.com/graphql".to_owned(),
                    "https://api.github.com/graphql".to_owned(),
                    "https://api.github.com/graphql".to_owned(),
                ],
                graphql_requests: vec![RawGraphQlRequest {
                    query: "query Test { viewer { login } }".to_owned(),
                    variables: json!({}),
                }],
            }],
            ..ExportDocument::default()
        };

        let context = dump_context(&document).expect("context should serialize");

        assert!(context.contains("\"request_count\": 3"));
        assert_eq!(context.matches("https://api.github.com/graphql").count(), 1);
    }
}
