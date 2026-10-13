# masse roadmap

## Security hardening

masse is designed to run unattended against repositories, web pages and skills
that may be hostile. Prompt injection is treated as possible, not exceptional:
the controls below must hold even when the model is fully steered by an
attacker. The browser-facing server is the control plane and the sandbox is the
blast-radius limiter.

Threat model in one line: *untrusted content (repository, skills, docs, tool
results) tells the model to do something harmful through tools the model is
legitimately allowed to call.* The defense is the tool/policy layer plus OS
isolation — never prompt wording alone.

## Confirmed findings (source read, not a full audit)

| # | Finding | Where | Disposition |
| - | - | - | - |
| 1 | `write_file` and `create_dir` escape the workspace through a symlink (they use lexical `resolve_within`; read/delete/list canonicalize). | `fs_tools.rs:128`, `fs_tools.rs:300`, `path.rs:11` | **Fix** (immediate) |
| 2 | `bialet_run` / `bialet_server_start` are arbitrary Wren execution with Bialet's `Http` — the real network egress path, unaffected by the `curl` lockdown. | `bialet_tools.rs:47`, `bialet_tools.rs:204` | **Open design** — see below |
| 3 | The HTTP API and WebSocket have no authentication; `requires_approval` is advertised but never enforced. | `harness-server/src/lib.rs:43`, `harness-server/src/lib.rs:93` | Minor for now (loopback/VPN) |
| 4 | `requires_approval()` exists on the tool trait but the agent loop never consults it: no human-in-the-loop for irreversible actions. | `tool.rs:185`, `harness-core/src/agent.rs` | **Add** (with non-scary UX) |
| 5 | `git` runs on a repository treated as data: hooks/`core.hooksPath`/`core.fsmonitor` execute, and `git_push`/`auto_push` are an exfiltration channel. | `git_tools.rs:346`, `agent.rs:312` | **Split** git; see below |
| 6 | Full tool args and output are persisted (session.csv, `.masse/masse.log`) with no redaction. | `main.rs:427` | Redact later; secrets live elsewhere — see below |

## Decisions

### 2. Network egress (open)
Restricting `curl` to the local Bialet server is not enough: `bialet_run` and
`bialet_server_start` execute Wren that can call `Http`. The requirement is
that masse must be able to **create APIs and test them locally**, so the design
cannot simply forbid all runtime network use. It must distinguish:

- the app under test being reached on loopback (allowed), from
- arbitrary outbound destinations from app code or the model (denied).

Recommended shape: enforce egress **outside the tool layer** (network namespace /
firewall allowlist) rather than by prompting the model, and keep the loopback
path open for testing. To be designed.

### 3. Server auth (minor, for now)
The default bind is `127.0.0.1` and the intended exposure is a private network
(WireGuard/Tailscale) or an authenticated reverse proxy. No code change until
masse is exposed beyond loopback; then auth for `/api/*` and `/ws` (token +
`Origin` validation + size/rate limits) becomes P0.

### 4. Approvals
Enforce `requires_approval()` in the agent loop with an explicit policy
decision for irreversible tools (`git_push`, `git_commit`, `delete_dir`,
`sqlite_schema`). The prompt to the user must be **plain, calm and
non-technical** — the primary audience is non-developers. Describe what will
change in their app, not the tool name or SQL.

### 5. Git
Split git by blast radius, mirroring the SQLite split:

- **Read-only (always on):** `git_status`, `git_diff`, `git_log`, `git_branch`.
- **Write (gated, the only mutating/network tools):** `git_commit`,
  `git_push`, and a new `git_pull` (missing today).

Fold `git_stage` into `git_commit` (stage the intended paths there) rather than
exposing it separately. Harden every git invocation with
`-c core.hooksPath=/dev/null -c core.fsmonitor=` so a hostile repository cannot
run code on commit, and keep `--auto-push` off by default.

### 6. Secrets
There is no `.env` in Bialet: application secrets are stored in the SQLite
`BIALET_CONFIG` table (`_db.sqlite3`). Redaction in logs and any read denylist
must target that (and `.git/`, key material), not `.env*`. Redaction is a
P2 follow-up; the immediate concern is that a compromised model can read the
database and echo secrets into session.csv / the runtime log.

## Planned work

### P0
- [ ] Fix symlink escape in `write_file` / `create_dir` (finding 1): validate
      the canonical path of the nearest existing ancestor against the root.
- [ ] Design runtime network egress control that still allows testing local
      APIs (finding 2).
- [ ] Deployment: run each agent in a disposable container/VM, non-root, repo
      mounted writable, no home/SSH/Docker socket; resource limits; pass
      Bialet's own `-m/-M/-c/-C` limits to `bialet_server_start`.

### P1
- [ ] Enforce approvals with non-developer-friendly wording (finding 4).
- [ ] Split git and add `git_pull`; harden hooks; auto-push off (finding 5).
- [ ] Server auth (token + Origin + size/rate limits) before any non-loopback
      exposure (finding 3).
- [ ] Adversarial test suite asserting that execution is **blocked** (not that
      the model claims to refuse): symlink escape, SSRF, poisoned skill, hostile
      `AGENTS.md`, cross-session access.

### P2
- [ ] Redact secrets from session.csv and the runtime log (finding 6).
- [ ] Mark `AGENTS.md`, skills and docs as untrusted content and stop skill
      loading from silently granting new tools.
- [ ] Read denylist for `.git/`, key material and `BIALET_CONFIG`.

## Controls already in place (keep)

- No generic shell; external binaries run through `CmdSpec` with argument
  arrays, never a shell string (`cli.rs:105`).
- Read/delete/list/`edit_file` canonicalize and verify workspace containment.
- Markdown rendering escapes HTML and drops `javascript:`/`data:`/`vbscript:`/
  `file:` links, so the browser panel is not trivially XSS-able.
- `curl` is confined to the loopback port opened by `bialet_server_start`.
- Per-command timeouts; structured `{ stdout, stderr, exit_code }` results.

## Out of scope of masse (deployment)

Authenticated reverse proxy or private VPN, network firewall/allowlist, the
sandbox runtime itself, and keeping credentials out of the workspace are
deployment responsibilities. masse only provides the policy hooks they depend
on.
