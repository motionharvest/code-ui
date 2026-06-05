# Code UI

Code UI is a terminal-based split-pane harness for running multiple coding agents side by side. Each pane is backed by a persistent **tmux** session with a PTY, so agents keep running when you switch panes or workspaces. Layout, pane titles, agent commands, scroll position, and resume hints are saved per project under `.codeui/`.

The Rust crate is named `split_tui`; the built binary is `split_tui`.

## Requirements

| Requirement | Why |
|-------------|-----|
| **Rust** (2021 edition) | Build and run from source |
| **tmux** | Every agent pane attaches to a dedicated tmux session |
| **git** | Git status in pane chrome; git worktree picker and lifecycle actions |
| **Unix-like OS** (Linux, macOS, WSL) | PTY/tmux integration (`nix`, `libc`) |
| **UTF-8 terminal** with mouse support | TUI rendering, resizing, text selection |

**Optional agent CLIs** (install only what you use):

- `pi` — Pi agent
- `agent` — Cursor CLI agent
- `codex` — Codex
- `opencode` — OpenCode
- Your login shell — plain terminal panes

The built-in **Commander** pane is a harness UI (not a separate binary); it coordinates workspaces and agent panes from within the app.

## Installation

Clone the repository and build:

```bash
git clone <repository-url> code-ui
cd code-ui
cargo build --release
```

The release binary is at `target/release/split_tui`.

To install into `~/.cargo/bin`:

```bash
cargo install --path .
```

Run from your **project root** (the git repo you are working in) so layout and session state persist to `.codeui/state`:

```bash
cd /path/to/your/project
split_tui
```

Or during development:

```bash
cargo run
# or
./reload.sh
```

`reload.sh` stops any running `split_tui` process, rebuilds if needed, and relaunches via `cargo run`.

## Building

| Command | Output |
|---------|--------|
| `cargo build` | Debug binary: `target/debug/split_tui` |
| `cargo build --release` | Optimized binary: `target/release/split_tui` |
| `cargo test` | Run unit tests |

## Configuration and persistence

- **Layout & panes** — `./.codeui/state` in the current working directory (workspaces, splits, pane titles, agent commands, resume hints, scroll offsets). `.codeui/` is added to `.gitignore` in repos when worktrees are created.
- **Theme** — `~/.config/split_tui/theme` (theme name on disk).
- **Debug layout boxes** — set `SPLIT_TUI_DEBUG_CONTAINERS=1` to show container debug overlays (also toggled with `D` in the shortcuts modal).

Press **Ctrl+Space** in the app to open the shortcuts help overlay (same list as below).

## Keyboard shortcuts

### General

| Shortcut | Action |
|----------|--------|
| Ctrl+Space | Show / hide shortcuts help |
| T | Open theme selector |
| D | Toggle container debug boxes |
| Ctrl+Q (twice) | Quit (first press arms confirmation; still passes through to the focused pane) |

### Panes and layout

| Shortcut | Action |
|----------|--------|
| Ctrl+Alt+↑ ↓ ← → | Split focused pane (opens agent picker) |
| Ctrl+Shift+A | Split right |
| Ctrl+Shift+B | Split down |
| Ctrl+Page Up / Page Down | Cycle pane focus |
| Ctrl+W | Close focused pane |
| Ctrl+↑ ↓ ← → | Move focus between panes and workspace sidebar |
| Ctrl+Shift+↑ ↓ ← → | Resize pane edges |
| Ctrl+Shift+K / J | Resize top/bottom (when Up/Down are captured by the OS) |

### Scrolling and selection

| Shortcut | Action |
|----------|--------|
| Wheel / Page Up / Page Down | Scroll history (3 lines per wheel notch; at live bottom, wheel goes to the app when supported) |
| Shift+Page Up / Page Down | Scroll by page |
| Shift+Home / End | Scroll to top / bottom |
| Esc | Return to live output when scrolled up |
| Shift+← / → | Adjust text selection by character |
| Ctrl+C | Copy selection; if nothing selected, send interrupt to the pane |

Pane titles show `↑N` while you are viewing history above the live bottom.

### Mouse

| Action | Effect |
|--------|--------|
| Drag pane dividers | Resize splits |
| Drag pane title onto another pane | Swap pane positions |
| Drag in pane contents | Select text |
| Ctrl+Shift+M | Toggle mouse capture (pass events to the terminal vs. the harness) |

## Features

### Split panes and agents

- Split the layout horizontally or vertically and choose an agent preset: Terminal, Commander, Pi, Cursor (`agent`), Codex, or OpenCode.
- One Commander pane per workspace is supported; additional agent types can be limited per workspace rules.
- Pane title bar shows the pane name and agent type; controls include refresh, maximize, and close.
- Click a lone pane’s title to open the new-pane picker (name + agent). Drag a title onto another pane to swap positions.
- Maximize a pane to fill the workspace; restore with the same control.

### Workspaces

- Sidebar lists workspaces; click a tab to switch, **+** to add, menu icon for rename/delete/settings.
- Each workspace has its own split layout and focus state.
- Ctrl+arrow navigation can move focus into the sidebar.

### Commander

- Dedicated Commander pane for orchestration: discuss tasks, approve plans, and drive workspace/pane changes from the harness.
- Handoff protocol between Commander and worker panes (`COMMANDER_HANDOFF` blocks in output).

### Git integration

- Pane chrome shows repo folder, branch, and path context when the tmux session cwd is inside a git repo.
- Click the git subtitle badge to open the **worktree picker**: list worktrees, create branches/worktrees, run agents on tasks, review commits, merge, delete, and run pre-merge checks.
- Worktree rows show lifecycle status (clean, dirty, ahead, merge-ready, etc.) and suggested next actions.

### Sessions and resume

- Agent panes run inside named tmux sessions so output survives focus changes.
- On exit, supported agents can leave a **resume** command in output; Code UI captures and persists it so the pane can restart with `codex resume …` (and similar) on next launch.
- On quit, panes get a short shutdown grace period so agents can flush session hints before PTYs are torn down.

### Themes

- Multiple built-in themes (including passthrough modes that leave terminal colors unchanged).
- Theme choice persists under `~/.config/split_tui/theme`.
- Open the theme modal with **T**; preview with arrow keys and digits, confirm with Enter.

### Text selection and copy

- In-app text selection with mouse drag and Shift+arrow adjustment.
- Ctrl+C copies the selection to the system clipboard when a selection is active.

### Other

- Bracketed paste and enhanced keyboard handling for reliable shortcuts in modern terminals.
- Optional TTS/state hooks for Commander feedback (internal harness features).

## Project layout

```
src/
  main.rs          — entry, terminal setup, render loop
  app.rs           — input, workspaces, modals, Commander
  pane.rs          — PTY/tmux panes and scrollback
  layout.rs        — split tree, persistence
  ui.rs            — chrome, modals, agent presets
  git_*.rs         — git status and worktrees
  worktree_*.rs    — worktree picker UI and lifecycle
  theme.rs         — themes and persistence
reload.sh          — dev rebuild + restart script
```

## License

See repository license file if present; otherwise check with the project maintainers.
