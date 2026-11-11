# Spectacli implementation plan

Spectacli (spectacle + CLI) is one lightweight window that runs the AI coding CLIs you already have installed, in embedded terminals, side by side.

This plan is written for one developer working with an AI coding assistant. Work through it phase by phase. Every task is small enough to be one commit.

---

## 0. Direct feedback on the brief (read this first)

These are the places where I think the brief is wrong or can be simpler. Each one changes the plan below.

1. **The RAM claim needs a precise definition, or it will be easy to attack.** Most of the memory goes to the CLIs themselves: a Claude Code or Codex process tree, with its MCP servers, often uses 150 to 400 MB. Spectacli cannot shrink that. Comparing "CLIs in Spectacli" with "desktop apps" is also not a like-for-like comparison, because the desktop apps have different features. The claim you *can* own and prove is this: **Spectacli overhead = total RAM minus the CLI process trees**, compared with Windows Terminal (the floor), Electron hubs, and the desktop apps. Section 4 sets up the benchmark that way.
2. **Suspending idle agents saves CPU, not RAM, and idle CLIs already use about 0% CPU.** A suspended process keeps all of its memory. Freezing it can also break things: HTTP keep-alives drop, MCP servers time out, and streaming responses die. Replace it with **hibernate**: stop an idle agent's process tree and resume its session later with the CLI's own resume flag. That frees the RAM for real, and it is the better differentiator. Keep true suspend (`NtSuspendProcess`) as an experimental toggle at most, or drop it.
3. **The prototype adds a full PowerShell process to every agent.** `powershell.exe -NoExit -Command claude` costs about 60 to 80 MB per pane. Spawn the CLI directly. This one change saves more RAM than any other feature on the list. On your machine, `claude.cmd` is only a shim around `node_modules\@anthropic-ai\claude-code\bin\claude.exe`, so Spectacli can resolve the shim and launch the `.exe` itself.
4. **Switching projects must not restart agents.** The prototype kills every pane and relaunches it in the new folder, which destroys the running sessions. Instead, give every project its own agents. Switching projects swaps which panes are visible. The other project's agents keep running in the background, or hibernate if you choose that.
5. **There are two search boxes.** The top bar has one and the left column has another. Keep a single command palette (Ctrl+K), opened from the top bar, and drop the sidebar Search button. Alternatively, make the sidebar button open the same palette.
6. **Three layouts are one too many for the MVP.** "Tabs" is a focus layout without the strip. Ship the auto grid plus per-pane maximize in the MVP, add focus mode in phase 2, and add tabs only if you miss them.
7. **Keep "conversations" out of the MVP.** In this plan a conversation is a saved group of agent sessions inside a project, and it needs the session-restore work to mean anything. The MVP has projects and agents. Conversations arrive in phase 3.
8. **Use CLI hooks for status where they exist, and parse terminal output only as a fallback.** Claude Code accepts `--settings <file-or-json>` on launch, and Codex accepts `-c key=value`. Spectacli can inject a notification hook *per launch* without editing the user's config files. That is far more accurate than guessing from output (see feature 4).
9. **Pick one spelling of the name.** The README says "SpectaCLI" and the brief says "Spectacli". The product name, the bundle identifier, and the config folder name should all agree.
10. **`github.md` is not loaded automatically by any assistant.** Claude Code automatically reads `CLAUDE.md` and `CLAUDE.local.md`. Create a `CLAUDE.local.md` containing the single line `@github.md`, and add `CLAUDE.local.md` to `.gitignore`. That way the rules apply in every session without being committed.
11. **Delete `agent-hub/` once you have ported what is listed in section 3.1.** Do not commit it. It is untracked right now, so it should either stay outside the repo or be removed.

Everything else in the brief holds up: Tauri v2, Rust plus portable-pty, xterm.js, the data-driven registry, and the no-sprawl rule.

---

## 1. Stack decision

