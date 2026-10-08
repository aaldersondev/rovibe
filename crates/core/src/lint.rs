//! Static checks on a project's code, so that an agent hears about a typo or
//! a type error from a tool call instead of from a failed playtest.
//!
//! Two checkers, both optional: selene (lints, undefined names) and luau-lsp
//! (type errors, with Roblox's API and the project's own tree known to it).

use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use tokio::process::Command;

use crate::state::{Project, Shared};

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// Roblox's globals, minus a rule about layout: on real game code it buried
/// every actual finding under hundreds of "one statement per line".
const DEFAULT_SELENE_CONFIG: &str = "std = \"roblox\"\n\n[lints]\nmultiple_statements = \"allow\"\n";

pub struct Diagnostic {
    pub error: bool,
    /// Name of the lint, for findings that have one.
    pub rule: Option<String>,
    /// `path:line: message`, path relative to the project.
    pub text: String,
}

/// Shipped next to the app, or whatever the user has on their PATH.
fn tools_dir() -> Option<PathBuf> {
    // The server may run from a copy of itself, with the tools left where
    // the app was installed.
    if let Some(dir) = std::env::var_os("ROVIBE_TOOLS_DIR") {
        return Some(PathBuf::from(dir));
    }
    Some(std::env::current_exe().ok()?.parent()?.join("tools"))
}

fn locate(name: &str) -> Option<PathBuf> {
    let bundled = tools_dir().map(|dir| dir.join(format!("{name}.exe")));
    bundled
        .filter(|path| path.exists())
        .or_else(|| which::which(name).ok())
}

pub fn available() -> (bool, bool) {
    (locate("selene").is_some(), locate("luau-lsp").is_some())
}

async fn run(mut command: Command, limit: Duration) -> Option<String> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);

    let output = tokio::time::timeout(limit, command.output()).await.ok()?.ok()?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    Some(text)
}

/// A project's own `selene.toml` wins; without one, Roblox's globals are
/// assumed so that `game` and `workspace` aren't reported as undefined.
fn selene_config(state: &Shared, project: &Project) -> PathBuf {
    let own = project.path.join("selene.toml");
    if own.exists() {
        return own;
    }
    // Rewritten every time, so an updated app updates its defaults.
    let fallback = state.data_dir.join("selene.toml");
    let _ = std::fs::write(&fallback, DEFAULT_SELENE_CONFIG);
    fallback
}

/// Runs selene on files or folders of the project. `None` when selene isn't
/// installed or didn't answer in time.
pub async fn selene(state: &Shared, project: &Project, targets: &[PathBuf], limit: Duration) -> Option<Vec<Diagnostic>> {
    let mut command = Command::new(locate("selene")?);
    command
        .current_dir(&project.path)
        .arg("--config")
        .arg(selene_config(state, project))
        .args(["--display-style", "quiet"])
        .args(targets);

    let output = run(command, limit).await?;
    Some(output.lines().filter_map(|line| parse_selene(project, line)).collect())
}

/// `path:line:col: severity[rule]: message`
fn parse_selene(project: &Project, line: &str) -> Option<Diagnostic> {
    let (location, rest) = line.split_once(": ")?;
    let (severity, message) = rest.split_once("]: ")?;
    let (severity, rule) = severity.split_once('[')?;
    let mut parts = location.rsplitn(3, ':');
    let (_column, line_number, path) = (parts.next()?, parts.next()?, parts.next()?);
    Some(Diagnostic {
        error: severity == "error",
        rule: Some(rule.to_owned()),
        text: format!("{}:{line_number}: {message} [{rule}]", strip_root(project, path)),
    })
}

fn strip_root(project: &Project, path: &str) -> String {
    let normalized = path.replace('\\', "/");
    let root = project.path.to_string_lossy().replace('\\', "/");
    if normalized.to_lowercase().starts_with(&root.to_lowercase()) {
        normalized
            .chars()
            .skip(root.chars().count())
            .collect::<String>()
            .trim_start_matches('/')
            .to_owned()
    } else {
        normalized
    }
}

