use anyhow::{Context, Result};
use octocrab::Octocrab;

pub struct PrMetadata {
    pub title: String,
    pub base_branch: String,
    pub head_sha: String,
    pub file_count: usize,
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
    let head_sha = pr.head.sha;

    // First page only (up to 30 files). GitHub returns at most 300 files per PR.
    // M5 will replace this with full pagination when building the file list for the TUI.
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

    Ok(PrMetadata {
        title,
        base_branch,
        head_sha,
        file_count: files_page.items.len(),
    })
}
