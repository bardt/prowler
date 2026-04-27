use anyhow::{Context, Result, bail};
use octocrab::Octocrab;
use serde::Deserialize;

pub struct PrFile {
    pub path: String,
    pub previous_path: Option<String>,
    pub status: String,
    /// Lines added in this file according to GitHub's GraphQL response. Used
    /// for the file-tree counters before the local diff has been computed —
    /// `compute_pr_diffs` is lazy, so unvisited files would otherwise show 0.
    pub additions: u64,
    pub deletions: u64,
    /// GitHub's per-viewer viewed state for this file: `VIEWED`, `DISMISSED`, or
    /// `UNVIEWED`. Used to seed `Session.files` on first open from a new machine.
    pub viewer_viewed_state: String,
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
    /// ID of the viewer's currently pending review on this PR, if any. Set by
    /// `fetch_pr` and used by `submit_review` to publish draft comments.
    pub pending_review_id: Option<String>,
    /// PullRequestState enum from GraphQL: `OPEN`, `CLOSED`, `MERGED`.
    pub state: String,
    /// True when the PR is a draft. Cross-cuts with `state == OPEN` —
    /// drafts render as DRAFT instead of OPEN.
    pub is_draft: bool,
    /// PR description (markdown source). Rendered as plain text in the
    /// description panel.
    pub body: String,
    /// Canonical web URL (e.g. https://github.com/owner/repo/pull/123).
    /// Used by `O` to open the PR page in the user's browser.
    pub url: String,
    /// Issue-level (non-inline) comments on the PR, in posting order.
    pub conversation: Vec<ConversationComment>,
    /// Login of the currently authenticated user. Used to detect the viewer's
    /// own comments for edit/delete actions.
    #[allow(dead_code)]
    pub viewer_login: String,
}

#[derive(Debug, Clone)]
pub struct ConversationComment {
    pub author: String,
    pub body: String,
    /// Pre-formatted "YYYY-MM-DD HH:MM".
    pub created_at: String,
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
    /// Comment node ID — required for `updatePullRequestReviewComment` /
    /// `deletePullRequestReviewComment` mutations.
    pub id: String,
    pub author: String,
    pub body: String,
    /// Pre-formatted "YYYY-MM-DD HH:MM".
    pub created_at: String,
    /// True when the comment is part of an unsubmitted review (GraphQL `state == "PENDING"`).
    pub is_pending: bool,
    /// True when the viewer wrote this comment (so they can edit / delete).
    pub viewer_did_author: bool,
}

#[allow(dead_code)] // some fields reserved for future UI.
#[derive(Debug, Clone)]
pub struct CommentThread {
    /// Thread node ID — used for `addPullRequestReviewThreadReply` (M9) and
    /// `resolveReviewThread` / `unresolveReviewThread` (M13).
    pub id: String,
    pub path: String,
    pub side: CommentSide,
    pub line: u32,
    pub is_resolved: bool,
    pub is_outdated: bool,
    pub viewer_can_resolve: bool,
    pub viewer_can_unresolve: bool,
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

    let data = response.into_data()?;
    let viewer_login = data.viewer.login.clone();
    let pr = data
        .repository
        .pull_request
        .context("GraphQL returned null pullRequest")?;

