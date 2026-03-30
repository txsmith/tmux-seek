use regex::Regex;
use serde::Deserialize;
use std::io::Write;
use std::process::{Command, Stdio};

#[derive(Debug, Deserialize)]
struct Pattern {
    name: String,
    regex: String,
}

#[derive(Debug, Deserialize)]
struct Config {
    patterns: Vec<Pattern>,
}

struct MatchedPattern {
    name: String,
    regex: String,
    count: usize,
    matching_lines: Vec<(usize, String)>,
}

struct SeekResult {
    fzf_lines: Vec<String>,
    matches: Vec<MatchedPattern>,
}

fn prepare_seek(config: &Config, capture: &str) -> SeekResult {
    let compiled: Vec<(&Pattern, Regex)> = config
        .patterns
        .iter()
        .filter_map(|p| {
            Regex::new(&p.regex)
                .map(|r| (p, r))
                .map_err(|e| eprintln!("seek: invalid regex for '{}': {}", p.name, e))
                .ok()
        })
        .collect();

    let mut matches: Vec<MatchedPattern> = compiled
        .iter()
        .map(|(p, _)| MatchedPattern {
            name: p.name.clone(),
            regex: p.regex.clone(),
            count: 0,
            matching_lines: vec![],
        })
        .collect();

    for (line_num, line) in capture.lines().enumerate() {
        for (i, (_, regex)) in compiled.iter().enumerate() {
            if regex.is_match(line) {
                matches[i].count += 1;
                matches[i].matching_lines.push((line_num, line.to_string()));
            }
        }
    }

    matches.retain(|m| m.count > 0);

    let max_name_len = matches.iter().map(|m| m.name.len()).max().unwrap_or(0);
    let fzf_lines: Vec<String> = matches
        .iter()
        .map(|m| {
            format!(
                "{:<width$}  ({})\t{}",
                m.name,
                m.count,
                m.regex,
                width = max_name_len,
            )
        })
        .collect();

    SeekResult { fzf_lines, matches }
}

#[cfg(test)]
fn resolve_seek(result: &SeekResult, selection: usize) -> Option<&str> {
    result.matches.get(selection).map(|m| m.regex.as_str())
}

fn load_config() -> Result<Config, Box<dyn std::error::Error>> {
    let path = std::env::var("TMUX_SEEK_PATTERNS")
        .map(std::path::PathBuf::from)
        .ok()
        .filter(|p| p.exists())
        .or_else(|| {
            let home = std::env::var("HOME").ok()?;
            let p = std::path::PathBuf::from(home)
                .join(".config")
                .join("tmux-seek")
                .join("patterns.yaml");
            p.exists().then_some(p)
        })
        .unwrap_or_else(|| {
            let exe = std::env::current_exe().expect("cannot resolve binary path");
            exe.parent().unwrap().parent().unwrap().join("patterns.yaml")
        });

    if !path.exists() {
        let plugin_dir = std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().and_then(|p| p.parent()).map(|p| p.display().to_string()))
            .unwrap_or_else(|| "<plugin dir>".to_string());
        eprintln!("seek: patterns file not found. Place it at one of:");
        eprintln!("  1. $TMUX_SEEK_PATTERNS (env var)");
        eprintln!("  2. ~/.config/tmux-seek/patterns.yaml");
        eprintln!("  3. {}/patterns.yaml (next to binary)", plugin_dir);
        std::process::exit(1);
    }
    let contents = std::fs::read_to_string(&path)?;
    let config: Config = serde_yaml::from_str(&contents)?;
    Ok(config)
}

#[cfg(test)]
fn load_config_from_str(yaml: &str) -> Result<Config, Box<dyn std::error::Error>> {
    let config: Config = serde_yaml::from_str(yaml)?;
    Ok(config)
}

fn capture_scrollback() -> Result<String, Box<dyn std::error::Error>> {
    let output = Command::new("tmux")
        .args(["capture-pane", "-p", "-J", "-S", "-"])
        .output()?;

    if !output.status.success() {
        return Err("tmux capture-pane failed — are you inside tmux?".into());
    }

    Ok(String::from_utf8(output.stdout)?)
}

