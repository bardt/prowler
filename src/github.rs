use anyhow::{bail, Context, Result};
use octocrab::Octocrab;
use std::collections::HashMap;

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
    Base, // GitHub "LEFT"
    Head, // GitHub "RIGHT"
}

#[derive(Debug, Clone)]
pub struct ReviewComment {
    pub id: u64,
    pub parent_id: Option<u64>,
    pub author: String,
    pub body: String,
    /// Pre-formatted "YYYY-MM-DD HH:MM".
    pub created_at: String,
    pub path: String,
    pub line: u32,
    pub side: CommentSide,
}

#[derive(Debug, Clone)]
pub struct CommentThread {
    pub path: String,
    pub side: CommentSide,
    pub line: u32,
    /// Root comment first, replies in chronological order.
    pub comments: Vec<ReviewComment>,
}

pub async fn fetch_pr(token: &str, owner: &str, repo: &str, pr_number: u64) -> Result<PrMetadata> {
    let octocrab = Octocrab::builder()
        .personal_token(token.to_owned())
        .build()
        .context("failed to build GitHub client")?;

    let pr = octocrab
        .pulls(owner, repo)
        .get(pr_number)
        .await
        .with_context(|| {
            format!(
                "failed to fetch PR #{pr_number} from {owner}/{repo} \
                 (GET /repos/{owner}/{repo}/pulls/{pr_number})"
            )
        })?;

    let title = pr
        .title
        .with_context(|| format!("PR #{pr_number} has no title"))?;
    let node_id = pr
        .node_id
        .with_context(|| format!("PR #{pr_number} has no node_id"))?;
    let base_branch = pr.base.ref_field;
    let base_sha = pr.base.sha;
    let head_branch = pr.head.ref_field;
    let head_sha = pr.head.sha;

    let first_page = octocrab
        .pulls(owner, repo)
        .list_files(pr_number)
        .await
        .with_context(|| {
            format!(
                "failed to list files for PR #{pr_number} from {owner}/{repo} \
                 (GET /repos/{owner}/{repo}/pulls/{pr_number}/files)"
            )
        })?;
    let all_files = octocrab
        .all_pages(first_page)
        .await
        .with_context(|| format!("failed to paginate files for PR #{pr_number}"))?;

    let files = all_files
        .into_iter()
        .map(|entry| PrFile {
            path: entry.filename,
            previous_path: entry.previous_filename,
            status: file_status_str(&entry.status),
        })
        .collect();

    Ok(PrMetadata {
        pr_number,
        node_id,
        title,
        base_branch,
        base_sha,
        head_branch,
        head_sha,
        files,
    })
}

/// Fetch all PR review comments (line-anchored). Comments without a current `line`
/// (outdated) are dropped — see backlog for handling them.
pub async fn fetch_comments(
    token: &str,
    owner: &str,
    repo: &str,
    pr_number: u64,
) -> Result<Vec<ReviewComment>> {
    let octocrab = Octocrab::builder()
        .personal_token(token.to_owned())
        .build()
        .context("failed to build GitHub client")?;

    let first_page = octocrab
        .pulls(owner, repo)
        .list_comments(Some(pr_number))
        .per_page(100)
        .send()
        .await
        .with_context(|| format!("failed to list review comments for PR #{pr_number}"))?;
    let all = octocrab
        .all_pages(first_page)
        .await
        .with_context(|| format!("failed to paginate comments for PR #{pr_number}"))?;

    let comments = all
        .into_iter()
        .filter_map(|c| {
            let line = c.line? as u32;
            let side = match c.side.as_deref() {
                Some("LEFT") => CommentSide::Base,
                Some("RIGHT") => CommentSide::Head,
                _ => return None,
            };
            let author = c
                .user
                .map(|u| u.login)
                .unwrap_or_else(|| "unknown".to_string());
            let created_at = c.created_at.format("%Y-%m-%d %H:%M").to_string();
            Some(ReviewComment {
                id: c.id.into_inner(),
                parent_id: c.in_reply_to_id.map(|i| i.into_inner()),
                author,
                body: c.body,
                created_at,
                path: c.path,
                line,
                side,
            })
        })
        .collect();

    Ok(comments)
}

/// Group flat comments into threads. Walks `parent_id` to find each comment's root,
/// then buckets by root id. Threads are sorted by path then line for stable rendering.
pub fn group_threads(comments: Vec<ReviewComment>) -> Vec<CommentThread> {
    let parent_map: HashMap<u64, u64> = comments
        .iter()
        .filter_map(|c| c.parent_id.map(|p| (c.id, p)))
        .collect();

    let mut buckets: HashMap<u64, Vec<ReviewComment>> = HashMap::new();
    for c in comments {
        let mut root = c.id;
        while let Some(&p) = parent_map.get(&root) {
            root = p;
        }
        buckets.entry(root).or_default().push(c);
    }

    let mut threads: Vec<CommentThread> = buckets
        .into_values()
        .filter_map(|mut group| {
            group.sort_by(|a, b| a.created_at.cmp(&b.created_at));
            let root = group.first()?;
            Some(CommentThread {
                path: root.path.clone(),
                side: root.side,
                line: root.line,
                comments: group,
            })
        })
        .collect();

    threads.sort_by(|a, b| a.path.cmp(&b.path).then(a.line.cmp(&b.line)));
    threads
}

/// Mark or unmark a PR file as viewed via GitHub GraphQL.
/// REST has no endpoint for this — `markFileAsViewed` / `unmarkFileAsViewed`
/// are GraphQL-only mutations and require the PR's node ID.
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

fn file_status_str(status: &octocrab::models::repos::DiffEntryStatus) -> String {
    use octocrab::models::repos::DiffEntryStatus::*;
    match status {
        Added => "added",
        Removed => "removed",
        Modified => "modified",
        Renamed => "renamed",
        Copied => "copied",
        Changed => "changed",
        Unchanged => "unchanged",
        _ => "unknown",
    }
    .to_owned()
}