| Choice | Verdict | Why |
|---|---|---|
| Tauri v2 + Rust | **Keep** | WebView2 is shared with the OS, so you avoid Electron's roughly 150 MB Chromium baseline. Rust gives native access to job objects, ConPTY, and process stats. |
| portable-pty | **Keep** | It wraps ConPTY on Windows and is battle-tested in WezTerm. If ConPTY bugs show up, bundle Microsoft's newer `conpty.dll` + `OpenConsole.exe` (MIT, from the `Microsoft.Windows.Console.ConPTY` package) next to the exe. Verify that portable-pty loads a side-by-side `conpty.dll`. If it does not, that is a small patch. |
| sysinfo crate | **Keep, with care** | Refresh only the PIDs Spectacli tracks, never the whole process table. On Windows, pair it with **Job Objects** (see feature 3). |
| React + TS + Vite | **Keep** | AI assistants know React best, and React's runtime cost is small next to WebView2 and xterm. Do not add a component library. Use plain CSS variables and Zustand for state. Preact or Solid would save a few MB, which is not worth the friction. |
| xterm.js | **Keep (v5 or v6)** | Use the WebGL addon on visible panes and fall back to the DOM renderer. Browsers cap WebGL contexts at about 16, so dispose WebGL on hidden panes. |
| Tauri plugins | `dialog`, `notification`, `window-state`, `single-instance`, `updater`, `clipboard-manager` (image read), `opener` | Use these instead of hand-written equivalents. |
| SQLite | **Skip** | The data is small. Versioned JSON files are simpler and easier to diff and back up (section 2). |

Alternatives considered: a fully native UI (egui, or a WezTerm fork) would be lighter but would take months more work. Electron is what you are trying to beat.

---

## 2. Architecture

### 2.1 Modules

```
┌──────────────────────────── WebView2 (React) ───────────────────────────────┐
│ TitleBar  Sidebar(Projects, Agents)  PaneGrid(TermPane×N)  BroadcastBar     │
│ stores: projectStore, agentStore, layoutStore, statsStore, inboxStore       │
│ lib/ipc.ts  (typed wrappers around invoke() + Channel + listen())           │
└──────────────▲───────────────────────────────┬──────────────────────────────┘
   Channel<PtyChunk> per agent (binary, batched)│ invoke(): commands
   events: stats, status, inbox, agent-exit     ▼
┌──────────────────────────── Rust core (src-tauri) ──────────────────────────┐
│ commands.rs   thin #[tauri::command] layer, validates input                 │
│ registry/     built-in agents.toml + user overrides, detection, shim resolve│
│ pty/          PtyManager: spawn, write, resize, kill, output batching,      │
│               scrollback ring buffer (for re-attach), Job Object per agent  │
│ stats/        1–2 s sampler over job PIDs → RAM/CPU per agent + total       │
│ status/       hook receiver (named pipe) + output heuristics → state machine│
│ store/        versioned JSON, atomic writes, migrations                     │
│ session/      resume templates, hibernate/restore                           │
│ worktree/     git worktree add/remove (shells out to git)                   │
│ paste/        clipboard image → <project>/.spectacli/pastes/*.png           │
└──────────────┬──────────────────────────────────────────────────────────────┘
               │ ConPTY (portable-pty)
               ▼
   claude.exe   codex.exe   gemini (node)   …   each in its own Job Object
```

### 2.2 Communication

- **Commands (frontend → Rust, `invoke`)**: `detect_agents`, `list_projects`, `add_project`, `remove_project`, `set_active_project`, `spawn_agent {projectId, agentDefId, profileId?, cols, rows} → {agentId}`, `attach_agent {agentId, channel} → {scrollback}`, `write {agentId, data}`, `resize {agentId, cols, rows}`, `kill_agent`, `hibernate_agent`, `resume_agent`, `broadcast {text, agentIds}`, `paste_image {agentId} → {path}`, `save_layout`, `get_settings`, `set_settings`, `create_worktree`, `remove_worktree`.
- **PTY output (Rust → frontend)**: use a `tauri::ipc::Channel` per attached pane rather than global events, because channels are ordered and faster. Rust **coalesces output every 8 to 16 ms** into one message per pane. This is the main defence against IPC overhead when many panes stream at once.
- **Events (Rust → frontend, `emit`)**: `stats:tick` (all agents in one payload every 1.5 s), `agent:status`, `agent:exit`, `inbox:new`, `registry:changed`.
- **Rust owns the processes and the truth.** The webview is a view. If you reload the webview (F5 in dev), it calls `attach_agent` for each live agent, receives the scrollback ring buffer, and carries on. That also makes hidden panes cheap: see feature 3.

