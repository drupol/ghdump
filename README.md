![GitHub stars][GitHub stars]
[![Crates.io Version][Crates.io Version]][ghdump crates]
[![Crates.io License][Crates.io License]][ghdump crates]
[![Donate!][Donate!]][sponsor link]

# ghdump

`ghdump` is a Rust CLI that exports a GitHub issue, pull request, or discussion through customisable templates.

The tool expects a GitHub URL as its only argument and detects the resource type automatically from that URL.
By default it renders Markdown, but the same context can also be rendered as plain text, YAML, TOML, or JSON through an appropriate template.

The template format is based on [MiniJinja] and the default template is stored in `templates/default.md.j2`. The exported context includes normalized data fetched from the GitHub REST and GraphQL APIs, plus raw payloads and export metadata.

The user is free to design their own templates. A default template is included in the repository as an exhaustive reference that demonstrates the available context (comments, reviews, reactions, metadata, raw payloads, and more).

For day-to-day use, it is recommended to create your own template focused on your specific workflow, rather than using the default template as-is.

## Installation

### Via Cargo

You can install the binary with Cargo:

```sh
cargo install ghdump
```

### Via Nixpkgs

Available via the [`ghdump` package], the binary is called `ghdump`.

### Via the source code

Clone the repository and run in the sourcecode folder:

```sh
cargo build --release
```

The binary will be in `target/release/ghdump`.

### Via Nix

You can use the package from this repository with Nix. If you have Nix installed, you can run the tool directly:

```sh
nix run github:drupol/ghdump
```

## Usage

Export an issue from its URL:

```sh
ghdump https://github.com/owner/repo/issues/123
```

Export a pull request to a file:

```sh
ghdump https://github.com/owner/repo/pull/456 --output pr-456.md
```

Export with a custom template:

```sh
ghdump https://github.com/owner/repo/pull/456 \
  --template ./my-template.md.j2 \
  --output pr-456.md
```

Export with a text or YAML-oriented template:

```sh
ghdump https://github.com/owner/repo/pull/456 \
  --template ./templates/llm.strict.md.j2 \
  --output pr-456.txt
```

Write the JSON context used by the template:

```sh
ghdump https://github.com/owner/repo/pull/456 \
  --dump-context ./pr-456.context.json \
  --output pr-456.md
```

Export a discussion explicitly:

```sh
GITHUB_TOKEN=... ghdump https://github.com/owner/repo/discussions/789
```

## Custom Templates

Templates are rendered with [MiniJinja]. A custom template can either be standalone or extend [built-in Markdown template (`default.md.j2`)][builtin template].

### Standalone template

Standalone templates receive the full context documented below:

```jinja
# {{ owner }}/{{ repo }} #{{ number }}: {{ title }}

State: {{ state }}
Author: {% if author %}{{ author.login }}{% else %}unknown{% endif %}
Labels: {{ labels | length }}
```

Use it with:

```sh
ghdump https://github.com/owner/repo/pull/456 \
  --template ./my-template.md.j2 \
  --output pr-456.md
```

### Extending the default template

The [built-in Markdown template (`default.md.j2`)][builtin template] is available to custom templates. This lets you override one section while reusing the rest:

<details>
<summary>Example: Custom Stats Section</summary>

```jinja
{% extends "default.md.j2" %}

{% block stats %}
## Custom Stats

- Comments: {{ comments | length }}
- Labels: {{ labels | length }}
- Files: {{ files | length }}
{% endblock %}
```

</details>

<details>
<summary>Example: Minimal Custom Template</summary>

```jinja
{% extends "default.md.j2" %}

{% block labels %}{% endblock %}
{% block stats %}{% endblock %}
{% block commits %}{% endblock %}
{% block milestone %}{% endblock %}
{% block files %}{% endblock %}
{% block raw_payloads %}{% endblock %}
```

</details>

The default template currently exposes these blocks:

- `document`: the whole rendered document.
- `header`: the top `# ...` heading.
- `metadata`: repository, URL, state, author, reviewers, labels, reactions, and related top-level metadata.
- `stats`: count summary.
- `checks`: CI summary (passing/failing/pending/cancelled) with detailed status/check-run entries (PR only).
- `participants`: list of unique participants with their roles (author, reviewer, commenter…).
- `reviews_summary`: compact review breakdown grouped by state (approved, changes-requested, commented, dismissed).
- `description`: issue, pull request, or discussion body.
- `milestone`: milestone section.
- `labels`: expanded labels section.
- `files`: changed files section.
- `commits`: commits section.
- `timeline`: chronological timeline rendered from comments, reviews, and timeline events.
- `raw_payloads`: raw REST and GraphQL payloads.
- `footer`: optional trailing section (empty by default).

