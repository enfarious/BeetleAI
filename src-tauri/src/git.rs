use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Run a git command with args in a specific directory
fn run_git_cmd<P: AsRef<Path>>(cwd: P, args: &[&str]) -> Result<String, String> {
    let mut cmd = Command::new("git");
    cmd.current_dir(cwd.as_ref()).args(args);
    crate::configure_no_window(&mut cmd);
    let output = cmd
        .output()
        .map_err(|e| format!("Failed to execute git: {}", e))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

/// Verify if a path is inside a Git repository
pub fn is_git_repo<P: AsRef<Path>>(repo_path: P) -> bool {
    run_git_cmd(&repo_path, &["rev-parse", "--is-inside-work-tree"]).is_ok()
}

/// Checks if the git working directory is clean
pub fn is_working_tree_clean<P: AsRef<Path>>(repo_path: P) -> Result<bool, String> {
    let output = run_git_cmd(repo_path, &["status", "--porcelain"])?;
    Ok(output.is_empty())
}

/// Creates a new worktree branch and adds the git worktree
pub fn create_worktree<P: AsRef<Path>>(
    repo_path: P,
    run_id: &str,
    base_branch: &str,
) -> Result<PathBuf, String> {
    let repo_path = repo_path.as_ref();

    // Check if dirty
    if !is_working_tree_clean(repo_path)? {
        return Err("The parent repository has uncommitted changes. Please stash or commit them before starting a run.".to_string());
    }

    let worktree_dir = repo_path.join(".harness").join("worktrees").join(run_id);
    let branch_name = format!("harness/run-{}", run_id);

    // Create .harness/worktrees directory if not exists
    let worktrees_parent = repo_path.join(".harness").join("worktrees");
    if !worktrees_parent.exists() {
        fs::create_dir_all(&worktrees_parent).map_err(|e| e.to_string())?;
    }

    // Command: git worktree add .harness/worktrees/<run-id> -b harness/run-<run-id> <base_branch>
    let worktree_str = worktree_dir.to_string_lossy().to_string();
    run_git_cmd(
        repo_path,
        &[
            "worktree",
            "add",
            &worktree_str,
            "-b",
            &branch_name,
            base_branch,
        ],
    )?;

    Ok(worktree_dir)
}

/// Removes a worktree and deletes its associated isolation branch
pub fn remove_worktree<P: AsRef<Path>>(repo_path: P, run_id: &str) -> Result<(), String> {
    let repo_path = repo_path.as_ref();
    let worktree_dir = repo_path.join(".harness").join("worktrees").join(run_id);
    let branch_name = format!("harness/run-{}", run_id);

    let worktree_str = worktree_dir.to_string_lossy().to_string();

    // 1. Remove the worktree path from git
    if worktree_dir.exists() {
        run_git_cmd(repo_path, &["worktree", "remove", "--force", &worktree_str])?;
    }

    // 2. Delete the temporary branch
    // Check if the branch exists first
    let branches = run_git_cmd(repo_path, &["branch", "--list", &branch_name])?;
    if !branches.is_empty() {
        run_git_cmd(repo_path, &["branch", "-D", &branch_name])?;
    }

    Ok(())
}

/// Merges the worktree branch into the base branch and cleans up the worktree
pub fn merge_worktree<P: AsRef<Path>>(
    repo_path: P,
    run_id: &str,
    base_branch: &str,
) -> Result<(), String> {
    let repo_path = repo_path.as_ref();
    let branch_name = format!("harness/run-{}", run_id);
    let worktree_dir = repo_path.join(".harness").join("worktrees").join(run_id);

    // 0. Commit the agent's work. Edits live as UNCOMMITTED working-tree state
    //    in the worktree; without this commit the branch tip never moves, the
    //    merge below is a silent no-op, and the --force cleanup deletes the
    //    only copy of the work.
    if worktree_dir.exists() {
        run_git_cmd(&worktree_dir, &["add", "-A"])?;
        // "nothing to commit" is a legitimate outcome; tolerate it.
        let _ = run_git_cmd(
            &worktree_dir,
            &["commit", "-m", &format!("Harness run {}", run_id)],
        );
    }

    // 0b. Refuse a no-op merge loudly instead of pretending success.
    let ahead = run_git_cmd(
        repo_path,
        &[
            "rev-list",
            "--count",
            &format!("{}..{}", base_branch, branch_name),
        ],
    )?;
    if ahead.trim() == "0" {
        return Err(format!(
            "Nothing to merge: run {} produced no committed changes relative to {}.",
            run_id, base_branch
        ));
    }

    // 1. Check out base branch (must be clean as verified earlier)
    run_git_cmd(repo_path, &["checkout", base_branch])?;

    // 2. Merge the isolation branch (non-fast-forward to keep history clean and visible)
    let merge_msg = format!("Merge harness run {}", run_id);
    let merge_res = run_git_cmd(
        repo_path,
        &["merge", &branch_name, "--no-ff", "-m", &merge_msg],
    );

    if let Err(e) = merge_res {
        // If merge fails (conflicts), abort the merge and return the conflict details
        let _ = run_git_cmd(repo_path, &["merge", "--abort"]);
        return Err(format!(
            "Merge failed due to conflicts: {}. Merge aborted.",
            e
        ));
    }

    // 3. Remove the worktree
    remove_worktree(repo_path, run_id)?;

    Ok(())
}

/// Computes the unified diff between base branch and isolation branch
pub fn get_diff<P: AsRef<Path>>(
    repo_path: P,
    run_id: &str,
    base_branch: &str,
) -> Result<String, String> {
    let repo_path = repo_path.as_ref();
    let branch_name = format!("harness/run-{}", run_id);

    // Call: git diff base_branch..harness/run-run_id
    let range = format!("{}..{}", base_branch, branch_name);
    run_git_cmd(repo_path, &["diff", &range])
}

/// Diff for a LIVE run: the worktree's current (typically uncommitted) state
/// against the base branch. Committed-range diffs are blind to uncommitted
/// work, which is where an in-progress run's changes actually live.
pub fn get_worktree_diff<P: AsRef<Path>>(
    repo_path: P,
    run_id: &str,
    base_branch: &str,
) -> Result<String, String> {
    let repo_path = repo_path.as_ref();
    let worktree_dir = repo_path.join(".harness").join("worktrees").join(run_id);
    if worktree_dir.exists() {
        return run_git_cmd(&worktree_dir, &["diff", base_branch]);
    }
    get_diff(repo_path, run_id, base_branch)
}

/// Clean up any orphaned worktrees left behind on app crashes/forced quits
#[allow(dead_code)]
pub fn prune_worktrees<P: AsRef<Path>>(repo_path: P) -> Result<(), String> {
    run_git_cmd(repo_path, &["worktree", "prune"])?;
    Ok(())
}

/// Check whether a local branch exists.
pub fn branch_exists<P: AsRef<Path>>(repo_path: P, branch_name: &str) -> bool {
    run_git_cmd(
        repo_path,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{}", branch_name),
        ],
    )
    .is_ok()
}

