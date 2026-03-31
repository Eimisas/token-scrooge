# Token Scrooge

<p align="center"><img src="scrooge.png" alt="Token Scrooge" width="180"/></p>

> Persistent memory for Claude Code. Stop re-explaining yourself every session.

Every session starts blank. You re-explain your stack, your conventions, your past decisions — again. Scrooge fixes that by storing facts in a local SQLite database and injecting the relevant ones silently before Claude reads your next message.

---

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/Eimisas/scrooge/master/install.sh | bash
```

Or build from source:

```bash
git clone https://github.com/Eimisas/scrooge
bash scrooge/install.sh
```

## Usage

```bash
scrooge setup        # one-time: installs Claude Code hooks globally
scrooge init         # opt this project in (creates DB, no Claude launch)
scrooge claude       # use instead of `claude` — memory is automatic
```

---

## The right way to use it

**Explicit memory is reliable. Use it for anything that matters:**

```bash
scrooge remember "we use Zustand for state management"
scrooge remember "auth uses httpOnly cookies, not localStorage"
scrooge remember "handlers must stay thin — business logic goes in services"
```

These facts are stored immediately, categorised as decisions or conventions, and injected in future sessions when relevant. This is the primary workflow.

**Auto-extraction is a passive bonus.** After each session ends, Scrooge scans the transcript with heuristic patterns and tries to capture decisions, fixes, and file changes automatically. It works well for structured phrases ("let's use X", "we decided to Y", "I've fixed Z in file.rs") and for tracking which files were created or modified. It won't catch everything, and occasionally it catches noise — treat it as a convenience, not a guarantee.

```bash
scrooge recall "authentication"    # search what's stored
scrooge recall ""                  # list everything
scrooge forget <id>                # remove a bad or stale fact
scrooge gain                       # token savings report
```

---

## How it works

**Injection (before each prompt):** Your prompt is matched against stored facts using hybrid search — BM25 keyword matching combined with semantic vector similarity (local BERT model, no API calls). Matches are re-ranked by category (conventions and decisions score highest), recency, and usage frequency. The top results are injected as invisible context before Claude reads your message.

**Extraction (after each session):** The transcript is scanned with regex heuristics to extract facts automatically. When a new decision or convention is semantically similar to an existing one (cosine similarity > 0.75), the old fact is archived and the new one takes its place — so switching from Redux to Zustand correctly supersedes the old fact rather than accumulating contradictions.

**Storage:** Per-project SQLite database at `.scrooge/memory.db`. Facts older than 180 days without access are archived automatically. Everything stays local.

---

## Requirements

- Claude Code ≥ 2.0.12
- macOS or Linux

SQLite is bundled — no other runtime dependencies.

---

[Advanced configuration & technical details →](advanced.md)
