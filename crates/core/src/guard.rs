//! What a tool call is about to touch, read from its input before it runs:
//! the files of a patch, and whether a shell command would write over files
//! another agent is holding.

/// Files named by a Codex `apply_patch` call, in the order they appear.
pub fn patch_paths(patch: &str) -> Vec<String> {
    const MARKERS: [&str; 4] = [
        "*** Add File: ",
        "*** Update File: ",
        "*** Delete File: ",
        "*** Move to: ",
    ];
    patch
        .lines()
        .filter_map(|line| {
            MARKERS
                .iter()
                .find_map(|marker| line.strip_prefix(marker))
                .map(|path| path.trim().to_owned())
        })
        .filter(|path| !path.is_empty())
        .collect()
}

/// Git commands that rewrite the whole working tree. With several agents in
/// one folder, they throw away everyone's unsaved work, not just the caller's.
const SWEEPING: &[&str] = &[
    "git reset --hard",
    "git checkout .",
    "git checkout -- .",
    "git checkout -f",
    "git restore .",
    "git restore --staged --worktree .",
    "git restore --worktree .",
    "git clean",
    "git stash",
];

/// Words that mark a command as writing to disk. A command that only reads
/// a held file (cat, grep, git diff) is nobody's business.
const WRITES: &[&str] = &[
    ">", "sed -i", "perl -i", "tee ", "rm ", "mv ", "cp ", "del ", "move ", "copy ", "truncate ",
    "set-content", "add-content", "out-file", "remove-item", "move-item", "copy-item",
    "rename-item", "clear-content", "new-item", "git checkout", "git restore", "git rm", "git mv",
];

fn squeeze(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

/// Why a shell command must not run while other agents hold `held` (their
/// paths, relative to the project, with who holds each), or `None` if it may.
pub fn shell_conflict(command: &str, held: &[(String, String)]) -> Option<String> {
    if held.is_empty() {
        return None;
    }
    let squeezed = squeeze(command);

    if let Some(sweeping) = SWEEPING.iter().find(|pattern| {
        // `git stash list` and `git stash show` only read.
        squeezed.contains(**pattern)
            && !(pattern.ends_with("stash")
                && (squeezed.contains("git stash list") || squeezed.contains("git stash show")))
    }) {
        let holders: Vec<&str> = held.iter().map(|(_, holder)| holder.as_str()).collect();
        return Some(format!(
            "`{sweeping}` réécrirait tout le dossier, y compris les fichiers en cours de {}. Limite la commande à tes propres fichiers.",
            unique(&holders).join(" et ")
        ));
    }

    if !WRITES.iter().any(|word| squeezed.contains(word)) {
        return None;
    }
    // Either slash style, as agents write paths both ways on Windows.
    let both_ways = squeezed.replace('\\', "/");
    held.iter()
        .find(|(path, _)| both_ways.contains(&path.to_lowercase()))
        .map(|(path, holder)| {
            format!(
                "{holder} travaille sur {path} : cette commande le modifierait. Avance sur une autre partie de ta tâche, ou réessaie plus tard."
            )
        })
}

fn unique<'a>(names: &[&'a str]) -> Vec<&'a str> {
    let mut seen = Vec::new();
    for name in names {
        if !seen.contains(name) {
            seen.push(*name);
        }
    }
    seen
}

#[cfg(test)]
mod tests {
    use super::*;

    fn held() -> Vec<(String, String)> {
        vec![
            ("src/server/Shop.luau".to_owned(), "Claude Code #2".to_owned()),
            ("src/shared".to_owned(), "Codex #3".to_owned()),
        ]
    }

    #[test]
    fn a_patch_names_every_file_it_touches() {
        let patch = "*** Begin Patch\n*** Add File: src/a.luau\n+return 1\n*** Update File: C:/Jeux/Demo/src/b.luau\n@@\n-x\n+y\n*** Move to: src/c.luau\n*** Delete File: src/d.luau\n*** End Patch";
        assert_eq!(
            patch_paths(patch),
            ["src/a.luau", "C:/Jeux/Demo/src/b.luau", "src/c.luau", "src/d.luau"]
        );
        assert!(patch_paths("echo *** Add File:").is_empty());
    }

    #[test]
    fn reading_a_held_file_is_allowed() {
        for command in [
            "cat src/server/Shop.luau",
            "git diff src/server/Shop.luau",
            "Get-Content src\\server\\Shop.luau | Select-String cart",
            "grep -n price src/shared/Config.luau",
        ] {
            assert_eq!(shell_conflict(command, &held()), None, "{command}");
        }
    }

    #[test]
    fn writing_to_a_held_file_is_refused_with_the_holders_name() {
        for command in [
            "echo x > src/server/Shop.luau",
            "sed -i 's/a/b/' src/server/Shop.luau",
            "Set-Content -LiteralPath 'src\\server\\Shop.luau' -Value 'x'",
            "git checkout HEAD -- SRC/Server/Shop.luau",
            "rm src/server/Shop.luau",
        ] {
            let refusal = shell_conflict(command, &held()).unwrap_or_else(|| panic!("{command}"));
            assert!(refusal.contains("Claude Code #2"), "{refusal}");
        }
        // A held folder covers the files in it.
        let refusal = shell_conflict("mv src/shared/Config.luau src/shared/Old.luau", &held()).unwrap();
        assert!(refusal.contains("Codex #3"));
    }

    #[test]
    fn writing_elsewhere_is_allowed() {
        for command in ["echo x > notes.txt", "rm src/client/Old.luau", "git add -A && git commit -m ok"] {
            assert_eq!(shell_conflict(command, &held()), None, "{command}");
        }
    }

    #[test]
    fn commands_that_rewrite_the_whole_folder_are_refused() {
        for command in ["git reset --hard", "git  checkout  .", "git stash", "git clean -fd", "cd src && git restore ."] {
            let refusal = shell_conflict(command, &held()).unwrap_or_else(|| panic!("{command}"));
            assert!(refusal.contains("Claude Code #2") && refusal.contains("Codex #3"), "{refusal}");
        }
        assert_eq!(shell_conflict("git stash list", &held()), None);
        assert_eq!(shell_conflict("git checkout -b feature", &held()), None);
    }

    #[test]
    fn alone_on_the_project_anything_goes() {
        assert_eq!(shell_conflict("git reset --hard", &[]), None);
    }
}
