# Texly – Agent Conventions

## Three-Agent Workflow

All agents run with `--dangerously-skip-permissions`.

| Role | Responsibility |
|------|---------------|
| **Orchestrator** | Plans work, pulls tickets from Plankton, coordinates Developer and Tester, synthesizes findings |
| **Developer** | Implements code for the current ticket, writes unit tests, commits |
| **Tester** | Reviews output, runs tests, verifies acceptance criteria, approves or rejects in Plankton |

### Ticket Lifecycle (strict order)

1. Orchestrator pulls task from **Todo** → moves to **In Progress** via `move_task`
2. Developer implements, logs progress via `add_log` in Plankton
3. Developer moves task to **Testing** column, calls `submit_for_review`
4. Tester reviews, runs tests, adds findings via `add_comment`
5. Tester calls `approve_task` (→ Done) or `reject_task` (→ back to In Progress with comment)

> **Rule:** All progress is documented in Plankton (`add_log` / `add_comment`), not just in chat. Plankton is the single source of truth.

---

## Plankton Project

- **Project ID:** `d7243c19-db4a-40f2-8a97-b25e2e4961ea`
- **Slug:** `texly-self-hosted-latex-editor-flat-file`
- **Columns:** Todo → In Progress → Testing → Done

---

## Epic Execution Order

```
1. Engine-Validierung (epic: 75527067)   ← validate first, everything else depends on this
   └─ Reale .tex-Dokumente durch Tectonic jagen  (task: 3b6bc6ed)
   └─ Fallback evaluieren: TeX Live + latexmk    (task: 35354a29)  ← only if needed

2. Backend: Axum-Server + Compile-Pipeline (epic: adefde6c)
   └─ Projekt-Setup: Axum-Grundgerüst     (task: de64ac7f)
   └─ File-API: Projekte & Dateien (CRUD) (task: 58bcd6be)
   └─ Compile-Endpoint: Tectonic-Subprozess + PDF-Serving (task: 57292e8b)
   └─ Compile-Log-Parser                  (task: bb4e7602)

3. Frontend: Editor + PDF-Viewer (epic: b1cb6888)
   └─ Editor + Datei-Tree: CodeMirror 6 + Sidebar (task: e8c051b3)
   └─ PDF-Viewer: PDF.js + Auto-Compile           (task: 5bbc099d)
   └─ Fehler-Panel + Asset-Upload                 (task: 3030c846)

4. Deployment: Docker + CapRover + Auth (epic: ad90add8)
   └─ Dockerfile + persistentes Volume            (task: 1e513789)
   └─ Zugriffsschutz: Auth-Variante               (task: 723478c5)
   └─ CapRover-Deploy + Btrfs-Snapshot-Backup     (task: dc02d4d9)
```

---

## Stack Conventions

### Backend
- **Language:** Rust (stable toolchain, `cargo` workspace)
- **HTTP framework:** Axum + tokio + tower-http
- **Compile engine:** Tectonic (primary). Fallback: TeX Live + latexmk — decided by Engine-Validierung task.
- **No database** — projects are plain subdirectories under `TEXLY_PROJECTS_DIR` (env var)
- **Path-traversal protection is mandatory** on every file endpoint. Canonicalize all paths and verify they remain inside the project root before any read/write.

### Frontend
- **Decision made in task `de64ac7f` (Axum-Grundgerüst):** Choose between Vue 3 (SFC, Vite) vs. Vanilla JS + CodeMirror 6. Record the decision here and in the task log.
- **Editor:** CodeMirror 6 with LaTeX highlighting (`@codemirror/lang-legacy-modes` / StreamLanguage stex)
- **PDF viewer:** PDF.js (via CDN or bundled)
- **No build step preferred** if going Vanilla (reduces Docker complexity)

### Chosen Frontend Approach
> _To be filled in after the first Backend task is started. Options: "Vue 3 + Vite" or "Vanilla JS (no bundler)"._

---

## Repository & Commit Conventions

### Branch Naming
```
feat/<short-slug>          # new feature
fix/<short-slug>           # bug fix
spike/<short-slug>         # validation/research tasks
chore/<short-slug>         # tooling, config
```

### Commit Format
```
<type>(<scope>): <imperative short description>

<body — why, not what>

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>
```
Types: `feat` | `fix` | `chore` | `docs` | `test` | `refactor`

### Scope Examples
`backend`, `frontend`, `docker`, `api`, `compiler`, `parser`, `editor`

---

## Testing

### Backend (Rust)
```bash
cargo test                  # unit + integration tests
cargo clippy -- -D warnings # lint gate
cargo fmt --check           # format gate
```
- Integration tests live in `tests/` at crate root, use `axum::test` or `tower::ServiceExt`
- Every file endpoint test must include a path-traversal attempt that expects 400/403

### Frontend
- Manual verification in browser for UI tasks (Tester role)
- `npm test` or `vitest run` if a test suite exists

### Tester Checklist (minimum per ticket)
- [ ] Acceptance criteria from Plankton description fully met
- [ ] `cargo test` passes (backend tasks)
- [ ] `cargo clippy` clean
- [ ] No obvious regressions in adjacent features
- [ ] Path-traversal check for any new file endpoints

---

## Security Non-Negotiables

1. **Path traversal:** All `*path` segments must be canonicalized and verified to lie under `project_root` before I/O. Return 400 on violation.
2. **No shell injection:** Never interpolate user input into shell commands. Use `tokio::process::Command` with explicit args, not `sh -c`.
3. **Compile timeout:** Tectonic/latexmk subprocess must have a hard timeout (default 120 s) to prevent resource exhaustion.
4. **Compile concurrency lock:** One compile at a time per project (use `tokio::sync::Mutex` per project, or a global `DashMap<String, Mutex<()>>`).