/// Get the current active branch name
pub fn get_current_branch<P: AsRef<Path>>(repo_path: P) -> Result<String, String> {
    run_git_cmd(repo_path, &["rev-parse", "--abbrev-ref", "HEAD"])
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn init_temp_git_repo() -> (TempDir, PathBuf) {
        let temp_dir = TempDir::new().unwrap();
        let repo_path = temp_dir.path().to_path_buf();

        // git init
        run_git_cmd(&repo_path, &["init", "-b", "main"]).unwrap();

        // Configure dummy user for commits
        run_git_cmd(&repo_path, &["config", "user.name", "Test User"]).unwrap();
        run_git_cmd(&repo_path, &["config", "user.email", "test@example.com"]).unwrap();

        // Create an initial commit
        let test_file = repo_path.join("README.md");
        fs::write(&test_file, "# Test Repo\nInitial content").unwrap();
        run_git_cmd(&repo_path, &["add", "README.md"]).unwrap();
        run_git_cmd(&repo_path, &["commit", "-m", "Initial commit"]).unwrap();

        (temp_dir, repo_path)
    }

    #[test]
    fn test_git_workflow() {
        let (_temp_dir, repo_path) = init_temp_git_repo();
        let run_id = "test_run_123";
        let base = "main";

        assert!(is_git_repo(&repo_path));
        assert!(is_working_tree_clean(&repo_path).unwrap());

        // 1. Create worktree
        let wt_path = create_worktree(&repo_path, run_id, base).unwrap();
        assert!(wt_path.exists());
        assert!(repo_path
            .join(".harness")
            .join("worktrees")
            .join(run_id)
            .exists());

        // 2. Modify file in worktree
        let wt_file = wt_path.join("new_file.txt");
        fs::write(&wt_file, "Hello from worktree sandbox!").unwrap();

        // Commit file inside worktree (agent simulator)
        run_git_cmd(&wt_path, &["add", "new_file.txt"]).unwrap();
        run_git_cmd(&wt_path, &["commit", "-m", "Agent commit in sandbox"]).unwrap();

        // 3. Get diff
        let diff = get_diff(&repo_path, run_id, base).unwrap();
        assert!(diff.contains("new_file.txt"));
        assert!(diff.contains("Hello from worktree sandbox!"));

        // 4. Merge worktree
        merge_worktree(&repo_path, run_id, base).unwrap();

        // 5. Verify branch and worktree directory are gone
        assert!(!wt_path.exists());

        // Verify changes are in base repo
        let base_file = repo_path.join("new_file.txt");
        assert!(base_file.exists());
        assert_eq!(
            fs::read_to_string(base_file).unwrap(),
            "Hello from worktree sandbox!"
        );
    }
}