Blocks can use the same variables and helper macros as the default template, such as `render_heading`, `render_field`, `render_actor`, `render_reactions`, `render_label`, `render_markdown_block`, `render_collapsible_section`, `render_diff_block`, and `render_raw_payload`.

## Template Variables

The template context contains the following variables (as serialized by `--dump-context`).

`null` means the value is optional and not always present depending on the resource type or API response.

Array counts can be computed directly in templates with MiniJinja's `length` filter, for example `{{ labels | length }}`.

<details>

<summary>Top-Level Fields</summary>

- **kind** (string): `"issue"`, `"pull-request"`, or `"discussion"`
- **owner** (string)
- **repo** (string)
- **number** (integer)
- **title** (string)
- **url** (string)
- **state** (string)
- **body** (string)
- **author** (actor object or `null`)
- **author_association** (string or `null`)
- **created_at** (RFC 3339 string or `null`)
- **updated_at** (RFC 3339 string or `null`)
- **closed_at** (RFC 3339 string or `null`)
- **state_reason** (string or `null`) — e.g. `completed`, `not_planned`, `reopened`
- **locked** (boolean)
- **active_lock_reason** (string or `null`)
- **draft** (boolean)
- **merged_at** (RFC 3339 string or `null`)
- **merged_by** (actor object or `null`)
- **merge_commit_sha** (string or `null`) — PR only
- **mergeable_state** (string or `null`) — PR only, e.g. `clean`, `dirty`, `blocked`
- **base** (branch-ref object or `null`) — PR only
- **head** (branch-ref object or `null`) — PR only
- **checks** (array of check-status objects) — PR only
- **labels** (array of label objects)
- **assignees** (array of actor objects)
- **requested_reviewers** (array of actor objects)
- **reactions** (array of reaction objects)
- **metadata** (array of metadata-field objects)
- **milestone** (milestone object or `null`)
- **comments** (array of comment objects)
- **reviews** (array of review objects)
- **review_threads** (array of review-thread objects)
- **timeline** (array of timeline-entry objects)
- **files** (array of changed-file objects)
- **commits** (array of commit objects)
- **raw_payloads** (array of raw-payload objects)

</details>

<details>

<summary>Shared Object Shapes</summary>

- **actor**:
  - **login** (string)
  - **url** (string or `null`)
  - **name** (string or `null`)
  - **email** (string or `null`)

- **branch-ref** (PR only):
  - **ref_name** (string) — branch name
  - **sha** (string or `null`) — tip commit SHA (available via REST path)
  - **repo_full_name** (string or `null`) — `owner/repo` (useful for forks)

- **check-status** (PR only):
  - **kind** (string) — `status` or `check`
  - **name** (string)
  - **status** (string)
  - **conclusion** (string or `null`)
  - **url** (string or `null`)
  - **description** (string or `null`)

- **metadata-field**:
  - **name** (string)
  - **value** (string)

- **reaction**:
  - **content** (string)
  - **count** (integer)
  - **users** (array of actor)

- **label**:
  - **name** (string)
  - **color** (string or `null`)
  - **description** (string or `null`)

- **milestone**:
  - **title** (string)
  - **state** (string or `null`)
  - **due_on** (RFC 3339 string or `null`)
  - **url** (string or `null`)

- **source-issue**:
  - **number** (integer)
  - **title** (string)
  - **url** (string or `null`)

</details>

<details>

<summary>Comments</summary>

- **comment**:
  - **level** (integer)
  - **ordinal** (integer)
  - **id** (string)
  - **url** (string)
  - **author** (actor or `null`)
  - **author_association** (string or `null`)
  - **body** (string)
  - **created_at** (RFC 3339 string or `null`)
  - **updated_at** (RFC 3339 string or `null`)
  - **reactions** (array of reaction)
  - **metadata** (array of metadata-field)
  - **replies** (array of comment)
  - **is_answer** (boolean)

</details>

<details>

<summary>Reviews & Review Threads (Pull Requests Only)</summary>

- **review**:
  - **id** (string)
  - **url** (string)
  - **author** (actor or `null`)
  - **author_association** (string or `null`)
  - **state** (string)
  - **body** (string)
  - **submitted_at** (RFC 3339 string or `null`)
  - **commit_id** (string or `null`)
  - **reactions** (array of reaction)

- **review-thread**:
  - **ordinal** (integer)
  - **id** (string)
  - **path** (string or `null`)
  - **is_resolved** (boolean or `null`)
  - **is_outdated** (boolean or `null`)
  - **comments** (array of review-comment)