### 2.3 Why Job Objects

Create one Job Object per agent, with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`:
- Closing a pane kills the whole tree, including MCP servers and child node processes. No orphans are left behind.
- If Spectacli crashes, Windows closes the job handles and kills every agent. There are no zombie CLIs to hunt for.
- `JobObjectBasicProcessIdList` gives you every PID in the tree for RAM sums, without scanning the whole system.
- `JobObjectBasicAccountingInformation` gives you total CPU time for the tree.

Assign the job right after spawn. Children that start in the first few milliseconds could escape, but CLIs take hundreds of milliseconds before they spawn anything, so this is fine in practice. The proper fix is to spawn suspended, which portable-pty does not expose, so treat it as a later refinement.

---

## 3. Features

### 3.1 What to reuse from the Electron prototype

| Reuse | Rewrite |
|---|---|
| The `KNOWN` list becomes the seed of `agents.toml` | All of `main.js` moves to Rust (`pty/`, `registry/`, `store/`) |
| The PATH × PATHEXT detection logic, ported to Rust | PowerShell wrapping: drop it and spawn directly |
| The CSS variables and colours from `index.html` | The DOM-string rendering in `renderer.js` becomes React components |
| The xterm options and fit-on-resize approach | "Switch project = restart all": replace with per-project agents |
| The picker plus custom-command UX | The `cmd` dedupe in `addAgent`: allow multiple instances and profiles |

Keep the prototype running until phase 1 is done. It is also your **Electron baseline** for the benchmark.

### 3.2 Feature 1: detection and the data-driven registry

`src-tauri/resources/agents.toml` is embedded with `include_str!`. User overrides live in `<config>/agents.user.toml` and are merged by `id`.

```toml
[[agent]]
id = "claude"
name = "Claude Code"
commands = ["claude"]                 # names to look up on PATH
version_args = ["--version"]
new_session_args = ["--session-id", "{session_uuid}"]
resume_args = ["--resume", "{session_uuid}"]
continue_args = ["--continue"]        # fallback: most recent in this cwd
hook = { kind = "claude-settings" }   # see feature 4
config_dir_env = "CLAUDE_CONFIG_DIR"  # for profiles; verify per version
image_paste = "path"                  # path | native
submit = "\r"

