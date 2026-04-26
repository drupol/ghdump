![GitHub stars][github stars]
[![Donate!][donate github]][5]

# ghdump

`ghdump` is a Rust CLI that exports a GitHub issue, pull request, or discussion through customisable templates.

The tool expects a GitHub URL as its only argument and detects the resource type automatically from that URL.
By default it renders Markdown, but the same context can also be rendered as plain text, YAML, TOML, or JSON through an appropriate template.

The template format is based on [MiniJinja] and the default template is stored in `templates/default.md.j2`. The exported context includes all the data returned by the GitHub REST and GraphQL APIs, as well as some additional metadata about the export process itself.

The user is free to design their own templates. A default template is included in the repository as an exhaustive reference that demonstrates the full available context (comments, reviews, reactions, metadata, raw payloads, and more).

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

## Template Variables

The template context contains the following variables (as serialized by `--dump-context`).

`null` means the value is optional and not always present depending on the resource type or API response.

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
- **counts** (count object)

</details>

<details>

<summary>Shared Object Shapes</summary>

- **actor**:
  - **login** (string)
  - **url** (string or `null`)
  - **name** (string or `null`)
  - **email** (string or `null`)

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
  - **reactions** (array of reaction)

</details>

<details>

<summary>Timeline Events</summary>

- **timeline-entry**:
  - **event_type** (string)
  - **actor** (actor or `null`)
  - **created_at** (RFC 3339 string or `null`)
  - **body** (string or `null`)
  - **commit_author** (actor or `null`)
  - **details** (array of metadata-field)

</details>

<details>

<summary>Files & Commits (Pull Requests Only)</summary>

- **changed-file**:
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

</details>

<details>

<summary>Raw API Payloads</summary>

- **raw-payload**:
  - **name** (string)
  - **request_urls** (array of strings, deduplicated while preserving order)
  - **request_count** (integer, original total before deduplication)
  - **graphql_requests** (array of GraphQL-request)
  - **payload** (JSON object)

- **GraphQL-request**:
  - **query** (string)
  - **variables** (JSON object)

</details>

<details>

<summary>Counts</summary>

- **counts**:
  - **comments** (integer)
  - **reviews** (integer)
  - **review_threads** (integer)
  - **timeline** (integer)
  - **files** (integer)
  - **commits** (integer)
  - **raw_payloads** (integer)

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

[github stars]: https://img.shields.io/github/stars/drupol/ghdump.svg?style=flat-square
[donate github]: https://img.shields.io/badge/Sponsor-Github-brightgreen.svg?style=flat-square
[5]: https://github.com/sponsors/drupol
[`ghdump` package]: https://search.nixos.org/packages?channel=unstable&from=0&size=50&sort=relevance&type=packages&query=ghdump
[MiniJinja]: https://docs.rs/minijinja/