fn run_fzf(fzf_input: &str) -> Result<String, Box<dyn std::error::Error>> {
    let mut child = Command::new("fzf")
        .args([
            "--delimiter=\t",
            "--with-nth=1",
            "--nth=1",
            "--no-preview",
            "--no-scrollbar",
            "--no-separator",
            "--padding=0",
            "--margin=0",
            "--no-multi",
            "--no-info",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(fzf_input.as_bytes())?;
    }

    let output = child.wait_with_output()?;

    if !output.status.success() {
        return Ok(String::new());
    }

    Ok(String::from_utf8(output.stdout)?)
}

fn parse_fzf_selection(output: &str) -> Option<&str> {
    let trimmed = output.trim();
    if trimmed.is_empty() {
        return None;
    }
    trimmed.split('\t').nth(1)
}

fn is_in_copy_mode() -> bool {
    let Ok(output) = Command::new("tmux")
        .args(["display-message", "-p", "#{pane_in_mode}"])
        .output()
    else {
        return false;
    };
    String::from_utf8_lossy(&output.stdout).trim() == "1"
}

fn exit_copy_mode() -> Result<(), Box<dyn std::error::Error>> {
    Command::new("tmux")
        .args(["send-keys", "-X", "cancel"])
        .status()?;
    Ok(())
}

fn enter_copy_mode_with_pattern(regex: &str) -> Result<(), Box<dyn std::error::Error>> {
    Command::new("tmux").args(["copy-mode"]).status()?;

    Command::new("tmux")
        .args(["send-keys", "-X", "search-backward", regex])
        .status()?;

    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var("TMUX").is_err() {
        eprintln!("seek: must be run inside a tmux session");
        std::process::exit(1);
    }

    let config = load_config()?;
    let capture = capture_scrollback()?;
    let result = prepare_seek(&config, &capture);

    if result.matches.is_empty() {
        eprintln!("seek: no patterns matched scrollback");
        std::process::exit(0);
    }

    let fzf_input = result.fzf_lines.join("\n");
    let selected = run_fzf(&fzf_input)?;

    if let Some(regex) = parse_fzf_selection(&selected) {
        if is_in_copy_mode() {
            exit_copy_mode()?;
        }
        enter_copy_mode_with_pattern(regex)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_CONFIG: &str = r#"
patterns:
  - name: URLs
    regex: 'https?://[^[:space:]]+'
  - name: "File:line"
    regex: '[^[:space:]]+:[0-9]+:[0-9]+'
  - name: Git hash
    regex: '[0-9a-f]{7,40}'
  - name: Rust error
    regex: 'error\[E[0-9]+\]'
  - name: IP address
    regex: '[0-9]{1,3}(\.[0-9]{1,3}){3}'
"#;

    const SCROLLBACK_COMPILER_SESSION: &str = "\
$ cargo build
   Compiling seek v0.1.0 (/home/user/projects/tmux-seek)
error[E0277]: the trait bound `Output: Default` is not satisfied
   --> src/main.rs:42:10
    |
42  |         .unwrap_or_default();
    |          ^^^^^^^^^^^^^^^^^ the trait `Default` is not implemented for `Output`
For more information about this error, try `rustc --explain E0277`.
$ git log --oneline
a]1b2c3d4 fix: resolve trait bound error
e5f6a7b8 feat: add scrollback capture
$ echo done";

    const SCROLLBACK_WEB_SESSION: &str = "\
$ curl https://api.example.com/v1/users
{\"id\": 1, \"name\": \"alice\"}
$ ssh 192.168.1.50
Last login: Mon Mar 29 10:00:00 2026
$ cat /var/log/app.log
2026-03-29 File \"/app/server.py\", line 42
https://docs.python.org/3/library/exceptions.html";

    const SCROLLBACK_EMPTY: &str = "\
$ echo hello
hello
$ ls
Cargo.toml  src";

    // --- prepare_seek: inspect what fzf would show ---

    #[test]
    fn compiler_session_shows_expected_patterns() {
        let config = load_config_from_str(TEST_CONFIG).unwrap();
        let result = prepare_seek(&config, SCROLLBACK_COMPILER_SESSION);

        let names: Vec<&str> = result.matches.iter().map(|m| m.name.as_str()).collect();
        assert!(names.contains(&"Rust error"), "should find rust error codes");
        assert!(names.contains(&"Git hash"), "should find git hashes");
        assert!(names.contains(&"File:line"), "should find file:line references");
    }

    #[test]
    fn compiler_session_counts_are_correct() {
        let config = load_config_from_str(TEST_CONFIG).unwrap();
        let result = prepare_seek(&config, SCROLLBACK_COMPILER_SESSION);

        let rust_err = result.matches.iter().find(|m| m.name == "Rust error").unwrap();
        assert_eq!(rust_err.count, 1, "error[E0277] appears on one line");
    }

    #[test]
    fn web_session_shows_urls_and_ips() {
        let config = load_config_from_str(TEST_CONFIG).unwrap();
        let result = prepare_seek(&config, SCROLLBACK_WEB_SESSION);

        let names: Vec<&str> = result.matches.iter().map(|m| m.name.as_str()).collect();
        assert!(names.contains(&"URLs"));
        assert!(names.contains(&"IP address"));
    }

    #[test]
    fn empty_scrollback_produces_no_matches() {
        let config = load_config_from_str(TEST_CONFIG).unwrap();
        let result = prepare_seek(&config, SCROLLBACK_EMPTY);

        assert!(result.matches.is_empty());
        assert!(result.fzf_lines.is_empty());
    }

    #[test]
    fn fzf_lines_contain_name_and_count() {
        let config = load_config_from_str(TEST_CONFIG).unwrap();
        let result = prepare_seek(&config, SCROLLBACK_WEB_SESSION);

        for (line, m) in result.fzf_lines.iter().zip(result.matches.iter()) {
            assert!(line.contains(&m.name), "fzf line should contain pattern name");
            assert!(line.contains(&format!("({})", m.count)), "fzf line should contain count");
        }
    }

    #[test]
    fn selecting_rust_error_returns_its_regex() {
        let config = load_config_from_str(TEST_CONFIG).unwrap();
        let result = prepare_seek(&config, SCROLLBACK_COMPILER_SESSION);

        let idx = result.matches.iter().position(|m| m.name == "Rust error").unwrap();
        let regex = resolve_seek(&result, idx).unwrap();
        assert_eq!(regex, r"error\[E[0-9]+\]");
    }

    #[test]
    fn selecting_url_returns_url_regex() {
        let config = load_config_from_str(TEST_CONFIG).unwrap();
        let result = prepare_seek(&config, SCROLLBACK_WEB_SESSION);

        let idx = result.matches.iter().position(|m| m.name == "URLs").unwrap();
        let regex = resolve_seek(&result, idx).unwrap();
        assert_eq!(regex, r"https?://[^[:space:]]+");
    }

    #[test]
    fn selecting_out_of_bounds_returns_none() {
        let config = load_config_from_str(TEST_CONFIG).unwrap();
        let result = prepare_seek(&config, SCROLLBACK_COMPILER_SESSION);

        assert!(resolve_seek(&result, 999).is_none());
    }

    // --- Config parsing ---

    #[test]
    fn config_parses_all_patterns() {
        let config = load_config_from_str(TEST_CONFIG).unwrap();
        assert_eq!(config.patterns.len(), 5);
        assert_eq!(config.patterns[0].name, "URLs");
    }

    #[test]
    fn config_rejects_invalid_yaml() {
        let result = load_config_from_str("not: [valid: yaml: ugh");
        assert!(result.is_err());
    }

    // --- parse_fzf_selection ---

    #[test]
    fn parse_fzf_selection_extracts_regex() {
        let line = "URLs           (12)\thttps?://[^[:space:]]+";
        assert_eq!(parse_fzf_selection(line), Some("https?://[^[:space:]]+"));
    }

    #[test]
    fn parse_fzf_selection_empty_is_none() {
        assert_eq!(parse_fzf_selection(""), None);
        assert_eq!(parse_fzf_selection("  \n  "), None);
    }
}
