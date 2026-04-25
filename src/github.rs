use anyhow::{bail, Context, Result};
use octocrab::Octocrab;
use serde::Deserialize;

pub struct PrFile {
    pub path: String,
    pub previous_path: Option<String>,
    pub status: String,
}

pub struct PrMetadata {
    pub pr_number: u64,
    /// GraphQL node ID — required for mutations like `markFileAsViewed`.
    pub node_id: String,
    pub title: String,
    pub base_branch: String,
    pub base_sha: String,
    pub head_branch: String,
    pub head_sha: String,
    pub files: Vec<PrFile>,
}

impl PrMetadata {
    pub fn pr_number(&self) -> u64 {
        self.pr_number
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommentSide {
    Base, // GraphQL "LEFT"
    Head, // GraphQL "RIGHT"
}

#[derive(Debug, Clone)]
pub struct ReviewComment {
    pub author: String,
    pub body: String,
    /// Pre-formatted "YYYY-MM-DD HH:MM".
    pub created_at: String,
    /// True when the comment is part of an unsubmitted review (GraphQL `state == "PENDING"`).
    pub is_pending: bool,
}

#[allow(dead_code)] // `id`, `is_resolved`, `is_outdated` are reserved for M9 / future UI.
#[derive(Debug, Clone)]
pub struct CommentThread {
    /// Thread node ID — used for `addPullRequestReviewThreadReply` (M9).
    pub id: String,
    pub path: String,
    pub side: CommentSide,
    pub line: u32,
    pub is_resolved: bool,
    pub is_outdated: bool,
    /// Root comment first, replies in order.
    pub comments: Vec<ReviewComment>,
}

/// Fetch the PR's metadata, file list, and review threads in a single GraphQL round trip
/// (with pagination follow-ups for files / threads only when necessary).
///
/// File list does NOT include `previous_path` for renames — GraphQL doesn't expose it.
/// Detect renames locally with `git::detect_renames` and merge into `metadata.files`.
pub async fn fetch_pr(
    token: &str,
    owner: &str,
    repo: &str,
    pr_number: u64,
) -> Result<(PrMetadata, Vec<CommentThread>)> {
    let octocrab = Octocrab::builder()
        .personal_token(token.to_owned())
        .build()
        .context("failed to build GitHub client")?;

    let response: GqlResponse<PrQueryData> = run_graphql(
        &octocrab,
        PR_QUERY,
        serde_json::json!({
            "owner": owner,
            "name": repo,
            "number": pr_number,
        }),
    )
    .await
    .with_context(|| format!("failed to fetch PR #{pr_number} from {owner}/{repo}"))?;

    let pr = response
        .into_data()?
        .repository
        .pull_request
        .context("GraphQL returned null pullRequest")?;

    let mut gql_files = pr.files.nodes;
    let mut files_page = pr.files.page_info;
    while files_page.has_next_page {
        let cursor = files_page
            .end_cursor
            .clone()
            .context("hasNextPage but missing endCursor")?;
        let next: GqlResponse<FilesPageData> = run_graphql(
            &octocrab,
            FILES_PAGE_QUERY,
            serde_json::json!({
                "owner": owner,
                "name": repo,
                "number": pr_number,
                "cursor": cursor,
            }),
        )
        .await
        .with_context(|| format!("failed to paginate files for PR #{pr_number}"))?;
        let conn = next
            .into_data()?
            .repository
            .pull_request
            .context("GraphQL returned null pullRequest while paginating files")?
            .files;
        gql_files.extend(conn.nodes);
        files_page = conn.page_info;
    }

    let mut gql_threads = pr.review_threads.nodes;
    let mut threads_page = pr.review_threads.page_info;
    while threads_page.has_next_page {
        let cursor = threads_page
            .end_cursor
            .clone()
            .context("hasNextPage but missing endCursor")?;
        let next: GqlResponse<ThreadsPageData> = run_graphql(
            &octocrab,
            THREADS_PAGE_QUERY,
            serde_json::json!({
                "owner": owner,
                "name": repo,
                "number": pr_number,
                "cursor": cursor,
            }),
        )
        .await
        .with_context(|| format!("failed to paginate threads for PR #{pr_number}"))?;
        let conn = next
            .into_data()?
            .repository
            .pull_request
            .context("GraphQL returned null pullRequest while paginating threads")?
            .review_threads;
        gql_threads.extend(conn.nodes);
        threads_page = conn.page_info;
    }

    let files = gql_files
        .into_iter()
        .map(|f| PrFile {
            path: f.path,
            previous_path: None,
            status: status_from_change_type(&f.change_type).to_owned(),
        })
        .collect();

    let metadata = PrMetadata {
        pr_number,
        node_id: pr.id,
        title: pr.title,
        base_branch: pr.base_ref_name,
        base_sha: pr.base_ref_oid,
        head_branch: pr.head_ref_name,
        head_sha: pr.head_ref_oid,
        files,
    };

    let threads = gql_threads
        .into_iter()
        .filter_map(|t| {
            let line = t.line? as u32;
            let side = match t.diff_side.as_str() {
                "LEFT" => CommentSide::Base,
                "RIGHT" => CommentSide::Head,
                _ => return None,
            };
            let comments = t
                .comments
                .nodes
                .into_iter()
                .map(|c| ReviewComment {
                    author: c
                        .author
                        .map(|a| a.login)
                        .unwrap_or_else(|| "unknown".to_string()),
                    body: c.body,
                    created_at: c.created_at.format("%Y-%m-%d %H:%M").to_string(),
                    is_pending: c.state == "PENDING",
                })
                .collect();
            Some(CommentThread {
                id: t.id,
                path: t.path,
                side,
                line,
                is_resolved: t.is_resolved,
                is_outdated: t.is_outdated,
                comments,
            })
        })
        .collect();

    Ok((metadata, threads))
}

/// Post a new review thread anchored to a single line on a PR diff.
/// Uses the `addPullRequestReviewThread` GraphQL mutation with `subjectType: LINE`.
pub async fn post_thread(
    token: &str,
    pr_node_id: &str,
    path: &str,
    side: CommentSide,
    line: u32,
    body: &str,
) -> Result<()> {
    let octocrab = Octocrab::builder()
        .personal_token(token.to_owned())
        .build()
        .context("failed to build GitHub client")?;

    let side_str = match side {
        CommentSide::Base => "LEFT",
        CommentSide::Head => "RIGHT",
    };

    let query = r#"
mutation($pid: ID!, $path: String!, $line: Int!, $side: DiffSide!, $body: String!) {
  addPullRequestReviewThread(input: {
    pullRequestId: $pid,
    path: $path,
    line: $line,
    side: $side,
    body: $body,
    subjectType: LINE
  }) {
    thread { id }
  }
}
"#;

    let payload = serde_json::json!({
        "query": query,
        "variables": {
            "pid": pr_node_id,
            "path": path,
            "line": line,
            "side": side_str,
            "body": body,
        }
    });

    let response: serde_json::Value = octocrab
        .graphql(&payload)
        .await
        .with_context(|| format!("addPullRequestReviewThread request failed for `{path}:{line}`"))?;

    if let Some(errors) = response.get("errors") {
        bail!("addPullRequestReviewThread for `{path}:{line}`: {errors}");
    }
    Ok(())
}

/// Reply to an existing review thread.
/// Uses the `addPullRequestReviewThreadReply` GraphQL mutation.
pub async fn reply_to_thread(token: &str, thread_id: &str, body: &str) -> Result<()> {
    let octocrab = Octocrab::builder()
        .personal_token(token.to_owned())
        .build()
        .context("failed to build GitHub client")?;

    let query = r#"
mutation($threadId: ID!, $body: String!) {
  addPullRequestReviewThreadReply(input: {
    pullRequestReviewThreadId: $threadId,
    body: $body
  }) {
    comment { id }
  }
}
"#;
    let payload = serde_json::json!({
        "query": query,
        "variables": { "threadId": thread_id, "body": body },
    });

    let response: serde_json::Value = octocrab
        .graphql(&payload)
        .await
        .with_context(|| {
            format!("addPullRequestReviewThreadReply request failed for thread {thread_id}")
        })?;

    if let Some(errors) = response.get("errors") {
        bail!("addPullRequestReviewThreadReply for thread {thread_id}: {errors}");
    }
    Ok(())
}

/// Mark or unmark a PR file as viewed via GitHub GraphQL.
pub async fn set_viewed(
    token: &str,
    pr_node_id: &str,
    file_path: &str,
    viewed: bool,
) -> Result<()> {
    let octocrab = Octocrab::builder()
        .personal_token(token.to_owned())
        .build()
        .context("failed to build GitHub client")?;

    let mutation = if viewed {
        "markFileAsViewed"
    } else {
        "unmarkFileAsViewed"
    };
    let query = format!(
        "mutation($pid: ID!, $path: String!) {{ \
            {mutation}(input: {{ pullRequestId: $pid, path: $path }}) {{ \
                clientMutationId \
            }} \
        }}"
    );
    let payload = serde_json::json!({
        "query": query,
        "variables": { "pid": pr_node_id, "path": file_path },
    });

    let response: serde_json::Value = octocrab
        .graphql(&payload)
        .await
        .with_context(|| format!("GraphQL {mutation} request failed for `{file_path}`"))?;

    if let Some(errors) = response.get("errors") {
        bail!("GraphQL {mutation} for `{file_path}`: {errors}");
    }
    Ok(())
}

fn status_from_change_type(ct: &str) -> &'static str {
    match ct {
        "ADDED" => "added",
        "DELETED" => "removed",
        "MODIFIED" => "modified",
        "RENAMED" => "renamed",
        "COPIED" => "copied",
        "CHANGED" => "changed",
        _ => "unknown",
    }
}

async fn run_graphql<R: serde::de::DeserializeOwned>(
    octocrab: &Octocrab,
    query: &str,
    variables: serde_json::Value,
) -> Result<GqlResponse<R>> {
    let payload = serde_json::json!({
        "query": query,
        "variables": variables,
    });
    let resp: GqlResponse<R> = octocrab
        .graphql(&payload)
        .await
        .context("GraphQL request failed")?;
    Ok(resp)
}

#[derive(Deserialize)]
struct GqlResponse<T> {
    data: Option<T>,
    errors: Option<serde_json::Value>,
}

impl<T> GqlResponse<T> {
    fn into_data(self) -> Result<T> {
        if let Some(errors) = self.errors {
            bail!("GraphQL errors: {errors}");
        }
        self.data.context("GraphQL response missing `data`")
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PrQueryData {
    repository: GqlRepo,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlRepo {
    pull_request: Option<GqlPullRequest>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlPullRequest {
    id: String,
    title: String,
    base_ref_name: String,
    base_ref_oid: String,
    head_ref_name: String,
    head_ref_oid: String,
    files: GqlFilesConn,
    review_threads: GqlThreadsConn,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlFilesConn {
    nodes: Vec<GqlFile>,
    page_info: PageInfo,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlFile {
    path: String,
    change_type: String,
    // additions/deletions/viewerViewedState aren't used yet but kept in the query
    // for future use (file-list line counts already come from the diff itself,
    // and viewerViewedState will seed Session.files when we tackle that backlog item).
    #[allow(dead_code)]
    additions: u64,
    #[allow(dead_code)]
    deletions: u64,
    #[allow(dead_code)]
    viewer_viewed_state: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageInfo {
    has_next_page: bool,
    end_cursor: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlThreadsConn {
    nodes: Vec<GqlThread>,
    page_info: PageInfo,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlThread {
    id: String,
    path: String,
    line: Option<u32>,
    diff_side: String,
    is_resolved: bool,
    is_outdated: bool,
    comments: GqlCommentsConn,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlCommentsConn {
    nodes: Vec<GqlComment>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlComment {
    author: Option<GqlActor>,
    body: String,
    created_at: chrono::DateTime<chrono::Utc>,
    state: String,
}

#[derive(Deserialize)]
struct GqlActor {
    login: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FilesPageData {
    repository: FilesPageRepo,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FilesPageRepo {
    pull_request: Option<FilesPagePr>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FilesPagePr {
    files: GqlFilesConn,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThreadsPageData {
    repository: ThreadsPageRepo,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThreadsPageRepo {
    pull_request: Option<ThreadsPagePr>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThreadsPagePr {
    review_threads: GqlThreadsConn,
}

const PR_QUERY: &str = r#"
query($owner: String!, $name: String!, $number: Int!) {
  repository(owner: $owner, name: $name) {
    pullRequest(number: $number) {
      id
      title
      baseRefName
      baseRefOid
      headRefName
      headRefOid
      files(first: 100) {
        nodes { path additions deletions changeType viewerViewedState }
        pageInfo { hasNextPage endCursor }
      }
      reviewThreads(first: 100) {
        nodes {
          id
          path
          line
          diffSide
          isResolved
          isOutdated
          comments(first: 100) {
            nodes { author { login } body createdAt state }
          }
        }
        pageInfo { hasNextPage endCursor }
      }
    }
  }
}
"#;

const FILES_PAGE_QUERY: &str = r#"
query($owner: String!, $name: String!, $number: Int!, $cursor: String!) {
  repository(owner: $owner, name: $name) {
    pullRequest(number: $number) {
      files(first: 100, after: $cursor) {
        nodes { path additions deletions changeType viewerViewedState }
        pageInfo { hasNextPage endCursor }
      }
    }
  }
}
"#;

const THREADS_PAGE_QUERY: &str = r#"
query($owner: String!, $name: String!, $number: Int!, $cursor: String!) {
  repository(owner: $owner, name: $name) {
    pullRequest(number: $number) {
      reviewThreads(first: 100, after: $cursor) {
        nodes {
          id
          path
          line
          diffSide
          isResolved
          isOutdated
          comments(first: 100) {
            nodes { author { login } body createdAt state }
          }
        }
        pageInfo { hasNextPage endCursor }
      }
    }
  }
}
"#;
