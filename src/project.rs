use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub name: String,
    pub worktree: Option<String>,
}

pub struct Projects {
    projects: HashMap<PathBuf, Project>,
}

impl Projects {
    pub fn new() -> Self {
        Self { projects: HashMap::new() }
    }

    pub fn project_for(&mut self, cwd: &Path) -> Project {
        self.projects
            .entry(cwd.to_path_buf())
            .or_insert_with(|| resolve(cwd))
            .clone()
    }
}

struct Checkout {
    toplevel: PathBuf,
    git_dir: PathBuf,
    common_dir: PathBuf,
}

fn resolve(cwd: &Path) -> Project {
    match checkout(cwd) {
        Some(checkout) if checkout.git_dir == checkout.common_dir => {
            Project { name: folder_name(&checkout.toplevel), worktree: None }
        }
        Some(checkout) => Project {
            name: repo_name(&checkout.common_dir),
            worktree: Some(folder_name(&checkout.toplevel)),
        },
        None => Project { name: folder_name(cwd), worktree: None },
    }
}

fn checkout(cwd: &Path) -> Option<Checkout> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args([
            "rev-parse",
            "--path-format=absolute",
            "--show-toplevel",
            "--absolute-git-dir",
            "--git-common-dir",
        ])
        .output()
        .ok()
        .filter(|output| output.status.success())?;
    let stdout = String::from_utf8(output.stdout).ok()?;
    let mut lines = stdout.lines().map(PathBuf::from);
    Some(Checkout {
        toplevel: lines.next()?,
        git_dir: lines.next()?,
        common_dir: lines.next()?,
    })
}

fn repo_name(common_dir: &Path) -> String {
    if common_dir.file_name().is_some_and(|name| name == ".git") {
        common_dir.parent().map(folder_name).unwrap_or_default()
    } else {
        folder_name(common_dir).trim_end_matches(".git").to_string()
    }
}

fn folder_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=test", "-c", "user.email=test@example.com"])
            .args(args)
            .output()
            .unwrap()
            .status;
        assert!(status.success(), "git {args:?} failed");
    }

    fn repo(root: &TempDir, name: &str) -> PathBuf {
        let path = root.path().join(name);
        fs::create_dir(&path).unwrap();
        git(&path, &["init", "-q"]);
        git(&path, &["commit", "-q", "--allow-empty", "-m", "init"]);
        path
    }

    fn project(name: &str, worktree: Option<&str>) -> Project {
        Project { name: name.to_string(), worktree: worktree.map(String::from) }
    }

    #[test]
    fn main_checkout_uses_its_folder_name() {
        let root = TempDir::new().unwrap();
        let api = repo(&root, "api");
        assert_eq!(Projects::new().project_for(&api), project("api", None));
    }

    #[test]
    fn subfolder_maps_to_its_checkout() {
        let root = TempDir::new().unwrap();
        let src = repo(&root, "api").join("src");
        fs::create_dir(&src).unwrap();
        assert_eq!(Projects::new().project_for(&src), project("api", None));
    }

    #[test]
    fn worktree_belongs_to_its_repo() {
        let root = TempDir::new().unwrap();
        let api = repo(&root, "api");
        let worktree = root.path().join("fix-x");
        git(&api, &["worktree", "add", "-q", "-b", "fix-x", worktree.to_str().unwrap()]);
        assert_eq!(Projects::new().project_for(&worktree), project("api", Some("fix-x")));
    }

    #[test]
    fn plain_folder_uses_its_name() {
        let root = TempDir::new().unwrap();
        let notes = root.path().join("notes");
        fs::create_dir(&notes).unwrap();
        assert_eq!(Projects::new().project_for(&notes), project("notes", None));
    }

    #[test]
    fn names_are_cached_per_cwd() {
        let root = TempDir::new().unwrap();
        let api = repo(&root, "api");
        let mut projects = Projects::new();
        projects.project_for(&api);
        fs::rename(&api, root.path().join("renamed")).unwrap();
        assert_eq!(projects.project_for(&api).name, "api");
    }
}