[[agent]]
id = "codex"
name = "Codex CLI"
commands = ["codex"]
resume_args = ["resume", "--last"]    # Codex picks its own ids; --last per cwd
hook = { kind = "codex-notify" }
config_dir_env = "CODEX_HOME"
```

Detection rules:
- Walk PATH × PATHEXT, preferring `.exe` over `.cmd`/`.bat`/`.ps1`.
- **Resolve npm shims.** If a `.cmd` ends by calling a `.exe` (as `claude.cmd` does today) or `node <script>.js`, launch that directly. If the shim cannot be parsed, fall back to `cmd.exe /d /s /c <shim>`. Known quirk: Ctrl+C under `cmd /c` can trigger "Terminate batch job (Y/N)?". That is why direct launch matters.
- **Reject impostors.** On your machine, `copilot` resolves to VS Code's `copilot-shim`, which is not the GitHub Copilot CLI. Probe `version_args` with a 3-second timeout and match a `version_regex` from the registry. Cache the result by path + mtime so startup stays instant.
- Run detection on startup in a background thread and when the picker opens. The UI never waits on it.
- Adding a CLI means adding one TOML entry. Custom commands are user entries with only `commands`.

### 3.3 Feature 2: projects, conversations, and session restore

- **Project** = folder + its agents + its layout. Selecting it sets the cwd for new agents and swaps the visible panes.
- **Conversation** (phase 3) = a named snapshot of a project's agent set: which agents, which profile, which session id, and the layout. "New" creates an empty conversation and lets you pick agents. Conversations are listed under their project in the sidebar.
- **Session restore**, from best to weakest:
  1. **Assign the id up front** (Claude): Spectacli generates a UUID, launches with `--session-id <uuid>`, and later resumes with `--resume <uuid>`. It never needs to read the CLI's files. Confirmed in your installed Claude Code 2.1.312.
  2. **Resume the latest in this cwd** (Codex `resume --last`, Claude `--continue`): this is reliable only when one agent of that CLI runs per folder. Per-agent worktrees (feature 7) make it reliable.
  3. **No resume support**: relaunch fresh and show "new session" in the pane header.
- Store resume templates in the registry so that a CLI update means a TOML change, not a code change.
- On app start: restore the last project and its layout, then relaunch its agents with their resume args. Other projects stay **hibernated** until opened, which keeps startup memory low.

### 3.4 Feature 3: per-agent RAM/CPU, hidden panes, hibernate

- **Sampler**: every 1.5 s, for each job, list PIDs, then read the memory figure with `GetProcessMemoryInfo`. Show **Private Bytes** as "RAM" (it is the honest number), with working set available in a tooltip. CPU% = Δ job CPU time ÷ Δ wall time ÷ cores. Emit one `stats:tick` for all agents.
- **Total** in the top bar = Σ agents + **Spectacli itself**: its own process plus the WebView2 processes under it. Show it as a split, for example "Agents 1.2 GB · Spectacli 74 MB". That split shows off the differentiator.
- **Hidden panes**: Rust keeps reading the PTY, which the CLIs need so they never block on a full pipe, and appends to a per-agent ring buffer (about 2 MB). The frontend **unmounts** xterm for panes that are not visible: other projects, minimized panes, or the whole window when it is minimized. When a pane shows again, it re-attaches and replays the buffer. That costs nothing while hidden and gives a short replay on show. The simpler path is to keep the xterm instance alive but drop WebGL, and you can start there. Measure both.
- **Hibernate (replaces idle suspend)**: after N minutes idle (default off, configurable per agent), or manually, Spectacli sends the CLI's graceful exit, waits, closes the job, and keeps the session id. The pane shows "Hibernated · click to resume", and resuming relaunches with the resume args. Only offer it for agents whose registry entry has `resume_args`.
- **Experimental suspend**: if you keep it, use `NtSuspendProcess` on every PID in the job, show it only for agents that are idle, and label it "may drop network connections".

### 3.5 Feature 4: attention inbox and notifications

The status state machine per agent is `starting → working → idle | needs_input → exited`.

Signal sources, best first:
1. **Injected hooks (accurate).** Rust opens a named pipe `\\.\pipe\spectacli-<random>`. At launch it passes:
   - Claude: `--settings '<json>'` with `Notification` and `Stop` hooks that run a tiny `spectacli-notify.exe <agentId> <event>` helper, which writes to the pipe. This uses no edits to `~/.claude/settings.json`, and the user's own settings still apply. The wording in your installed help says settings passed with `--settings` are layered; verify on your version that they merge rather than replace.
   - Codex: `-c notify=["spectacli-notify.exe","<agentId>"]`.
   - Other CLIs: add the field to the registry when they gain hook support.
2. **Terminal signals (fairly reliable)**: BEL (`\x07`), OSC 9 / OSC 777 notifications, OSC 9;4 progress, and title changes (OSC 0/2), which several CLIs use for spinners.
3. **Output heuristics (weak)**: per-CLI regexes in the registry for prompts such as "Do you want to…", "(y/n)", or "Allow?", plus quiescence (no output for 3 s after a burst means probably idle).

State the limits honestly in the UI and README. Heuristics break when a CLI changes its wording, misfire on output that merely contains the trigger text, and cannot tell "thinking silently" from "done". Hooks fix this for the CLIs that support them, and the pane header shows which source the status came from.

Inbox: the bell lists `needs_input` and `finished` events (agent, project, time) and clicking one focuses that pane. Send an OS toast only when the window is not focused or the pane is not visible. Debounce to at most one toast per agent per 10 s.

### 3.6 Feature 5: image paste

- On Ctrl+V in a pane, or a paste button, `paste_image` reads the clipboard image with the clipboard-manager plugin, writes `<project>/.spectacli/pastes/2026-11-12_143005_ab12.png`, and types the **quoted path** into the pane.
- On first use in a project, add `.spectacli/` to **`.git/info/exclude`** (local, never committed) so pastes do not end up in the user's repo. Do not edit their `.gitignore`.
- Registry `image_paste = "native"` means "let the CLI handle Ctrl+V itself". Claude Code can read the clipboard directly, so default to that for Claude and keep the path method as a menu option.
- The broadcast bar accepts pasted images the same way and sends the path to every selected agent.
- Clean up: delete pastes older than 30 days on startup (a setting).

### 3.7 Feature 6: profiles (two accounts of one CLI)

- A profile = `{id, agentDefId, label, args[], env{}, cwdOverride?}`, for example "Claude (work)" with `CLAUDE_CONFIG_DIR=C:\Users\me\.claude-work`.
- Spectacli only **sets** environment variables and args. It never opens files in those directories. Log in once inside the pane, and the CLI writes its own credentials.
- Do not allow profile env values to be read back into logs or telemetry. Mask values whose keys look secret (`*KEY*`, `*TOKEN*`, `*SECRET*`) in the UI, and warn that API keys belong in the CLI's own config, not here.
- Verify each CLI's config-dir variable on your installed versions before you document it.

### 3.8 Feature 7: git worktree per agent (optional)

- The toggle "Isolate in worktree" on spawn runs `git worktree add <project>/.spectacli/worktrees/<agent-slug> -b spectacli/<agent-slug>` and launches the agent there.
- The pane header shows the branch. A pane menu offers "Open folder", "Remove worktree" (refuses if dirty unless forced), and "Copy merge command". Spectacli does not merge for you. That is the user's call, and it keeps the app small.
- Worktrees make "resume latest in cwd" reliable, because each agent has its own cwd.

### 3.9 Feature 8: Windows reliability

- **ConPTY**: needs Windows 10 1809 or later. Known issues include resize reflow glitches and lost output when killed. Mitigations: debounce resize (100 ms) and only send it when cols/rows actually change. Optionally bundle the newer `conpty.dll`.
- **Encoding**: decode UTF-8 in Rust with a streaming decoder that keeps partial multibyte sequences across reads. Send bytes (`Uint8Array`) to xterm, not strings.
- **Shells**: the custom-command option runs `pwsh` if present, otherwise `powershell`. For **WSL**, use a registry entry of `wsl.exe -d <distro> --cd <wslpath> -- <cmd>`, converting `C:\x` to `/mnt/c/x`. Detection of CLIs *inside* WSL is phase 5 (`wsl -e sh -lc 'command -v claude'`).
- **Crash recovery**: Job Objects kill children if the app dies. On next start, the restore flow resumes sessions. Write state atomically. Install a Rust panic hook that logs to `<data>/logs/` with daily rotation, keeping 5 files.
- **Single instance**: a second launch focuses the existing window (single-instance plugin).
- **High DPI and multiple monitors**: the window-state plugin, plus re-fitting xterm on `devicePixelRatio` change.

---

## 4. Data model and storage

Location: Tauri's `app_config_dir()`, for example `%APPDATA%\dev.spectacli.app\`. The identifier is permanent once you ship, so choose it now.

```
%APPDATA%\dev.spectacli.app\
  settings.json          app settings, theme, hibernate defaults
  projects.json          projects, conversations, agent instances, layouts
  profiles.json          per-agent profiles (env + args, never credentials)
  agents.user.toml       registry overrides and custom commands
  detect-cache.json      path + mtime → version (safe to delete)
  logs\