/// Type-checks the project with luau-lsp. Lint findings it also prints are
/// dropped: selene already reports those, with better names.
pub async fn types(state: &Shared, project: &Project, targets: &[PathBuf]) -> Option<Vec<Diagnostic>> {
    let checker = locate("luau-lsp")?;
    let dir = state.data_dir.join("lint");
    std::fs::create_dir_all(&dir).ok()?;

    let mut command = Command::new(&checker);
    // Without this the checker turns on every experimental flag, among them
    // a type solver that reported fields as missing from tables that declare
    // them, on code that runs fine.
    command
        .current_dir(&project.path)
        .args(["analyze", "--formatter=plain", "--no-flags-enabled"]);

    if let Some(definitions) = tools_dir().map(|dir| dir.join("globalTypes.d.luau")).filter(|path| path.exists()) {
        command.arg(format!("--definitions=@roblox={}", definitions.display()));
    }

    // The sourcemap is what makes `script.Parent.Foo` and
    // `ReplicatedStorage.Shared.Bar` resolve to the right modules.
    if let Some(sync) = crate::sync::binary() {
        let sourcemap = dir.join(format!("{}.json", project.id));
        let mut build = Command::new(sync);
        build
            .current_dir(&project.path)
            .arg("sourcemap")
            .arg("default.project.json")
            .arg("-o")
            .arg(&sourcemap);
        if run(build, Duration::from_secs(20)).await.is_some() && sourcemap.exists() {
            command.arg(format!("--sourcemap={}", sourcemap.display()));
        }
    }
    command.args(targets);

    let output = run(command, Duration::from_secs(60)).await?;
    Some(output.lines().filter_map(|line| parse_types(project, line)).collect())
}

/// `path [game/…]:line:col-col: (W0) TypeError: message`. Lint findings use
/// the same shape with another label, and are left to selene.
fn parse_types(project: &Project, line: &str) -> Option<Diagnostic> {
    let (location, message) = line.split_once(": (")?;
    let (_, message) = message.split_once(") ")?;
    // On untyped game code the type checker mostly reports fields a table
    // gains after it is created: worth a look, not worth stopping an agent.
    // A file that doesn't parse is another matter.
    let (message, syntax) = match message.strip_prefix("SyntaxError: ") {
        Some(message) => (message, true),
        None => (message.strip_prefix("TypeError: ")?, false),
    };
    let mut parts = location.rsplitn(3, ':');
    let (_columns, line_number, path) = (parts.next()?, parts.next()?, parts.next()?);
    let path = path.split(" [").next().unwrap_or(path);
    Some(Diagnostic {
        error: syntax,
        rule: Some(if syntax { "syntax" } else { "type" }.to_owned()),
        text: format!("{}:{line_number}: {message}", strip_root(project, path)),
    })
}

/// The `check_code` tool: both checkers over the given paths, errors first.
pub async fn check(state: &Shared, project: &Project, paths: &[String], with_warnings: bool) -> Result<String, String> {
    let targets: Vec<PathBuf> = if paths.is_empty() {
        vec![PathBuf::from("src")]
    } else {
        paths.iter().map(PathBuf::from).collect()
    };
    for target in &targets {
        if !project.path.join(target).exists() {
            return Err(format!("{} n'existe pas dans le projet", target.display()));
        }
    }

    let (lints, typed) = tokio::join!(
        selene(state, project, &targets, Duration::from_secs(60)),
        types(state, project, &targets)
    );
    if lints.is_none() && typed.is_none() {
        return Err("Aucun vérificateur n'est installé (selene, luau-lsp) : lance scripts\\get-tools.ps1".into());
    }

    // Both checkers report an undefined name; selene's wording names the rule.
    let has_lints = lints.is_some();
    let typed = typed
        .into_iter()
        .flatten()
        .filter(|diagnostic| !(has_lints && diagnostic.text.contains("Unknown global")));

    let mut all: Vec<Diagnostic> = lints.into_iter().flatten().chain(typed).collect();
    all.sort_by(|a, b| b.error.cmp(&a.error).then_with(|| a.text.cmp(&b.text)));
    all.dedup_by(|a, b| a.text == b.text);

    let errors = all.iter().filter(|diagnostic| diagnostic.error).count();
    let warnings = all.len() - errors;
    if all.is_empty() {
        return Ok("Aucun problème trouvé.".into());
    }

    let mut text = format!("{errors} erreur(s), {warnings} avertissement(s)\n");
    let listed: Vec<&Diagnostic> = all
        .iter()
        .filter(|diagnostic| diagnostic.error || with_warnings)
        .collect();
    for diagnostic in listed.iter().take(80) {
        text.push_str(if diagnostic.error { "erreur  " } else { "avert.  " });
        text.push_str(&diagnostic.text);
        text.push('\n');
    }
    if listed.len() > 80 {
        text.push_str(&format!("… {} autres : restreins `paths`\n", listed.len() - 80));
    }

    // Warnings are counted by kind rather than listed: on a real project
    // there are hundreds, and the errors are what must not be missed.
    if !with_warnings && warnings > 0 {
        let mut by_rule: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
        for diagnostic in all.iter().filter(|diagnostic| !diagnostic.error) {
            *by_rule.entry(diagnostic.rule.as_deref().unwrap_or("autre")).or_default() += 1;
        }
        let mut kinds: Vec<_> = by_rule.into_iter().collect();
        kinds.sort_by(|a, b| b.1.cmp(&a.1));
        let summary: Vec<String> = kinds.iter().map(|(rule, count)| format!("{rule} ×{count}")).collect();
        text.push_str(&format!(
            "Avertissements : {}. Passe `warnings: true` pour les lister.\n",
            summary.join(", ")
        ));
    }
    Ok(text)
}

