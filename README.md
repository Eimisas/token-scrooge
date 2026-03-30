# Token Scrooge

<img src="scrooge.png" alt="Token Scrooge" width="180"/>

> Zero-setup persistent memory for Claude Code.

`scrooge` wraps the `claude` CLI. It watches your sessions, extracts decisions, bug fixes, and conventions, then silently injects the relevant ones before each message — without loading everything every time.

No config files. No running servers. One 4MB binary.

---

## Why

Claude forgets everything between sessions. Scrooge watches what happens, extracts what matters, and reminds Claude next time — automatically.

- **No re-explaining** — decisions, fixes, and conventions carry over on their own
- **Smarter than CLAUDE.md** — Claude's built-in memory files load the same static context every time; scrooge searches and injects only the facts relevant to your *current* prompt
- **Zero maintenance** — nothing to write or update; it runs silently in the background
- **Stacks with [RTK](https://github.com/rtk-ai/rtk)** — RTK compresses tool output, scrooge handles session memory; **combined they cut 30–50% of tokens on a typical session** 🤑💸

---

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/Eimisas/scrooge/main/install.sh | bash
```

Or build from source (requires Rust):

```bash
git clone https://github.com/Eimisas/scrooge
bash scrooge/install.sh
```

## Usage

```bash
scrooge claude
```

That's the only change you make. Memory is automatic from here.

On first run, scrooge creates `.scrooge/memory.db` in your project root and installs two hooks into `~/.claude/settings.json`. From that point, every session is silently watched — and every future session gets relevant context injected before Claude sees your message.

---

## Manual memory control

```bash
scrooge remember "we use JWT in httpOnly cookies, not localStorage"
scrooge remember "always use Result<T, AppError> in service layer" --tag convention
scrooge recall "authentication"      # search memory
scrooge forget <id>                  # remove a specific fact
scrooge --savings                    # token savings report
```

---

## How it works

```
Before each message
  your prompt ─► BM25 search over memory.db ─► top 3 matching facts
              ─► injected as invisible system context ─► Claude

After each session ends
  JSONL transcript ─► heuristic extraction ─► decisions, fixes, conventions
                   ─► stored in SQLite with content-hash deduplication
```

Facts are categorised by type:

| Category | Extracted from |
|---|---|
| `note` | Explicit "remember: …" messages from you |
| `fix` | "I've fixed…" in assistant messages |
| `decision` | "we decided to use…", "going with…" |
| `convention` | "from now on…", "always…", "never…" |
| `file` | Files created or significantly modified |
| `context` | Session summary bullet points |

Fact extraction uses heuristic regex patterns — no model calls, no ML inference. Retrieval uses SQLite FTS5 with BM25 ranking — sub-2ms on 1000+ facts.

---

## What gets stored where

```
~/.scrooge/memory.db           # global fallback (outside any project)
<project-root>/.scrooge/       # per-project (auto-added to .gitignore)
~/.claude/settings.json        # two hooks added here on first run
```

The per-project DB is local-only and automatically gitignored. Nothing leaves your machine.

---

## Works alongside RTK

If you use [RTK](https://github.com/rtk-ai/rtk) for tool output compression, scrooge and RTK work at different layers and don't conflict:

- **RTK** compresses what CLI tools return to Claude (`git status`, `cargo test`, etc.)
- **scrooge** manages what context Claude carries between sessions

**Running both gives 30–50% total token reduction on a typical session.**

---

## Requirements

- Claude Code ≥ 2.0.12
- macOS or Linux

No other runtime dependencies — SQLite is bundled in the binary.

---

## License

MIT