    let pending_review_id = pr.reviews.nodes.iter().find_map(|r| {
        if r.author.as_ref().map(|a| a.login.as_str()) == Some(viewer_login.as_str()) {
            Some(r.id.clone())
        } else {
            None
        }
    });

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
            additions: f.additions,
            deletions: f.deletions,
            viewer_viewed_state: f.viewer_viewed_state,
        })
        .collect();

    let conversation = pr
        .comments
        .nodes
        .into_iter()
        .map(|c| ConversationComment {
            author: c
                .author
                .map(|a| a.login)
                .unwrap_or_else(|| "unknown".to_string()),
            body: c.body,
            created_at: c.created_at.format("%Y-%m-%d %H:%M").to_string(),
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
        pending_review_id,
        state: pr.state,
        is_draft: pr.is_draft,
        body: pr.body,
        url: pr.url,
        conversation,
        viewer_login: viewer_login.clone(),
    };

    let threads = gql_threads
        .into_iter()
        .filter_map(|t| {
            // For outdated threads `line` is null. Anchor at the original line
            // on the BASE side (where the original code still lives) so the
            // user can see them in context.
            let (line, side) = match (t.line, t.original_line, t.diff_side.as_str()) {
                (Some(line), _, "LEFT") => (line as u32, CommentSide::Base),
                (Some(line), _, "RIGHT") => (line as u32, CommentSide::Head),
                (None, Some(orig), _) => (orig as u32, CommentSide::Base),
                _ => return None,
            };
            let comments = t
                .comments
                .nodes
                .into_iter()
                .map(|c| {
                    let author = c
                        .author
                        .as_ref()
                        .map(|a| a.login.clone())
                        .unwrap_or_else(|| "unknown".to_string());
                    let viewer_did_author = author == viewer_login;
                    ReviewComment {
                        id: c.id,
                        author,
                        body: c.body,
                        created_at: c.created_at.format("%Y-%m-%d %H:%M").to_string(),
                        is_pending: c.state == "PENDING",
                        viewer_did_author,
                    }
                })
                .collect();
            Some(CommentThread {
                id: t.id,
                path: t.path,
                side,
                line,
                is_resolved: t.is_resolved,
                is_outdated: t.is_outdated,
                viewer_can_resolve: t.viewer_can_resolve,
                viewer_can_unresolve: t.viewer_can_unresolve,
                comments,
            })
        })
        .collect();

    Ok((metadata, threads))
}

/// Post a new review thread anchored to a single line, or a multi-line span
/// when `start_line` is provided. Uses the `addPullRequestReviewThread`
/// GraphQL mutation with `subjectType: LINE`.
pub async fn post_thread(
    token: &str,
    pr_node_id: &str,
    path: &str,
    side: CommentSide,
    line: u32,
    start_line: Option<u32>,
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

    // GitHub rejects start_line == line on multi-line input, so only include
    // the start fields when the span actually spans more than one line.
    let multi = matches!(start_line, Some(s) if s != line);

    let (query, vars) = if multi {
        (
            r#"
mutation($pid: ID!, $path: String!, $line: Int!, $startLine: Int!, $side: DiffSide!, $startSide: DiffSide!, $body: String!) {
  addPullRequestReviewThread(input: {
    pullRequestId: $pid,
    path: $path,
    line: $line,
    startLine: $startLine,
    side: $side,
    startSide: $startSide,
    body: $body,
    subjectType: LINE
  }) {
    thread { id }
  }
}
"#,
            serde_json::json!({
                "pid": pr_node_id,
                "path": path,
                "line": line,
                "startLine": start_line.unwrap(),
                "side": side_str,
                "startSide": side_str,
                "body": body,
            }),
        )
    } else {
        (
            r#"
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
"#,
            serde_json::json!({
                "pid": pr_node_id,
                "path": path,
                "line": line,
                "side": side_str,
                "body": body,
            }),
        )
    };

    let payload = serde_json::json!({ "query": query, "variables": vars });
    let response: serde_json::Value = octocrab.graphql(&payload).await.with_context(|| {
        format!("addPullRequestReviewThread request failed for `{path}:{line}`")
    })?;

    if let Some(errors) = response.get("errors") {
        bail!("addPullRequestReviewThread for `{path}:{line}`: {errors}");
    }
    Ok(())
}