/// Checks one file right after an agent wrote it. Only errors come back:
/// this interrupts the agent, which warnings aren't worth.
pub async fn after_edit(state: &Shared, project: &Project, file: &Path) -> Option<String> {
    let is_luau = file
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| matches!(extension, "luau" | "lua"));
    if !is_luau || !file.starts_with(&project.path) || !file.exists() {
        return None;
    }

    let found = selene(state, project, &[file.to_path_buf()], Duration::from_secs(5)).await?;
    let errors: Vec<&str> = found
        .iter()
        .filter(|diagnostic| diagnostic.error)
        .map(|diagnostic| diagnostic.text.as_str())
        .take(10)
        .collect();
    (!errors.is_empty()).then(|| {
        format!(
            "selene signale des erreurs dans le fichier que tu viens d'écrire :\n{}\nCorrige-les avant de continuer.",
            errors.join("\n")
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> Project {
        Project {
            id: "p1".into(),
            name: "Jeu".into(),
            path: PathBuf::from(r"C:\Jeux\Demo"),
            sync_port: 34873,
            place_id: None,
            place_name: None,
            protected: false,
        }
    }

    #[test]
    fn selene_findings_keep_their_severity_and_rule() {
        let error = parse_selene(&project(), r"src\shared\Bad.luau:3:7: error[undefined_variable]: `ghost` is not defined").unwrap();
        assert!(error.error);
        assert_eq!(error.text, "src/shared/Bad.luau:3: `ghost` is not defined [undefined_variable]");

        let warning = parse_selene(&project(), "src/a.luau:1:7: warning[unused_variable]: x is assigned a value, but never used").unwrap();
        assert!(!warning.error);
    }

    #[test]
    fn selene_paths_given_in_full_are_shortened_to_the_project() {
        let found = parse_selene(&project(), r"C:\Jeux\Demo\src\a.luau:2:1: error[parse_error]: unexpected token").unwrap();
        assert_eq!(found.text, "src/a.luau:2: unexpected token [parse_error]");
    }

    #[test]
    fn selene_summary_lines_are_not_findings() {
        for line in ["Results:", "1 errors", "3 warnings", "0 parse errors", ""] {
            assert!(parse_selene(&project(), line).is_none(), "{line}");
        }
    }

    #[test]
    fn type_errors_are_read_and_lints_are_left_to_selene() {
        let line = r"c:\Jeux\Demo\src\shared\Bad.luau [game/ReplicatedStorage/Shared/Bad]:4:1-25: (W0) TypeError: Expected this to be 'number', but got 'string'";
        let found = parse_types(&project(), line).unwrap();
        // A type finding is a warning; only a file that doesn't parse is an error.
        assert!(!found.error);
        assert_eq!(found.rule.as_deref(), Some("type"));
        let broken = parse_types(&project(), "src/a.luau:9:1-4: (E0) SyntaxError: Expected 'end'").unwrap();
        assert!(broken.error);
        assert_eq!(found.text, "src/shared/Bad.luau:4: Expected this to be 'number', but got 'string'");

        let lint = "src/shared/Bad.luau:1:7-7: (W0) LocalUnused: Variable 'x' is never used; prefix with '_' to silence";
        assert!(parse_types(&project(), lint).is_none());
        assert!(parse_types(&project(), "[INFO] Loading definitions file: @roblox - C:/x/globalTypes.d.luau").is_none());
    }
}