- **review-comment**:
  - **id** (string)
  - **url** (string)
  - **author** (actor or `null`)
  - **author_association** (string or `null`)
  - **body** (string)
  - **created_at** (RFC 3339 string or `null`)
  - **updated_at** (RFC 3339 string or `null`)
  - **path** (string or `null`)
  - **line** (integer or `null`)
  - **start_line** (integer or `null`)
  - **diff_hunk** (string or `null`)
  - **in_reply_to** (string or `null`)
  - **review_id** (string or `null`)
  - **is_minimized** (boolean or `null`)
  - **minimized_reason** (string or `null`)
  - **reactions** (array of reaction)

</details>

<details>

<summary>Timeline Events</summary>

The `timeline` array contains GitHub timeline events normalized as timeline-entry objects. Top-level comments, reviews, and review threads are exposed through their own arrays. The default Markdown template merges `comments`, `reviews`, and non-duplicated `timeline` events into a single chronological section when rendering.

- **timeline-entry**:
  - **event_type** (string)
  - **actor** (actor or `null`)
  - **created_at** (RFC 3339 string or `null`)
  - **body** (string or `null`)
  - **commit_author** (actor or `null`)
  - **source_issue** (source-issue or `null`)
  - **details** (array of metadata-field)
  - **files** (array of changed-file)

</details>

<details>

<summary>Files & Commits (Pull Requests Only)</summary>

- **changed-file**:
  - **sha** (string)
  - **path** (string)
  - **status** (string)
  - **additions** (integer)
  - **deletions** (integer)
  - **changes** (integer)
  - **previous_path** (string or `null`)
  - **blob_url** (string or `null`)
  - **patch** (string or `null`)

- **commit**:
  - **sha** (string)
  - **url** (string or `null`)
  - **message** (string)
  - **author_name** (string or `null`)
  - **authored_at** (RFC 3339 string or `null`)
  - **author_user** (actor or `null`)
  - **files** (array of changed-file)

</details>

<details>

<summary>Raw API Payloads</summary>

- **raw-payload**:
  - **name** (string)
  - **request_urls** (array of strings, deduplicated while preserving order)
  - **request_count** (integer, original total before deduplication)
  - **graphql_requests** (array of GraphQL-request)
  - **payload** (JSON value)

- **GraphQL-request**:
  - **query** (string)
  - **variables** (JSON object)

</details>

<details>

<summary>Computing Counts</summary>

Use MiniJinja's `length` filter against the arrays already in the context:

- `{{ comments | length }}`
- `{{ reviews | length }}`
- `{{ review_threads | length }}`
- `{{ timeline | length }}`
- `{{ labels | length }}`
- `{{ files | length }}`
- `{{ commits | length }}`
- `{{ raw_payloads | length }}`

Grouped reactions expose per-type `count` values. To compute a total across all reaction types, sum `reaction.count` across the top-level `reactions` array plus any `review.reactions`, `comment.reactions`, nested reply reactions, and `review_threads[].comments[].reactions` you want to include.

</details>

## Tests

The default test suite is designed to run without network access. Rendering and transformation tests rely on local JSON fixtures stored in `fixtures/`.

```sh
cargo test
```

In an environment where Cargo dependencies are already available, you can also verify the offline mode explicitly:

```sh
cargo test --offline
```

## Notes

- The default template is stored in `templates/default.md.j2`, produces Markdown output, and is intentionally exhaustive to showcase all available data.
- For practical usage, prefer creating your own template tailored to the information you want to keep.
- Raw API payloads remain available in the exported context and the template decides whether to render them.
- Each `Raw API Payloads` block includes the actual URLs used to fetch the corresponding data, as well as the GraphQL requests when applicable.
- The file produced by `--dump-context` matches exactly the context injected into `MiniJinja`, which makes it useful for designing and debugging templates without changing the Rust code.

[GitHub stars]: https://img.shields.io/github/stars/drupol/ghdump.svg?style=flat-square
[Donate!]: https://img.shields.io/badge/Sponsor-Github-brightgreen.svg?style=flat-square
[sponsor link]: https://github.com/sponsors/drupol
[`ghdump` package]: https://search.nixos.org/packages?channel=unstable&from=0&size=50&sort=relevance&type=packages&query=ghdump
[MiniJinja]: https://docs.rs/minijinja/
[Crates.io License]: https://img.shields.io/crates/l/ghdump?style=flat-square
[Crates.io Version]: https://img.shields.io/crates/v/ghdump?style=flat-square
[ghdump crates]: https://crates.io/crates/ghdump
[builtin template]: https://github.com/drupol/ghdump/blob/main/templates/default.md.j2
