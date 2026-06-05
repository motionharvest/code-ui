# Code UI

Run several coding agents in one terminal window — side by side, like split-screen TV.

---

## Get started (3 steps)

You need: **Linux**, **macOS**, or **WSL** (Windows Subsystem for Linux). A normal Mac or Linux laptop is fine.

### Step 1 — Install two helper tools

Code UI needs **tmux** and **git**. You only do this once.

**Ubuntu / Debian / WSL:**

```bash
sudo apt update
sudo apt install -y tmux git
```

**macOS** (install [Homebrew](https://brew.sh) first if you do not have it):

```bash
brew install tmux git
```

**Fedora:**

```bash
sudo dnf install -y tmux git
```

### Step 2 — Install Code UI

Copy this whole line, paste it into your terminal, press Enter:

```bash
curl -fsSL https://raw.githubusercontent.com/motionharvest/code-ui/main/install.sh | bash
```

Wait until it says **Installed**. That downloads Code UI and puts the `code-ui` command on your computer.

**Want a specific version?** (optional)

```bash
CODE_UI_VERSION=v0.1.1 curl -fsSL https://raw.githubusercontent.com/motionharvest/code-ui/main/install.sh | bash
```

### Step 3 — Open Code UI in your project

1. Open a terminal.
2. Go to the folder of the project you are working on (it should be a git repo):

```bash
cd /path/to/your/project
```

3. Start Code UI:

```bash
code-ui
```

That is it. You should see the split-pane UI.

---

## First shortcuts (learn these right away)

Do these **inside Code UI** (after `code-ui` is running).

### Split the screen in half

Click the pane you want to split so it is focused. Then hold **Ctrl+Alt** and press the **arrow key** for where the **new** pane should go:

| Arrow | New pane appears… |
|-------|-------------------|
| **←** | to the **left** |
| **→** | to the **right** |
| **↑** | **above** |
| **↓** | **below** |

A small menu opens — pick what runs there (Terminal, Cursor, Codex, etc.) and press Enter.

Think of it like pointing: *“put the new window that way.”*

### Quit Code UI

Press **Ctrl+Q** once (arms quit), then **Ctrl+Q** again to close.

### More shortcuts

- **Ctrl+Page Up** / **Ctrl+Page Down** — switch to another pane
- **Ctrl+W** — close the pane you are in (not the whole app)
- **Ctrl+Space** — full shortcut list inside the app

---

## If `code-ui` is not found

The installer puts the app in `~/.local/bin`. If your shell says `command not found`, run this once:

```bash
echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.bashrc
source ~/.bashrc
```

On **zsh** (default on newer macOS), use `~/.zshrc` instead of `~/.bashrc`.

Then try again:

```bash
cd /path/to/your/project
code-ui
```

---

## Optional: coding agents

You do **not** need these to open Code UI. Install only the agents you actually use:

| Agent | Install separately |
|-------|-------------------|
| Cursor CLI | `agent` |
| Codex | `codex` |
| OpenCode | `opencode` |
| Pi | `pi` |
| Plain shell | nothing extra |

You can always add a normal **Terminal** pane with no agent.

---

## What is Code UI?

- **Split panes** — run multiple agents (or shells) in one window.
- **Keeps running** — switch panes or close the UI; work continues in tmux behind the scenes.
- **Remembers your layout** — saved in `.codeui/` inside each project (you do not commit this folder).
- **Split and quit** — see [First shortcuts](#first-shortcuts-learn-these-right-away) above; **Ctrl+Space** for the full list in the app.

Works best in a modern terminal (Ghostty, WezTerm, Kitty, Alacritty, Windows Terminal, etc.) with a mouse.

---

## More help

| Problem | What to try |
|---------|-------------|
| Install script fails | Check internet; see [Releases](https://github.com/motionharvest/code-ui/releases) to download a zip manually |
| Blank or broken UI | Make your terminal window bigger; use a UTF-8 terminal |
| Agents missing | Install that agent’s CLI (table above); use a **Terminal** pane meanwhile |

**Manual download:** [github.com/motionharvest/code-ui/releases](https://github.com/motionharvest/code-ui/releases) — pick the file for your computer, unzip, move `code-ui` somewhere on your `PATH`.

---

## For developers

Building from source needs **Rust** and **Zig** (not needed for the install script above).

```bash
git clone git@github.com:motionharvest/code-ui.git
cd code-ui
cargo build --release
./target/release/code-ui
```

| Command | Output |
|---------|--------|
| `cargo build` | `target/debug/code-ui` |
| `cargo build --release` | `target/release/code-ui` |
| `cargo test` | run tests |
| `cargo install --path .` | install to `~/.cargo/bin` |
| `./reload.sh` | rebuild and restart during dev |

Publishing releases: [RELEASING.md](RELEASING.md).

## Configuration and persistence

- **Layout & panes** — `./.codeui/state` in the current working directory (workspaces, splits, pane titles, agent commands, resume hints, scroll offsets). `.codeui/` is added to `.gitignore` in repos when worktrees are created.
- **Theme** — `~/.config/code-ui/theme` (migrates automatically from `~/.config/split_tui/theme` if present).
- **Debug layout boxes** — set `CODE_UI_DEBUG_CONTAINERS=1` to show container debug overlays (also toggled with `D` in the shortcuts modal).

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
| Ctrl+Alt+↑ ↓ ← → | Split focused pane — arrow = where the new pane goes (opens agent picker) |
| Ctrl+Shift+A | Split right (quick, no arrow) |
| Ctrl+Shift+B | Split down (quick, no arrow) |
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
- Theme choice persists under `~/.config/code-ui/theme`.
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
