//! Help for keyd (https://github.com/rvaiya/keyd), the program that remaps keys.
//!
//! keyd grabs the keyboards exclusively and puts them back together as one virtual keyboard: while
//! it holds one, no other program can grab it or tell it apart from the others. Here we prepare the
//! change that frees it (the `-vendor:product` line under `[ids]` in the configuration file) and apply
//! it as administrator, then restart keyd.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Where keyd keeps its configuration.
pub const CONFIG_DIR: &str = "/etc/keyd";

/// Above this size the file is not passed as an argument to the command: better not to touch it.
const MAX_CONFIG_BYTES: usize = 100_000;

/// How to free a keyboard from keyd.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fix {
    /// keyd configuration file.
    pub file: PathBuf,
    /// New content of the file; `None` if the configuration is already fine and keyd only needs a restart.
    pub new_content: Option<String>,
    /// What is done to the file, to show the user (starts with a verb: "add...").
    pub summary: String,
    /// The same, as a command to paste into a terminal.
    pub command: String,
}

/// What has to change in a keyd file for it to leave a keyboard alone.
#[derive(Debug, PartialEq, Eq)]
enum Edit {
    /// The file does not concern this keyboard.
    NotInFile,
    /// It already excludes it: keyd only needs a restart.
    AlreadyExcluded,
    Changed {
        content: String,
        /// Added the `-id` line under a `*`.
        added: bool,
        /// Removed the line with the id, because the keyboard was listed by hand.
        removed: bool,
        /// The wildcard found (`*` or `k:*`).
        wildcard: Option<String>,
    },
}

/// Looks at the `[ids]` section of a keyd file. `id` has the format `1a2c:9b09`.
fn edit_ids(config: &str, id: &str) -> Edit {
    let lines: Vec<&str> = config.split_inclusive('\n').collect();
    let excluded = format!("-{id}");
    let mut in_ids = false;
    let mut wildcard: Option<String> = None;
    let mut last_id_line: Option<usize> = None;
    let mut listed: Vec<usize> = Vec::new();

    for (i, raw) in lines.iter().enumerate() {
        let line = raw.trim();
        if line.starts_with('[') && line.ends_with(']') {
            in_ids = line.eq_ignore_ascii_case("[ids]");
            continue;
        }
        if !in_ids || line.is_empty() || line.starts_with('#') {
            continue;
        }
        last_id_line = Some(i);
        if line.eq_ignore_ascii_case(&excluded) {
            return Edit::AlreadyExcluded;
        } else if line.eq_ignore_ascii_case(id) {
            listed.push(i);
        } else if line == "*" || line == "k:*" {
            wildcard = Some(line.to_string());
        }
    }

    let add = wildcard.is_some();
    if listed.is_empty() && !add {
        return Edit::NotInFile;
    }
    let mut content = String::with_capacity(config.len() + excluded.len() + 1);
    for (i, line) in lines.iter().enumerate() {
        if !listed.contains(&i) {
            content.push_str(line);
        }
        if add && Some(i) == last_id_line {
            if !content.is_empty() && !content.ends_with('\n') {
                content.push('\n');
            }
            content.push_str(&excluded);
            content.push('\n');
        }
    }
    Edit::Changed {
        content,
        added: add,
        removed: !listed.is_empty(),
        wildcard,
    }
}

/// The wildcard as it is written in a `sed` expression.
fn sed_pattern(wildcard: &str) -> String {
    wildcard.replace('*', "\\*")
}

fn fix_from(file: &Path, id: &str, edit: Edit) -> Option<Fix> {
    let shown = file.display();
    match edit {
        Edit::NotInFile => None,
        Edit::AlreadyExcluded => Some(Fix {
            file: file.to_path_buf(),
            new_content: None,
            summary: format!("leave {shown} as it is (it already excludes this keyboard)"),
            command: "sudo systemctl restart keyd".to_string(),
        }),
        Edit::Changed {
            content,
            added,
            removed,
            wildcard,
        } => {
            if content.len() > MAX_CONFIG_BYTES {
                return None;
            }
            let mut sed_scripts = Vec::new();
            if removed {
                sed_scripts.push(format!("-e '/^{id}[[:space:]]*$/d'"));
            }
            if let (true, Some(w)) = (added, &wildcard) {
                sed_scripts.push(format!("-e '/^{}[[:space:]]*$/a -{id}'", sed_pattern(w)));
            }
            let summary = match (added, removed) {
                (true, true) => {
                    format!("remove \"{id}\" and add \"-{id}\" under [ids] in {shown}")
                }
                (true, false) => format!("add \"-{id}\" under [ids] in {shown}"),
                _ => format!("remove \"{id}\" from the [ids] list in {shown}"),
            };
            Some(Fix {
                file: file.to_path_buf(),
                new_content: Some(content),
                summary,
                command: format!(
                    "sudo sed -i {} {shown} && sudo systemctl restart keyd",
                    sed_scripts.join(" ")
                ),
            })
        }
    }
}

/// Looks in `dir` for the keyd file that handles the keyboard and prepares how to free it.
pub fn find_fix(dir: &Path, vendor: u16, product: u16) -> Option<Fix> {
    let id = format!("{vendor:04x}:{product:04x}");
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "conf"))
        .collect();
    files.sort();
    files.into_iter().find_map(|file| {
        let text = std::fs::read_to_string(&file).ok()?;
        fix_from(&file, &id, edit_ids(&text, &id))
    })
}

