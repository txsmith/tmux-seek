use regex::Regex;
use serde::Deserialize;
use std::io::Write;
use std::process::{Command, Stdio};

// --- Data Model ---

#[derive(Debug, Deserialize)]
struct Pattern {
    name: String,
    regex: String,
}

#[derive(Debug, Deserialize)]
struct Config {
    patterns: Vec<Pattern>,
}

struct PatternResult<'a> {
    pattern: &'a Pattern,
    count: usize,
    matching_lines: Vec<(usize, String)>,
}

// --- Config Loading ---

fn load_config() -> Result<Config, Box<dyn std::error::Error>> {
    let path = std::env::var("EXCAVATOR_PATTERNS")
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

    let contents = std::fs::read_to_string(&path)
        .map_err(|e| format!("seek: cannot read {}: {}", path.display(), e))?;
    let config: Config = serde_yaml::from_str(&contents)?;
    Ok(config)
}

// --- Scrollback Capture ---

fn get_history_limit() -> Result<usize, Box<dyn std::error::Error>> {
    let output = Command::new("tmux")
        .args(["show-options", "-gv", "history-limit"])
        .output()?;
    Ok(String::from_utf8(output.stdout)?.trim().parse()?)
}

fn capture_scrollback(limit: usize) -> Result<String, Box<dyn std::error::Error>> {
    let output = Command::new("tmux")
        .args([
            "capture-pane",
            "-p",
            "-J",
            "-S",
            &format!("-{}", limit),
        ])
        .output()?;

    if !output.status.success() {
        return Err("tmux capture-pane failed — are you inside tmux?".into());
    }

    Ok(String::from_utf8(output.stdout)?)
}

// --- Pattern Matching ---

fn match_patterns<'a>(capture: &str, patterns: &'a [Pattern]) -> Vec<PatternResult<'a>> {
    let compiled: Vec<(&Pattern, Regex)> = patterns
        .iter()
        .filter_map(|p| {
            Regex::new(&p.regex)
                .map(|r| (p, r))
                .map_err(|e| eprintln!("seek: invalid regex for '{}': {}", p.name, e))
                .ok()
        })
        .collect();

    let mut results: Vec<PatternResult> = compiled
        .iter()
        .map(|(p, _)| PatternResult {
            pattern: p,
            count: 0,
            matching_lines: vec![],
        })
        .collect();

    for (line_num, line) in capture.lines().enumerate() {
        for (i, (_, regex)) in compiled.iter().enumerate() {
            if regex.is_match(line) {
                results[i].count += 1;
                results[i].matching_lines.push((line_num, line.to_string()));
            }
        }
    }

    results.into_iter().filter(|r| r.count > 0).collect()
}

// --- fzf Integration ---

fn build_fzf_input(results: &[PatternResult]) -> String {
    let max_name_len = results.iter().map(|r| r.pattern.name.len()).max().unwrap_or(0);

    results
        .iter()
        .map(|r| {
            format!(
                "{:<width$}\t({})\t{}",
                r.pattern.name,
                r.count,
                r.pattern.regex,
                width = max_name_len
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn write_tempfile(capture: &str) -> Result<String, Box<dyn std::error::Error>> {
    let path = format!("/tmp/seek-capture-{}.txt", std::process::id());
    std::fs::write(&path, capture)?;
    Ok(path)
}

fn run_fzf(input: &str, tmpfile: &str) -> Result<String, Box<dyn std::error::Error>> {
    let preview_cmd = format!(
        "grep -n -E \"$(echo {{}} | cut -f3)\" {} | head -50",
        tmpfile
    );

    let mut child = Command::new("fzf")
        .args([
            "--delimiter=\t",
            "--with-nth=1,2",
            &format!("--preview={}", preview_cmd),
            "--preview-window=right:50%",
            "--header=tmux-seek: select a pattern",
            "--no-multi",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(input.as_bytes())?;
    }

    let output = child.wait_with_output()?;

    if !output.status.success() {
        return Ok(String::new()); // user cancelled
    }

    Ok(String::from_utf8(output.stdout)?)
}

fn parse_fzf_selection(output: &str) -> Option<&str> {
    let trimmed = output.trim();
    if trimmed.is_empty() {
        return None;
    }
    trimmed.split('\t').nth(2)
}

// --- copy-mode Handoff ---

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

// --- Main ---

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Fail fast if not in tmux
    if std::env::var("TMUX").is_err() {
        eprintln!("seek: must be run inside a tmux session");
        std::process::exit(1);
    }

    // 2. Load config
    let config = load_config()?;

    // 3. Capture scrollback
    let limit = get_history_limit()?;
    let capture = capture_scrollback(limit)?;

    // 4. Match patterns
    let results = match_patterns(&capture, &config.patterns);

    // 5. Handle no matches
    if results.is_empty() {
        eprintln!("seek: no patterns matched scrollback");
        std::process::exit(0);
    }

    // 6. Write capture to tempfile for preview
    let tmpfile = write_tempfile(&capture)?;

    // 7. Build fzf input and invoke
    let fzf_input = build_fzf_input(&results);
    let selected = run_fzf(&fzf_input, &tmpfile)?;

    // 8. Cleanup tempfile
    std::fs::remove_file(&tmpfile).ok();

    // 9. Parse selection and hand off
    if let Some(regex) = parse_fzf_selection(&selected) {
        if is_in_copy_mode() {
            exit_copy_mode()?;
        }
        enter_copy_mode_with_pattern(regex)?;
    }

    Ok(())
}
