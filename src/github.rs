use anyhow::{Context, Result};
use octocrab::Octocrab;

pub struct PrFile {
    pub path: String,
    pub previous_path: Option<String>,
    pub status: String,
}

pub struct PrMetadata {
    pub title: String,
    pub base_branch: String,
    pub base_sha: String,
    pub head_branch: String,
    pub head_sha: String,
    pub files: Vec<PrFile>,
}

impl PrMetadata {
    pub fn file_count(&self) -> usize {
        self.files.len()
    }
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
    let base_branch = pr.base.ref_field;
    let base_sha = pr.base.sha;
    let head_branch = pr.head.ref_field;
    let head_sha = pr.head.sha;

    // First page only (up to 30 files). M5 will replace with full pagination.
    let files_page = octocrab
        .pulls(owner, repo)
        .list_files(pr_number)
        .await
        .with_context(|| {
            format!(
                "failed to list files for PR #{pr_number} from {owner}/{repo} \
                 (GET /repos/{owner}/{repo}/pulls/{pr_number}/files)"
            )
        })?;

    let files = files_page
        .items
        .into_iter()
        .map(|entry| PrFile {
            path: entry.filename,
            previous_path: entry.previous_filename,
            status: file_status_str(&entry.status),
        })
        .collect();

    Ok(PrMetadata {
        title,
        base_branch,
        base_sha,
        head_branch,
        head_sha,
        files,
    })
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
