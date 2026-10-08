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

const FIRST_COMMIT: &str = "Création du projet";
const REVIEWED: &str = "refs/rovibe/reviewed";

const IGNORE: &str = "# Places Studio : binaires, à enregistrer depuis Studio\n*.rbxl\n*.rbxlx\n*.rbxl.lock\n*.rbxlx.lock\n";

#[derive(Serialize)]
pub struct Commit {
    pub hash: String,
    pub subject: String,
    pub when: String,
}

#[derive(Serialize)]
pub struct History {
    /// False when the folder isn't a repository root RoVibe manages.
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
        args.extend(["-c", "user.name=RoVibe", "-c", "user.email=rovibe@localhost"]);
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
    commit(dir, FIRST_COMMIT).await?;
    git(dir, &["update-ref", REVIEWED, "HEAD"]).await.map(|_| ())
}

/// Carries over the review mark of a project from the name it had in an
/// earlier version, so what awaited the user's review still does.
pub async fn adopt_review_mark(dir: &Path, old: &str) {
    if !is_root(dir) || git(dir, &["rev-parse", "--verify", "--quiet", REVIEWED]).await.is_ok() {
        return;
    }
    if git(dir, &["update-ref", REVIEWED, old]).await.is_ok() {
        let _ = git(dir, &["update-ref", "-d", old]).await;
    }
}

/// Paths with uncommitted changes, relative to the project. `None` when the
/// project isn't a repository of its own.
pub async fn dirty_paths(dir: &Path) -> Option<std::collections::HashSet<String>> {
    if !is_root(dir) {
        return None;
    }
    let status = git(dir, &["status", "--porcelain", "-uall"]).await.ok()?;
    Some(
        status
            .lines()
            // `XY path`, or `XY old -> new` for a rename.
            .filter_map(|line| line.get(3..))
            .map(|path| path.rsplit(" -> ").next().unwrap_or(path).trim_matches('"').to_owned())
            .collect(),
    )
}

#[derive(Serialize)]
pub struct Change {
    pub path: String,
    /// `added`, `modified` or `deleted`.
    pub status: &'static str,
    pub added: u64,
    pub deleted: u64,
}

/// The state the user last looked at, moved forward when they accept the
/// changes. Agents commit their own work, so comparing with the last commit
/// would show nothing: the review is against this mark instead.
async fn reviewed(dir: &Path) -> Result<(), String> {
    if !is_root(dir) {
        return Err("Ce projet n'a pas son propre dépôt git".into());
    }
    if git(dir, &["rev-parse", "--verify", "--quiet", REVIEWED]).await.is_ok() {
        return Ok(());
    }
    // A project created before this mark existed starts from its creation;
    // a repository the user brought along starts from where it is now.
    let root = git(dir, &["rev-list", "--max-parents=0", "HEAD"]).await?;
    let root = root.lines().last().unwrap_or("HEAD").to_owned();
    let subject = git(dir, &["log", "-1", "--pretty=format:%s", &root]).await.unwrap_or_default();
    let start = if subject == FIRST_COMMIT { root.as_str() } else { "HEAD" };
    git(dir, &["update-ref", REVIEWED, start]).await.map(|_| ())
}

fn safe(path: &str) -> Result<(), String> {
    let escapes = path.is_empty()
        || path.starts_with(['/', '\\', '-'])
        || path.contains(':')
        || path.split(['/', '\\']).any(|part| part == "..");
    if escapes {
        Err("Chemin invalide".into())
    } else {
        Ok(())
    }
}

/// Everything that differs from the last reviewed state, committed or not.
pub async fn changes(dir: &Path) -> Result<Vec<Change>, String> {
    reviewed(dir).await?;

    let statuses = git(dir, &["diff", "--name-status", "--no-renames", REVIEWED]).await?;
    let counts = git(dir, &["diff", "--numstat", "--no-renames", REVIEWED]).await?;
    let count_of = |path: &str| {
        counts
            .lines()
            .filter_map(|line| {
                let mut fields = line.splitn(3, '\t');
                let (added, deleted, name) = (fields.next()?, fields.next()?, fields.next()?);
                (name == path).then(|| (added.parse().unwrap_or(0), deleted.parse().unwrap_or(0)))
            })
            .next()
            .unwrap_or((0, 0))
    };

    let mut found: Vec<Change> = statuses
        .lines()
        .filter_map(|line| {
            let (letter, path) = line.split_once('\t')?;
            let (added, deleted) = count_of(path);
            Some(Change {
                path: path.to_owned(),
                status: match letter {
                    "A" => "added",
                    "D" => "deleted",
                    _ => "modified",
                },
                added,
                deleted,
            })
        })
        .collect();

    // Files git doesn't know yet are changes too.
    for path in git(dir, &["ls-files", "--others", "--exclude-standard"]).await?.lines() {
        let lines = std::fs::read_to_string(dir.join(path))
            .map(|text| text.lines().count() as u64)
            .unwrap_or(0);
        found.push(Change { path: path.to_owned(), status: "added", added: lines, deleted: 0 });
    }
    found.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(found)
}

