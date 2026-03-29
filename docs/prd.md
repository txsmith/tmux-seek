# tmux-seek — PRD & Implementation Strategy

## Problem

Finding content in tmux scrollback is friction-heavy. You know a compiler error, URL, or git hash appeared somewhere in your session — but copy-mode regex search requires you to remember or construct the right pattern, and visible-pane tools like tmux-fingers don't reach into history.

## Positioning

| | tmux-fingers / thumbs | tmux-seek |
|---|---|---|
| Scope | Visible pane only | Full scrollback |
| Selection | Hint keys | fzf pattern picker |
| Output | Direct action | Seeded copy-mode |
| Best for | "I see it, grab it" | "I know it's in there somewhere" |

Complementary tools, not competitors. Both can coexist in the same tmux config.

## Goals

- Fuzzy selection of a pattern class from those that actually match the current scrollback
- Hand off to tmux copy-mode with that pattern seeded — nothing more
- Patterns are user-extensible without touching code or keybindings
- Works cleanly on Linux x86 and macOS ARM as a compiled binary

## Non-goals

- Visible-pane hint rendering (that's fingers)
- Actions on match content (out of scope — see companion plugin tmux-dispatch)
- Termux / mobile support
- Anything beyond getting the user into copy-mode efficiently

---

## User Flow

1. User triggers seek via keybinding (`prefix + /` by default)
2. Scrollback is captured from the current pane
3. All configured patterns are tested against the capture
4. fzf opens showing only patterns with at least one match, with match counts
5. User fuzzy-searches and selects a pattern
6. tmux enters copy-mode with that pattern's regex seeded into `search-backward`
7. User navigates matches with native tmux `n` / `N`

### Edge Cases

- **No patterns match** — notify user and exit cleanly, do not open fzf
- **Already in copy-mode** — exit copy-mode first, re-enter after selection
- **fzf cancelled (Escape)** — no-op, return to normal state
- **Not inside tmux** — fail fast with a clear error message

---

## fzf Interface

```
URLs                  (12)
File:line errors       (3)
Git hashes             (1)
```

Preview pane shows the actual matching lines from the scrollback for the highlighted pattern. Surrounding context lines (2-3 above/below each match) are shown to disambiguate similar matches.

---

## Pattern Library

Defined in `patterns.yaml`, shipped with sensible defaults, fully user-replaceable.

```yaml
patterns:
  - name: URLs
    regex: 'https?://[^\s]+'

  - name: File:line
    regex: '[^\s]+:[0-9]+:[0-9]+'

  - name: Git hash
    regex: '[0-9a-f]{7,40}'

  - name: IP address
    regex: '[0-9]{1,3}(\.[0-9]{1,3}){3}'

  - name: File path
    regex: '(/[^\s]+)+'

  - name: Rust error
    regex: 'error\[E[0-9]+\]'

  - name: Python traceback
    regex: 'File "[^\s]+", line [0-9]+'

  - name: UUID
    regex: '[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}'
```

### Config Resolution Order

1. Path in `$EXCAVATOR_PATTERNS` env var
2. `~/.config/tmux-seek/patterns.yaml`
3. Plugin directory defaults (shipped with plugin)

---

## Ecosystem

- **tmux-open-nvim (`ton`)** — optional companion, opens file:line in a running neovim instance. Not integrated into seek directly; users wire it themselves after landing in copy-mode
- **tmux-dispatch** — planned companion plugin with the same pattern-based config approach, handles "execute an action on a yanked selection." Seek deliberately stops at copy-mode; dispatch picks up from there

---

## Repository Structure

```
tmux-seek/
├── seek.tmux          # tmux plugin entry point, keybinding wiring
├── patterns.yaml           # default pattern library, shipped with plugin
├── Cargo.toml
└── src/
    └── main.rs             # single binary, all logic
```

### External Dependencies

- `fzf` — required, must be in PATH
- `tmux` — obviously required, minimum version TBD (needs `send-keys -X` support, tmux >= 2.4)

---

---

# Implementation Strategy

## Language & Build

**Rust.** Chosen for:
- Single compiled binary, no runtime dependency on either target
- Excellent `regex` crate — compiled patterns, fast multi-pattern matching
- Strong `serde` + `serde_yaml` for config parsing
- Good cross-compilation support for both targets

### Cargo.toml Dependencies

```toml
[dependencies]
regex = "1"
serde = { version = "1", features = ["derive"] }
serde_yaml = "0.9"
```

### Cross-Compilation Targets

```
x86_64-unknown-linux-gnu    # homelab / server
aarch64-apple-darwin         # MacBook
```

Build natively on each machine rather than cross-compiling — simpler for a personal tool on two machines you control.

---

## Data Model

```rust
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
    matching_lines: Vec<(usize, String)>,  // (line_number, line_content)
}
```

`matching_lines` carries line numbers (0-indexed from capture) alongside content — needed to generate preview context (surrounding lines).

---

## Scrollback Capture

### Step 1: Query history-limit

```rust
fn get_history_limit() -> Result<usize> {
    let output = Command::new("tmux")
        .args(["show-options", "-gv", "history-limit"])
        .output()?;
    Ok(String::from_utf8(output.stdout)?.trim().parse()?)
}
```

### Step 2: Capture full scrollback

```rust
fn capture_scrollback(limit: usize) -> Result<String> {
    let output = Command::new("tmux")
        .args([
            "capture-pane",
            "-p",               // print to stdout
            "-J",               // join wrapped lines — critical for path/pattern integrity
            "-S", &format!("-{}", limit),
        ])
        .output()?;

    if !output.status.success() {
        return Err("tmux capture-pane failed — are you inside tmux?".into());
    }

    Ok(String::from_utf8(output.stdout)?)
}
```

**Why `-J` is essential:** Without it, long lines are split at terminal width into multiple lines in the capture output. This severs patterns like `src/main.rs:42:7` that may wrap. `-J` rejoins continuation lines into logical lines, making captures geometry-independent.

**Geometry note:** The capture is taken once and treated as an immutable snapshot for the lifetime of the invocation. Terminal resizing mid-interaction is not accounted for — this is acceptable because we never use line coordinates for positioning, only content for regex matching.

---

## Pattern Matching — Single Pass

All patterns are compiled upfront, then the capture is iterated once. This avoids spawning N grep processes (one per pattern) and is O(lines × patterns) rather than O(N × lines).

```rust
fn match_patterns<'a>(
    capture: &str,
    patterns: &'a [Pattern],
) -> Vec<PatternResult<'a>> {
    // Compile all regexes upfront — fail fast on invalid patterns
    let compiled: Vec<(&Pattern, Regex)> = patterns
        .iter()
        .filter_map(|p| {
            Regex::new(&p.regex)
                .map(|r| (p, r))
                .map_err(|e| eprintln!("Invalid regex for '{}': {}", p.name, e))
                .ok()
        })
        .collect();

    // Initialise result accumulators
    let mut results: Vec<PatternResult> = compiled
        .iter()
        .map(|(p, _)| PatternResult {
            pattern: p,
            count: 0,
            matching_lines: vec![],
        })
        .collect();

    // Single pass over capture
    for (line_num, line) in capture.lines().enumerate() {
        for (i, (_, regex)) in compiled.iter().enumerate() {
            if regex.is_match(line) {
                results[i].count += 1;
                results[i].matching_lines.push((line_num, line.to_string()));
            }
        }
    }

    // Filter to patterns with at least one match
    results.into_iter().filter(|r| r.count > 0).collect()
}
```

---

## fzf Invocation

### Input format

One line per matching pattern, piped to fzf stdin:

```
URLs\t12\thttps?://[^\s]+
File:line\t3\t[^\s]+:[0-9]+:[0-9]+
Git hash\t1\t[0-9a-f]{7,40}
```

Tab-separated: `name \t count \t regex`. The regex is passed as a hidden field used by the preview script — fzf's `--with-nth` controls what the user sees.

### Display

```
--with-nth=1,2          # show name and count only
--delimiter='\t'
--header='Select a pattern'
```

Formatted display in fzf:

```
URLs                  (12)
File:line errors       (3)
Git hashes             (1)
```

Count formatting (`(12)`) is done before piping to fzf — pad name to fixed width for alignment.

### Preview

The preview script receives the full selected line via `{}` and extracts the regex field to filter the capture:

```bash
--preview='echo {} | cut -f3 | xargs -I PATTERN grep -n "PATTERN" /tmp/seek-capture-$$.txt'
```

The capture is written to a tempfile before fzf is invoked. Preview shows matching lines with line numbers. Surrounding context (2-3 lines above/below) can be added with `grep -C 2`.

**Tempfile management:**
- Written to `/tmp/seek-capture-<pid>.txt` before fzf opens
- Deleted in a cleanup handler (catch signals, `atexit` equivalent via `Drop` trait or explicit cleanup before exit)

### fzf flags summary

```bash
fzf \
  --delimiter='\t' \
  --with-nth=1,2 \
  --preview='...' \
  --preview-window='right:50%' \
  --header='tmux-seek: select a pattern' \
  --no-multi
```

---

## Parsing fzf Output

fzf prints the selected line to stdout on exit. Parse the regex back out:

```rust
fn parse_fzf_selection(output: &str) -> Option<&str> {
    output.trim().split('\t').nth(2)  // third field is the regex
}
```

If fzf exits with a non-zero code (Escape, Ctrl-C), output is empty — treat as no-op.

---

## copy-mode Handoff

Two tmux commands, issued sequentially via separate `Command` calls — **not** via shell, to avoid any interpolation of the regex string:

```rust
fn enter_copy_mode_with_pattern(regex: &str) -> Result<()> {
    // Enter copy-mode
    Command::new("tmux")
        .args(["copy-mode"])
        .status()?;

    // Seed search — regex passed as a direct argv element, no shell involved
    Command::new("tmux")
        .args(["send-keys", "-X", "search-backward", regex])
        .status()?;

    Ok(())
}
```

**Why no shell:** The regex string is passed as a single element in the argv array. No shell quoting layer touches it. Characters meaningful to shells (`|`, `(`, `)`, `[`, `$`) pass through unmodified to tmux's regex engine, which interprets them as POSIX ERE.

**Already in copy-mode:** Check pane state before entering:

```rust
fn is_in_copy_mode() -> bool {
    let output = Command::new("tmux")
        .args(["display-message", "-p", "#{pane_in_mode}"])
        .output()
        .unwrap_or_default();
    String::from_utf8_lossy(&output.stdout).trim() == "1"
}
```

If already in copy-mode, send `send-keys -X cancel` first to exit, then re-enter cleanly.

---

## main() Flow

```rust
fn main() -> Result<()> {
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

    // 4. Match patterns — single pass
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
```

---

## tmux Plugin Wiring (`seek.tmux`)

```bash
#!/usr/bin/env bash

CURRENT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BINARY="$CURRENT_DIR/bin/seek"

# Default keybinding, user-overridable via tmux option
default_key="/"
key=$(tmux show-option -gv "@seek-key" 2>/dev/null || echo "$default_key")

tmux bind-key "$key" run-shell "$BINARY"
```

User can override with:
```
set -g @seek-key "s"
```

in their `tmux.conf`.

---

## Error Handling Philosophy

- Invalid regex in config → warn and skip that pattern, continue with rest
- `fzf` not in PATH → clear error message with install hint
- tmux command failures → propagate with context
- Empty scrollback → not an error, falls into "no patterns matched" path

---

## Open Questions for Implementer

1. **Preview context lines** — show matching lines only, or `grep -C 2` for surrounding context? Surrounding context is more useful but makes the preview busier. Recommend making it configurable via `@seek-preview-context` tmux option, defaulting to 2.

2. **Minimum tmux version** — `send-keys -X` was introduced in tmux 2.4. Consider adding a version check at startup.

3. **fzf `--bind` passthrough** — should users be able to configure additional fzf keybindings (e.g. `ctrl-/` to toggle preview) via tmux options? Low priority for v1 but worth keeping the door open by not hardcoding fzf flags.

4. **Pattern validation on load** — compile all regexes at startup and report invalid ones before doing any work, rather than silently skipping them mid-match.