<project>\.spectacli\    pastes\, worktrees\   (excluded via .git/info/exclude)
```

```ts
// Every file: { "schemaVersion": 1, ...data }
Project       { id, name, path, createdAt, lastOpenedAt, conversations: Conversation[], activeConversationId }
Conversation  { id, title, createdAt, agents: AgentInstance[], layout: Layout }
AgentInstance { id, agentDefId, profileId?, sessionId?, cwd, worktree?: {path, branch},
                hibernated: boolean, autoHibernateMin?: number }
Layout        { mode: "grid" | "focus", focusedAgentId?, maximizedAgentId?, order: string[] }
Profile       { id, agentDefId, label, args: string[], env: Record<string,string> }
Settings      { theme, fontSize, scrollback, hibernateAfterMin?, toastWhen: "unfocused"|"always"|"never", ... }
```

Forward compatibility:
- Store `schemaVersion` in every file. Rust holds a list of `migrate_vN_to_vN+1` functions run in order on load. Before migrating, copy the file to `*.v{N}.bak`.
- Add `#[serde(default)]` to every new field, and keep `#[serde(flatten)] extra: Map<String, Value>` on each struct, so that **unknown fields from a newer version survive** a round trip through an older one. That way a downgrade does not erase data.
- Use string IDs (UUIDv7) and never array indexes.
- Write with **atomic writes** (write `*.tmp`, `fsync`, rename). Debounce saves by 500 ms.
- If parsing fails, rename the file to `*.corrupt-<ts>`, start from the latest `.bak`, and show a toast.