pub async fn diff(dir: &Path, path: &str) -> Result<String, String> {
    safe(path)?;
    reviewed(dir).await?;
    let tracked = git(dir, &["diff", "--no-renames", REVIEWED, "--", path]).await?;
    if !tracked.is_empty() {
        return Ok(tracked);
    }
    // An untracked file has no diff: all of it is new.
    let text = std::fs::read_to_string(dir.join(path)).map_err(|error| error.to_string())?;
    Ok(text.lines().map(|line| format!("+{line}\n")).collect())
}

/// The user has seen the changes and keeps them.
pub async fn accept(dir: &Path) -> Result<String, String> {
    reviewed(dir).await?;
    snapshot(dir, "Changements relus").await?;
    git(dir, &["update-ref", REVIEWED, "HEAD"]).await?;
    Ok("Changements acceptés.".into())
}

/// Puts one file back to the last reviewed state; a file that didn't exist
/// then is removed.
pub async fn revert(dir: &Path, path: &str) -> Result<String, String> {
    safe(path)?;
    reviewed(dir).await?;
    let existed = git(dir, &["cat-file", "-e", &format!("{REVIEWED}:{path}")]).await.is_ok();
    if existed {
        git(dir, &["checkout", REVIEWED, "--", path]).await?;
    } else {
        let _ = git(dir, &["rm", "--quiet", "--cached", "--force", "--", path]).await;
        std::fs::remove_file(dir.join(path)).map_err(|error| error.to_string())?;
    }
    Ok(format!("{path} est revenu à son état relu."))
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
    async fn the_review_shows_what_changed_since_the_user_last_looked() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("Shop.luau");
        std::fs::write(&file, "local a = 1\nreturn a\n").unwrap();
        std::fs::write(dir.path().join("Old.luau"), "return 0\n").unwrap();
        ensure_repo(dir.path()).await.unwrap();
        assert!(changes(dir.path()).await.unwrap().is_empty());

        // An agent edits and commits, another leaves files uncommitted.
        std::fs::write(&file, "local a = 2\nlocal b = 3\nreturn a + b\n").unwrap();
        snapshot(dir.path(), "travail d'un agent").await.unwrap();
        std::fs::write(dir.path().join("New.luau"), "return {}\n").unwrap();
        std::fs::remove_file(dir.path().join("Old.luau")).unwrap();

        let found = changes(dir.path()).await.unwrap();
        let summary: Vec<(&str, &str, u64, u64)> = found
            .iter()
            .map(|change| (change.path.as_str(), change.status, change.added, change.deleted))
            .collect();
        assert_eq!(
            summary,
            [("New.luau", "added", 1, 0), ("Old.luau", "deleted", 0, 1), ("Shop.luau", "modified", 3, 2)]
        );
        assert!(diff(dir.path(), "Shop.luau").await.unwrap().contains("+local b = 3"));
        assert_eq!(diff(dir.path(), "New.luau").await.unwrap(), "+return {}\n");

        // Undoing one file at a time: an edit, a deletion, a new file.
        revert(dir.path(), "Shop.luau").await.unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "local a = 1\nreturn a\n");
        revert(dir.path(), "Old.luau").await.unwrap();
        assert!(dir.path().join("Old.luau").exists());
        revert(dir.path(), "New.luau").await.unwrap();
        assert!(!dir.path().join("New.luau").exists());
        assert!(changes(dir.path()).await.unwrap().is_empty());

        // Accepting moves the mark: what was new is now the reference.
        std::fs::write(&file, "return 42\n").unwrap();
        assert_eq!(changes(dir.path()).await.unwrap().len(), 1);
        accept(dir.path()).await.unwrap();
        assert!(changes(dir.path()).await.unwrap().is_empty());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "return 42\n");
    }

    #[tokio::test]
    async fn a_review_path_cannot_leave_the_project() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.luau"), "1").unwrap();
        ensure_repo(dir.path()).await.unwrap();
        for bad in ["../x", "C:/Windows/win.ini", "/etc/passwd", "--output=x", "a/../../b", ""] {
            assert!(diff(dir.path(), bad).await.is_err(), "{bad}");
            assert!(revert(dir.path(), bad).await.is_err(), "{bad}");
        }
    }

    #[tokio::test]
    async fn files_a_command_dirtied_can_be_told_from_those_already_dirty() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.luau"), "1").unwrap();
        std::fs::write(dir.path().join("b.luau"), "1").unwrap();
        ensure_repo(dir.path()).await.unwrap();

        std::fs::write(dir.path().join("a.luau"), "2").unwrap();
        let before = dirty_paths(dir.path()).await.unwrap();
        std::fs::write(dir.path().join("b.luau"), "2").unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub/c.luau"), "3").unwrap();
        let after = dirty_paths(dir.path()).await.unwrap();

        let mut new: Vec<&String> = after.difference(&before).collect();
        new.sort();
        assert_eq!(new, ["b.luau", "sub/c.luau"]);
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
