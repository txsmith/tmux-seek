# tmux-seek

Conveniently search your tmux scrollback with common patterns. Pick a pattern class (URLs, file paths, git hashes, etc.) via fzf, then land in copy-mode with that regex seeded — navigate matches with `n`/`N`.

<!-- TODO: screencast -->

## Install

### With TPM

```bash
set -g @plugin 'txsmith/tmux-seek'
```

Then `prefix + I` to install.

### Manual

```bash
git clone https://github.com/txsmith/tmux-seek.git ~/.tmux/plugins/tmux-seek
cd ~/.tmux/plugins/tmux-seek
cargo build --release
mkdir -p bin && ln -sf ../target/release/seek bin/seek
```

Add to `tmux.conf`:

```bash
run-shell ~/.tmux/plugins/tmux-seek/seek.tmux
```

### Requirements

- `fzf` in PATH
- tmux >= 2.4

## Usage

`prefix + /` opens fzf with all pattern types that matched your scrollback. Select one to enter copy-mode with that pattern's regex. Use `n`/`N` to jump between matches.

If no patterns match the current scrollback, seek exits with a message.

## Configuration

### Custom keybinding

```bash
set -g @seek-key "s"
```

### Custom patterns

Create `~/.config/tmux-seek/patterns.yaml`:

```yaml
patterns:
  - name: URLs
    regex: 'https?://[a-zA-Z0-9._~:/?#@!$&*+,;=%-]+'

  - name: Git hash
    regex: '[0-9a-f]{7,40}'

  - name: My custom pattern
    regex: 'TODO|FIXME|HACK'
```

Each pattern needs a `name` (shown in fzf) and a `regex`. Only patterns with at least one match in the current scrollback are shown.

Regexes must be POSIX ERE compatible — they're used by the Rust regex engine, `grep -E` (fzf preview), and tmux's `search-backward`. Use `[^[:space:]]` instead of `\s`, and `[Ee][Rr][Rr]` instead of `(?i)err`.

### Config resolution

1. `$TMUX_SEEK_PATTERNS` env var
2. `~/.config/tmux-seek/patterns.yaml`
3. `patterns.yaml` in the plugin directory

### Default patterns

URL/path, file:line, git hash, IP address, Rust error, Python traceback, UUID, quoted string, email, Java class, error keywords, hex number, base64, timestamp, JVM stack trace.