---

## 5. Roadmap

Estimates assume a few focused hours a day with an AI assistant. They are calendar days, padded for Windows surprises.

### Phase 0: scaffold (1 day)
- `npm create tauri-app` (React + TS + Vite) at the repo root. Set the identifier, product name, and frameless window (`decorations: false`).
- Add the `.gitignore` entries (already mostly done), `CLAUDE.local.md` → `@github.md`, the README skeleton with the not-affiliated notice, and a CI workflow (`cargo clippy`, `cargo test`, `tsc`, `vitest`).
- **Done when**: `npm run tauri dev` opens an empty frameless window, CI is green, and `git check-ignore github.md` prints the file name.

### Phase 1: MVP (8 to 12 days)
1. **(done)** Custom title bar: `data-tauri-drag-region`, min/max/close via the window API, double-click to maximize, and Windows 11 snap layouts on hover over maximize (verify; may need the `tauri-plugin-decorum` approach or a native hit-test).
2. **(done)** Registry + detection + shim resolution + impostor probe (Rust, with unit tests for PATH and shim parsing).
3. **(done)** `PtyManager`: spawn direct, Job Object, batched Channel output, write, resize, kill, ring buffer.
4. `TermPane` component: xterm + fit + WebGL with DOM fallback, header (name, status dot placeholder, maximize).
5. Auto grid: 1 = full, 2 = 1×2, 3 to 4 = 2×2, 5 to 6 = 3×2, 7 and up = 3×3 with a scroll; maximize/restore per pane.
6. Sidebar: projects (add folder, select, remove), active agents list, the + picker with detected CLIs and a custom command.
7. Per-project agents: switching hides and shows panes instead of restarting them. Persist `projects.json`.
8. Restore on restart: the last project's agents relaunch, without resume yet.
- **Done when**: you can run Claude, Codex, and a custom command in two projects at once; switching projects keeps sessions alive; closing a pane leaves no orphan processes (check in Task Manager); the window drags, snaps, and resizes cleanly; and the installer is under 10 MB.

### Phase 2: resources and status (5 to 7 days)
- Stats sampler, RAM/CPU in pane headers and sidebar rows, the top-bar total with the Agents/Spectacli split.
- Hidden-pane unmount and replay. Focus layout.
- Status state machine, the notify helper exe, the named pipe, Claude `--settings` hooks, Codex `notify`, the OSC/BEL parser, regex fallback, the inbox, and toasts.
- **Done when**: RAM readouts match Process Explorer's private bytes within 5%; Claude and Codex produce "needs input" and "finished" within 1 s via hooks; a hidden pane costs no measurable CPU while its agent streams.