/// Submit (publish) a review with a verdict and optional summary body.
///
/// If `pending_review_id` is `Some`, this submits the existing pending review,
/// publishing all of its draft comments. If `None`, a fresh review is created and
/// submitted in one shot via `addPullRequestReview` (verdict-only, no comments).
pub async fn submit_review(
    token: &str,
    pr_node_id: &str,
    pending_review_id: Option<&str>,
    event: &str,
    body: &str,
) -> Result<()> {
    let octocrab = Octocrab::builder()
        .personal_token(token.to_owned())
        .build()
        .context("failed to build GitHub client")?;

    let body_var = if body.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::Value::String(body.to_owned())
    };

    let payload = if let Some(review_id) = pending_review_id {
        serde_json::json!({
            "query": r#"
mutation($rid: ID!, $event: PullRequestReviewEvent!, $body: String) {
  submitPullRequestReview(input: { pullRequestReviewId: $rid, event: $event, body: $body }) {
    pullRequestReview { id state }
  }
}"#,
            "variables": { "rid": review_id, "event": event, "body": body_var },
        })
    } else {
        serde_json::json!({
            "query": r#"
mutation($pid: ID!, $event: PullRequestReviewEvent!, $body: String) {
  addPullRequestReview(input: { pullRequestId: $pid, event: $event, body: $body }) {
    pullRequestReview { id state }
  }
}"#,
            "variables": { "pid": pr_node_id, "event": event, "body": body_var },
        })
    };

    let response: serde_json::Value = octocrab
        .graphql(&payload)
        .await
        .context("submit_review GraphQL request failed")?;

    if let Some(errors) = response.get("errors") {
        bail!("submit_review: {errors}");
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

    let response: serde_json::Value = octocrab.graphql(&payload).await.with_context(|| {
        format!("addPullRequestReviewThreadReply request failed for thread {thread_id}")
    })?;

    if let Some(errors) = response.get("errors") {
        bail!("addPullRequestReviewThreadReply for thread {thread_id}: {errors}");
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct DashboardPr {
    pub number: u64,
    pub title: String,
    pub author: String,
    pub is_draft: bool,
    /// Pre-formatted "YYYY-MM-DD HH:MM".
    pub updated_at: String,
    pub additions: u64,
    pub deletions: u64,
    /// `APPROVED`, `CHANGES_REQUESTED`, `REVIEW_REQUIRED`, or empty.
    pub review_decision: String,
    /// `nameWithOwner` of the PR's repo (the search may cross repos).
    #[allow(dead_code)]
    pub repo_name_with_owner: String,
    #[allow(dead_code)]
    pub url: String,
    /// Number of inline review threads + issue comments.
    pub comment_count: u64,
}

#[derive(Debug, Clone, Default)]
pub struct DashboardData {
    pub review_requested: Vec<DashboardPr>,
    pub authored: Vec<DashboardPr>,
    pub assigned: Vec<DashboardPr>,
}

/// Fetch the three lists that drive the dashboard: PRs awaiting the viewer's
/// review, PRs the viewer authored, and PRs assigned to the viewer. Scoped to
/// the current repo so the dashboard reflects the directory you ran prowler in.
pub async fn fetch_dashboard(token: &str, owner: &str, repo: &str) -> Result<DashboardData> {
    let octocrab = Octocrab::builder()
        .personal_token(token.to_owned())
        .build()
        .context("failed to build GitHub client")?;

    let scope = match crate::config::get().dashboard.scope.as_str() {
        "all" => String::new(),
        _ => format!("repo:{owner}/{repo}"),
    };
    let req = format!("is:open is:pr review-requested:@me {scope}")
        .trim()
        .to_owned();
    let auth = format!("is:open is:pr author:@me {scope}")
        .trim()
        .to_owned();
    let asgn = format!("is:open is:pr assignee:@me {scope}")
        .trim()
        .to_owned();

    let payload = serde_json::json!({
        "query": DASHBOARD_QUERY,
        "variables": { "req": req, "auth": auth, "asgn": asgn },
    });

    let response: GqlResponse<DashboardData_> = octocrab
        .graphql(&payload)
        .await
        .context("dashboard GraphQL request failed")?;
    let data = response.into_data()?;

    let mut out = DashboardData {
        review_requested: data
            .review_requested
            .nodes
            .into_iter()
            .filter_map(into_dashboard_pr)
            .collect(),
        authored: data
            .authored
            .nodes
            .into_iter()
            .filter_map(into_dashboard_pr)
            .collect(),
        assigned: data
            .assigned
            .nodes
            .into_iter()
            .filter_map(into_dashboard_pr)
            .collect(),
    };
    let by_recent = |a: &DashboardPr, b: &DashboardPr| b.updated_at.cmp(&a.updated_at);
    out.review_requested.sort_by(by_recent);
    out.authored.sort_by(by_recent);
    out.assigned.sort_by(by_recent);
    Ok(out)
}

fn into_dashboard_pr(node: GqlSearchNode) -> Option<DashboardPr> {
    let pr = node.into_pr()?;
    Some(DashboardPr {
        number: pr.number,
        title: pr.title,
        author: pr
            .author
            .map(|a| a.login)
            .unwrap_or_else(|| "unknown".to_string()),
        is_draft: pr.is_draft,
        updated_at: pr.updated_at.format("%Y-%m-%d %H:%M").to_string(),
        additions: pr.additions,
        deletions: pr.deletions,
        review_decision: pr.review_decision.unwrap_or_default(),
        repo_name_with_owner: pr.repository.name_with_owner,
        url: pr.url,
        comment_count: pr.comments.total_count + pr.review_threads.total_count,
    })
}

/// Resolve or unresolve a review thread via GraphQL.
pub async fn set_thread_resolved(token: &str, thread_id: &str, resolved: bool) -> Result<()> {
    let octocrab = Octocrab::builder()
        .personal_token(token.to_owned())
        .build()
        .context("failed to build GitHub client")?;
    let mutation = if resolved {
        "resolveReviewThread"
    } else {
        "unresolveReviewThread"
    };
    let query = format!(
        "mutation($tid: ID!) {{ {mutation}(input: {{ threadId: $tid }}) {{ thread {{ id isResolved }} }} }}"
    );
    let payload = serde_json::json!({
        "query": query,
        "variables": { "tid": thread_id },
    });
    let response: serde_json::Value = octocrab
        .graphql(&payload)
        .await
        .with_context(|| format!("GraphQL {mutation} request failed for thread {thread_id}"))?;
    if let Some(errors) = response.get("errors") {
        bail!("GraphQL {mutation} for thread {thread_id}: {errors}");
    }
    Ok(())
}

/// Edit the body of an existing review comment authored by the viewer.
pub async fn update_review_comment(token: &str, comment_id: &str, body: &str) -> Result<()> {
    let octocrab = Octocrab::builder()
        .personal_token(token.to_owned())
        .build()
        .context("failed to build GitHub client")?;
    let payload = serde_json::json!({
        "query": r#"
mutation($cid: ID!, $body: String!) {
  updatePullRequestReviewComment(input: { pullRequestReviewCommentId: $cid, body: $body }) {
    pullRequestReviewComment { id }
  }
}"#,
        "variables": { "cid": comment_id, "body": body },
    });
    let response: serde_json::Value = octocrab
        .graphql(&payload)
        .await
        .with_context(|| format!("updatePullRequestReviewComment failed for {comment_id}"))?;
    if let Some(errors) = response.get("errors") {
        bail!("updatePullRequestReviewComment for {comment_id}: {errors}");
    }
    Ok(())
}

/// Delete a review comment authored by the viewer.
pub async fn delete_review_comment(token: &str, comment_id: &str) -> Result<()> {
    let octocrab = Octocrab::builder()
        .personal_token(token.to_owned())
        .build()
        .context("failed to build GitHub client")?;
    let payload = serde_json::json!({
        "query": r#"
mutation($cid: ID!) {
  deletePullRequestReviewComment(input: { id: $cid }) {
    pullRequestReview { id }
  }
}"#,
        "variables": { "cid": comment_id },
    });
    let response: serde_json::Value = octocrab
        .graphql(&payload)
        .await
        .with_context(|| format!("deletePullRequestReviewComment failed for {comment_id}"))?;
    if let Some(errors) = response.get("errors") {
        bail!("deletePullRequestReviewComment for {comment_id}: {errors}");
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
    viewer: GqlViewer,
    repository: GqlRepo,
}

#[derive(Deserialize)]
struct GqlViewer {
    login: String,
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
    state: String,
    is_draft: bool,
    body: String,
    url: String,
    base_ref_name: String,
    base_ref_oid: String,
    head_ref_name: String,
    head_ref_oid: String,
    reviews: GqlReviewsConn,
    comments: GqlIssueCommentsConn,
    files: GqlFilesConn,
    review_threads: GqlThreadsConn,
}

#[derive(Deserialize)]
struct GqlIssueCommentsConn {
    nodes: Vec<GqlIssueComment>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlIssueComment {
    author: Option<GqlActor>,
    body: String,
    created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Deserialize)]
struct GqlReviewsConn {
    nodes: Vec<GqlReview>,
}

#[derive(Deserialize)]
struct GqlReview {
    id: String,
    author: Option<GqlActor>,
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
    additions: u64,
    deletions: u64,
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
    /// Set when GitHub auto-cleared `line` (head moved); we fall back to this
    /// to anchor outdated threads at their original position on the BASE side.
    original_line: Option<u32>,
    diff_side: String,
    is_resolved: bool,
    is_outdated: bool,
    viewer_can_resolve: bool,
    viewer_can_unresolve: bool,
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
    id: String,
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DashboardData_ {
    #[serde(rename = "reviewRequested")]
    review_requested: GqlSearchConn,
    #[serde(rename = "authored")]
    authored: GqlSearchConn,
    #[serde(rename = "assigned")]
    assigned: GqlSearchConn,
}

#[derive(Deserialize)]
struct GqlSearchConn {
    nodes: Vec<GqlSearchNode>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum GqlSearchNode {
    Pr(Box<GqlSearchPr>),
    Other(#[allow(dead_code)] serde_json::Value),
}

impl GqlSearchNode {
    fn into_pr(self) -> Option<GqlSearchPr> {
        match self {
            GqlSearchNode::Pr(pr) => Some(*pr),
            GqlSearchNode::Other(_) => None,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlSearchPr {
    number: u64,
    title: String,
    is_draft: bool,
    author: Option<GqlActor>,
    updated_at: chrono::DateTime<chrono::Utc>,
    additions: u64,
    deletions: u64,
    review_decision: Option<String>,
    repository: GqlRepoName,
    url: String,
    comments: GqlTotalCount,
    review_threads: GqlTotalCount,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlRepoName {
    name_with_owner: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlTotalCount {
    total_count: u64,
}

const DASHBOARD_QUERY: &str = r#"
query($req: String!, $auth: String!, $asgn: String!) {
  reviewRequested: search(query: $req, type: ISSUE, first: 30) {
    nodes {
      ... on PullRequest {
        number title isDraft updatedAt additions deletions reviewDecision url
        author { login }
        repository { nameWithOwner }
        comments { totalCount }
        reviewThreads { totalCount }
      }
    }
  }
  authored: search(query: $auth, type: ISSUE, first: 30) {
    nodes {
      ... on PullRequest {
        number title isDraft updatedAt additions deletions reviewDecision url
        author { login }
        repository { nameWithOwner }
        comments { totalCount }
        reviewThreads { totalCount }
      }
    }
  }
  assigned: search(query: $asgn, type: ISSUE, first: 30) {
    nodes {
      ... on PullRequest {
        number title isDraft updatedAt additions deletions reviewDecision url
        author { login }
        repository { nameWithOwner }
        comments { totalCount }
        reviewThreads { totalCount }
      }
    }
  }
}
"#;

const PR_QUERY: &str = r#"
query($owner: String!, $name: String!, $number: Int!) {
  viewer { login }
  repository(owner: $owner, name: $name) {
    pullRequest(number: $number) {
      id
      title
      state
      isDraft
      body
      url
      baseRefName
      baseRefOid
      headRefName
      headRefOid
      comments(first: 100) {
        nodes { author { login } body createdAt }
      }
      reviews(states: [PENDING], first: 10) {
        nodes { id author { login } }
      }
      files(first: 100) {
        nodes { path additions deletions changeType viewerViewedState }
        pageInfo { hasNextPage endCursor }
      }
      reviewThreads(first: 100) {
        nodes {
          id
          path
          line
          originalLine
          diffSide
          isResolved
          isOutdated
          viewerCanResolve
          viewerCanUnresolve
          comments(first: 100) {
            nodes { id author { login } body createdAt state }
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
          originalLine
          diffSide
          isResolved
          isOutdated
          viewerCanResolve
          viewerCanUnresolve
          comments(first: 100) {
            nodes { id author { login } body createdAt state }
          }
        }
        pageInfo { hasNextPage endCursor }
      }
    }
  }
}
"#;
