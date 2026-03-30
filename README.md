# Token Scrooge

<p align="center"><img src="scrooge.png" alt="Token Scrooge" width="180"/></p>

> Zero-setup persistent memory for Claude Code.

Every session starts blank. You re-explain your stack, your conventions, your past decisions — again.

Scrooge fixes that. It watches your sessions, extracts what matters, and injects it silently before Claude reads your next message.

---

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/Eimisas/scrooge/main/install.sh | bash
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

Manual memory control:

```bash
scrooge remember "we use JWT in httpOnly cookies"   # save a fact
scrooge recall "authentication"                      # search memory
scrooge forget <id>                                  # remove a fact
```

---

## How it works

Before each message, your prompt is matched against stored facts using BM25 search. The best matches are re-ranked by type (conventions score highest), recency, and usage frequency — then injected as invisible context before Claude sees your prompt.

After each session, the transcript is scanned with heuristic regex to extract decisions, conventions, and fixes automatically. No model calls. Works out of the box — optional per-project config available.

---

## Works alongside RTK

[RTK](https://github.com/rtk-ai/rtk) compresses tool output. Scrooge handles session memory. Running both typically cuts **30–50% of tokens** on a typical session.

---

## Requirements

- Claude Code ≥ 2.0.12
- macOS or Linux

SQLite is bundled — no other runtime dependencies.

---

[Advanced configuration & technical details →](advanced.md)