### Phase 3: sessions and conversations (4 to 6 days)
- Session ids (`--session-id` for Claude), resume templates, conversations in the sidebar, "New".
- Hibernate (manual plus auto timer) and resume.
- Store migrations, backups, corrupt-file recovery.
- **Done when**: quitting and restarting resumes Claude sessions exactly; Codex resumes via `--last` when isolated; hibernating 4 agents frees their RAM, as shown in the top bar.

### Phase 4: input features (3 to 5 days)
- Broadcast bar: pick target agents (default: all running in this project) and send using bracketed paste followed by the registry `submit` key.
- Image paste to pane and to broadcast, `.git/info/exclude`, cleanup.
- Profiles UI.
- **Done when**: one prompt reaches 3 different CLIs and each submits it; a pasted screenshot arrives as a path that Claude can open; two Claude profiles run side by side and stay logged in as different accounts.

### Phase 5: worktrees and Windows polish (4 to 6 days)
- Worktree toggle, branch in the header, removal.
- WSL entries, pwsh detection, DPI handling, the panic logger, optional bundled `conpty.dll`.
- **Done when**: 2 agents edit the same repo in separate worktrees without conflicts; a forced crash (kill the Spectacli process) leaves no orphan CLIs, and the next launch restores everything.

### Phase 6: release (3 to 4 days, plus waiting time for signing)
- `tauri-action` GitHub workflow builds the NSIS `.exe` (primary: per-user install, no admin) plus `.msi` (for managed environments) on tag push and uploads them to a **draft** GitHub Release.
- Updater plugin: generate the updater key pair (this is separate from code signing; keep the private key in GitHub secrets and a password manager), publish `latest.json` with the release, and check on startup plus every 6 h. Updates are opt-in by default: show "Update available".
- Code signing and SmartScreen (see risks). Benchmark results in the README.
- **Done when**: a fresh Windows 11 VM installs from the release, the app auto-updates from v0.1.0 to v0.1.1, and the README shows measured numbers.

**Total: roughly 6 to 8 weeks of part-time work.**

---

## 6. Proving the RAM savings

**Metric**: Private Bytes summed per process tree, plus working set reported alongside. Sample every second for 5 minutes and report the median and p95. Use the same PowerShell script for every candidate, saved in the repo as `bench/measure.ps1`. The script walks the process tree from the root PID with CIM `Win32_Process` parent links and sums `PrivatePageCount`/`WorkingSetSize`.

**Scenarios** (each run 3 times, on a fresh boot, with the same CLI versions):
1. Empty app, no agents (pure overhead).
2. Two agents (Claude + Codex) idle at the prompt.
3. Four agents, one streaming a long response.
4. Four agents, three hidden in another project.

**Candidates**:
- **Windows Terminal running the same CLIs**: this is the floor. Spectacli overhead should be measured against it.
- **Spectacli**.
- **The Electron prototype (`agent-hub`)**: an Electron-hub baseline you already have.
- **Other hubs** (Agentrium, YzPzCode) if you can install them.
- **Desktop apps** (Claude desktop, ChatGPT desktop/Codex app, and others), labelled clearly as "different product, for context".

**Report**: a table plus one headline that you can defend, for example: *"With 4 agents, Spectacli adds 70 MB on top of the CLIs. An Electron hub adds 310 MB. Running the desktop apps instead uses 1.9 GB."* Publish the script and the raw CSVs so others can reproduce the numbers.

---

## 7. Risks and open questions

