//! Version history for a project: every risky step starts from a commit the
//! user can come back to. Only acts on projects that are their own repository
//! root, so a project nested in someone else's repository is left alone.

use std::path::Path;

use serde::Serialize;
use tokio::process::Command;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Files come back from git byte for byte. With Windows' usual line-ending
/// conversion, restoring a script would rewrite every line of it, and the
/// sync server would then push that as a change to Studio.
const ATTRIBUTES: &str = "* -text\n";

const IGNORE: &str = "# Places Studio : binaires, à enregistrer depuis Studio\n*.rbxl\n*.rbxlx\n*.rbxl.lock\n*.rbxlx.lock\n";

#[derive(Serialize)]
pub struct Commit {
    pub hash: String,
    pub subject: String,
    pub when: String,
}

#[derive(Serialize)]
pub struct History {
    /// False when the folder isn't a repository root Essaim manages.
    pub enabled: bool,
    pub dirty: usize,
    pub commits: Vec<Commit>,
}

async fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let mut command = Command::new("git");
    command.current_dir(dir).args(args);
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);

    let output = command
        .output()
        .await
        .map_err(|error| format!("git est introuvable : {error}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim_end().to_owned())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
    }
}

fn is_root(dir: &Path) -> bool {
    dir.join(".git").exists()
}

/// Commits need an author; a machine where git was never configured still
/// has to be able to take a snapshot.
async fn commit(dir: &Path, message: &str) -> Result<(), String> {
    let configured = git(dir, &["config", "user.email"]).await.is_ok_and(|email| !email.is_empty());
    let mut args = Vec::new();
    if !configured {
        args.extend(["-c", "user.name=Essaim", "-c", "user.email=essaim@localhost"]);
    }
    args.extend(["commit", "--quiet", "-m", message]);
    git(dir, &args).await.map(|_| ())
}

/// Turns a new project into a repository with a first commit. Does nothing
/// when the folder already belongs to a repository, its own or a parent's.
pub async fn ensure_repo(dir: &Path) -> Result<(), String> {
    if is_root(dir) || git(dir, &["rev-parse", "--is-inside-work-tree"]).await.is_ok() {
        return Ok(());
    }

    git(dir, &["init", "--quiet"]).await?;
    git(dir, &["config", "core.autocrlf", "false"]).await?;
    for (name, content) in [(".gitignore", IGNORE), (".gitattributes", ATTRIBUTES)] {
        let path = dir.join(name);
        if !path.exists() {
            std::fs::write(path, content).map_err(|error| error.to_string())?;
        }
    }
    git(dir, &["add", "-A"]).await?;
    commit(dir, "Création du projet").await
}

/// Commits everything that changed. Returns false when there was nothing to
/// commit.
pub async fn snapshot(dir: &Path, message: &str) -> Result<bool, String> {
    if !is_root(dir) {
        return Err("Ce projet n'a pas son propre dépôt git".into());
    }
    git(dir, &["add", "-A"]).await?;
    if git(dir, &["status", "--porcelain"]).await?.is_empty() {
        return Ok(false);
    }
    commit(dir, message).await?;
    Ok(true)
}

pub async fn history(dir: &Path) -> History {
    if !is_root(dir) {
        return History { enabled: false, dirty: 0, commits: Vec::new() };
    }

    let dirty = git(dir, &["status", "--porcelain"])
        .await
        .map(|status| status.lines().count())
        .unwrap_or(0);

    // The unit separator can't appear in a subject line.
    let commits = git(dir, &["log", "-40", "--pretty=format:%H%x1f%s%x1f%cr"])
        .await
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let mut fields = line.split('\u{1f}');
            Some(Commit {
                hash: fields.next()?.to_owned(),
                subject: fields.next()?.to_owned(),
                when: fields.next()?.to_owned(),
            })
        })
        .collect();

    History { enabled: true, dirty, commits }
}

/// Brings the files back to how they were at `hash`, as a new commit on top
/// of the history: nothing is rewritten, so the restore can itself be undone.
pub async fn restore(dir: &Path, hash: &str) -> Result<String, String> {
    let valid = (7..=40).contains(&hash.len()) && hash.chars().all(|c| c.is_ascii_hexdigit());
    if !valid {
        return Err("Identifiant de commit invalide".into());
    }

    let subject = git(dir, &["log", "-1", "--pretty=format:%s", hash]).await?;
    let short = &hash[..7];
    snapshot(dir, &format!("Point de sauvegarde avant le retour à {short}")).await?;

    git(dir, &["read-tree", "-u", "--reset", hash]).await?;
    if git(dir, &["status", "--porcelain"]).await?.is_empty() {
        return Ok("Les fichiers sont déjà dans cet état.".into());
    }
    commit(dir, &format!("Retour à {short} : {subject}")).await?;
    Ok(format!("Fichiers revenus à « {subject} »."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_restore_brings_files_back_and_can_itself_be_undone() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("Economy.luau");
        std::fs::write(&file, "return 10\n").unwrap();

        ensure_repo(dir.path()).await.unwrap();
        let first = history(dir.path()).await;
        assert!(first.enabled);
        assert_eq!((first.dirty, first.commits.len()), (0, 1));
        let origin = first.commits[0].hash.clone();

        std::fs::write(&file, "return 999\n").unwrap();
        std::fs::write(dir.path().join("New.luau"), "return {}\n").unwrap();
        assert_eq!(history(dir.path()).await.dirty, 2);
        assert!(snapshot(dir.path(), "Economie à 999").await.unwrap());
        // Nothing changed since: no empty commit.
        assert!(!snapshot(dir.path(), "Rien").await.unwrap());

        restore(dir.path(), &origin).await.unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "return 10\n");
        assert!(!dir.path().join("New.luau").exists());

        // History only grows, so the state before the restore is still there.
        let after = history(dir.path()).await;
        assert_eq!(after.commits.len(), 3);
        assert_eq!(after.commits[1].subject, "Economie à 999");
        let undone = after.commits[1].hash.clone();
        restore(dir.path(), &undone).await.unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "return 999\n");
    }

    #[tokio::test]
    async fn unsaved_work_is_committed_before_a_restore() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.luau");
        std::fs::write(&file, "1").unwrap();
        ensure_repo(dir.path()).await.unwrap();
        let origin = history(dir.path()).await.commits[0].hash.clone();

        std::fs::write(&file, "2").unwrap();
        restore(dir.path(), &origin).await.unwrap();

        let subjects: Vec<String> = history(dir.path()).await.commits.into_iter().map(|commit| commit.subject).collect();
        assert_eq!(subjects.len(), 3);
        assert!(subjects[1].starts_with("Point de sauvegarde avant le retour"));
    }

    #[tokio::test]
    async fn anything_that_is_not_a_commit_id_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.luau"), "1").unwrap();
        ensure_repo(dir.path()).await.unwrap();

        for bad in ["", "HEAD", "abc", "--help", "1234567; rm -rf ."] {
            assert!(restore(dir.path(), bad).await.is_err(), "{bad}");
        }
    }

    #[tokio::test]
    async fn a_folder_inside_someone_elses_repository_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.luau"), "1").unwrap();
        ensure_repo(dir.path()).await.unwrap();

        let nested = dir.path().join("games/mine");
        std::fs::create_dir_all(&nested).unwrap();
        ensure_repo(&nested).await.unwrap();

        assert!(!nested.join(".git").exists());
        assert!(!history(&nested).await.enabled);
        assert!(snapshot(&nested, "x").await.is_err());
    }
}