/// Run as administrator: backup copy, new file (if needed), restart keyd.
const SCRIPT: &str = r#"set -e
f="$1"
if [ -n "$2" ]; then
  [ -e "$f.keynortia-backup" ] || cp -p "$f" "$f.keynortia-backup"
  printf '%s' "$2" > "$f"
fi
systemctl restart keyd 2>/dev/null || keyd reload"#;

/// Applies the change with `pkexec` (it asks for the administrator password).
pub fn apply(fix: &Fix) -> Result<(), String> {
    let output = Command::new("pkexec")
        .args(["/bin/sh", "-c", SCRIPT, "keynortia"])
        .arg(&fix.file)
        .arg(fix.new_content.as_deref().unwrap_or(""))
        .output()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                "pkexec not found: use \"Copy command\" and paste it into a terminal".to_string()
            } else {
                e.to_string()
            }
        })?;
    match output.status.code() {
        Some(0) => Ok(()),
        Some(126) => Err("authorization cancelled".to_string()),
        Some(127) => Err("authorization not granted".to_string()),
        _ => Err(format!(
            "failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "1a2c:9b09";

    fn changed(config: &str) -> String {
        match edit_ids(config, ID) {
            Edit::Changed { content, .. } => content,
            other => panic!("expected a change, found {other:?}"),
        }
    }

    #[test]
    fn adds_an_exclusion_after_the_wildcard() {
        let config = "[ids]\n*\n\n[main]\ncapslock = overload(control, esc)\n";
        assert_eq!(
            changed(config),
            "[ids]\n*\n-1a2c:9b09\n\n[main]\ncapslock = overload(control, esc)\n"
        );
    }

    #[test]
    fn keeps_other_exclusions_and_comments_in_place() {
        let config =
            "# mine\n[ids]\n*\n# not the keyboard of the mouse\n-046d:c52b\n\n[main]\na = b\n";
        assert_eq!(
            changed(config),
            "# mine\n[ids]\n*\n# not the keyboard of the mouse\n-046d:c52b\n-1a2c:9b09\n\n[main]\na = b\n"
        );
    }

    #[test]
    fn works_with_windows_line_endings_and_a_file_without_final_newline() {
        let crlf = changed("[ids]\r\n*\r\n[main]\r\na = b\r\n");
        assert!(crlf.contains("*\r\n-1a2c:9b09\n[main]"), "{crlf:?}");
        assert_eq!(changed("[ids]\n*"), "[ids]\n*\n-1a2c:9b09\n");
    }

    #[test]
    fn already_excluded_only_needs_a_restart() {
        let config = "[ids]\n*\n-1A2C:9B09\n[main]\n";
        assert_eq!(edit_ids(config, ID), Edit::AlreadyExcluded);
    }

    #[test]
    fn a_keyboard_listed_by_hand_is_taken_off_the_list() {
        let config = "[ids]\n0001:0001\n1a2c:9b09\n[main]\na = b\n";
        assert_eq!(changed(config), "[ids]\n0001:0001\n[main]\na = b\n");
    }

    #[test]
    fn a_file_for_other_keyboards_is_left_alone() {
        assert_eq!(
            edit_ids("[ids]\n0001:0001\n[main]\na = b\n", ID),
            Edit::NotInFile
        );
        // an asterisk outside [ids] does not count
        assert_eq!(edit_ids("[ids]\n[main]\na = *\n*\n", ID), Edit::NotInFile);
    }

    #[test]
    fn the_command_for_a_terminal_matches_the_edit() {
        let edit = edit_ids("[ids]\n*\n", ID);
        let fix = fix_from(Path::new("/etc/keyd/default.conf"), ID, edit).unwrap();
        assert_eq!(
            fix.command,
            "sudo sed -i -e '/^\\*[[:space:]]*$/a -1a2c:9b09' /etc/keyd/default.conf && sudo systemctl restart keyd"
        );
        assert_eq!(
            fix.summary,
            "add \"-1a2c:9b09\" under [ids] in /etc/keyd/default.conf"
        );
        let again = fix_from(
            Path::new("/etc/keyd/default.conf"),
            ID,
            edit_ids("[ids]\n*\n-1a2c:9b09\n", ID),
        )
        .unwrap();
        assert_eq!(again.new_content, None);
        assert_eq!(again.command, "sudo systemctl restart keyd");
    }

    #[test]
    fn finds_the_right_file_in_the_folder() {
        let dir = std::env::temp_dir().join(format!("keynortia-keyd-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a-others.conf"), "[ids]\n0001:0001\n[main]\n").unwrap();
        std::fs::write(dir.join("b-all.conf"), "[ids]\n*\n[main]\n").unwrap();
        std::fs::write(dir.join("c-note.txt"), "[ids]\n*\n").unwrap();
        let fix = find_fix(&dir, 0x1a2c, 0x9b09).expect("the file with the wildcard");
        assert_eq!(fix.file, dir.join("b-all.conf"));
        assert_eq!(
            fix.new_content.as_deref(),
            Some("[ids]\n*\n-1a2c:9b09\n[main]\n")
        );
        assert!(find_fix(&dir.join("does-not-exist"), 0x1a2c, 0x9b09).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