| Risk | Mitigation |
|---|---|
| **Terminal performance with many panes** | Batch output in Rust; WebGL only on visible panes; unmount hidden panes; cap scrollback (5,000 lines default); test 9 panes with `yes`-style floods. |
| **Status detection accuracy** | Hooks first; label the source; let the user correct it ("mark idle"); regexes live in the registry so they can be patched without a release. |
| **WebView2 differences** | Windows 11 ships it. On Windows 10, the NSIS installer's `downloadBootstrapper` mode installs it. Test on Windows 10 22H2 in a VM. Do not rely on newest-Chromium-only APIs. |
| **CLI changes (flags, output, hooks)** | Keep everything per-CLI in the registry; the startup version probe can warn "untested version"; maintain a small CI smoke test per CLI that runs `--help` and checks expected flags. |
| **Provider terms of service** | Spectacli launches official CLIs unchanged, which every vendor supports. Never scrape output to automate around rate limits, never share sessions across accounts, never touch credentials. Keep the not-affiliated notice, and avoid vendor logos (use text names only). |
| **Code signing and SmartScreen** | Unsigned builds show "Windows protected your PC", and a new certificate needs time to build reputation, EV included (EV no longer gives instant reputation). Options: **SignPath.io** (free for qualifying OSS projects), or **Azure Trusted Signing / Artifact Signing** (low monthly cost; check eligibility for individuals in your country). Until then, ship unsigned with clear README steps and publish SHA-256 hashes. Also consider a **winget** manifest. |
| **Updates** | The updater signature (minisign) is mandatory and separate from Authenticode. Losing the updater private key strands every user, so back it up. Keep a manual download link in every release note. |
| **ConPTY bugs** | Bundle the newer `conpty.dll`/`OpenConsole.exe`; debounce resize; add a "restart pane" action. |
| **Memory target missed** | WebView2 alone is about 50 to 70 MB with an empty page. Each xterm with WebGL adds a few MB plus GPU memory. The target "Spectacli ≤ 100 MB with 4 visible panes" is realistic; "≤ 50 MB" probably is not. Measure in phase 1, not at the end. |

**Open questions you can decide later** (none block phase 0 or 1):
- The exact name casing and the bundle identifier (decide before the first public release).
- Whether to keep experimental suspend at all.
- Telemetry: recommendation is none, for an app whose pitch is lightness and trust.

---

## 8. Project structure

```
spectacli/
├─ src/                         # React frontend
│  ├─ main.tsx  App.tsx
│  ├─ components/
│  │  ├─ TitleBar/  Sidebar/  PaneGrid/  TermPane/
│  │  ├─ AgentPicker/  CommandPalette/  BroadcastBar/  Inbox/
│  ├─ stores/                   # zustand: projects, agents, layout, stats, inbox
│  ├─ lib/ipc.ts                # typed invoke/Channel/listen wrappers
│  ├─ lib/layout.ts             # grid math (unit tested)
│  └─ styles/tokens.css
├─ src-tauri/
│  ├─ src/
│  │  ├─ main.rs  lib.rs  commands.rs  error.rs
│  │  ├─ registry/  (mod.rs, detect.rs, shim.rs)
│  │  ├─ pty/       (manager.rs, job.rs, batcher.rs, ring.rs)
│  │  ├─ stats/  status/  store/  session/  worktree/  paste/
│  ├─ resources/agents.toml
│  ├─ notify-helper/            # tiny bin crate: spectacli-notify.exe
│  ├─ capabilities/default.json # least-privilege Tauri permissions
│  ├─ tauri.conf.json  Cargo.toml
├─ bench/measure.ps1  bench/results/
├─ .github/workflows/ci.yml  release.yml
├─ README.md  LICENSE  PLAN.md  .gitignore
└─ CLAUDE.local.md  github.md   # both ignored
```

Use a Cargo workspace (`src-tauri` + `notify-helper`) so the helper exe is built and bundled as a Tauri `externalBin`.

---

## 9. Working rules for this repo

- Follow `github.md`: small imperative commits after each logical change, no AI attribution lines, no push unless asked, and never stage `github.md`.
- Use one assistant session per phase (or per task cluster) to keep context small. Start each session by pointing it at this file and the phase number.
- After each phase, run the benchmark script and record the numbers in `bench/results/`. That keeps the core claim honest throughout development rather than only at the end.
