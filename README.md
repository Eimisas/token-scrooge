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
scrooge setup        # one-time: installs hooks and downloads local models
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

These facts are stored immediately, and injected in future sessions when relevant. This is the primary workflow.

**Auto-extraction is now intelligent.** After each session ends, Scrooge's **Gatekeeper** (a local Qwen2.5 SLM) scans the transcript to capture decisions, fixes, and file changes automatically. It understands context better than simple patterns and extracts high-quality, structured facts without you lifting a finger.

```bash
scrooge recall "authentication"    # search what's stored
scrooge recall ""                  # list everything
scrooge daemon status              # check memory assistant health
scrooge gain                       # token savings report
```

---

## How it works

**Injection (before each prompt):** Your prompt is matched against stored facts using hybrid search (BM25 + Semantic Vectors). If Scrooge detects conflicting or redundant facts (e.g., an old decision vs. a new one), a local **Librarian** (Qwen2.5-0.5B) reconciles them into a single "current truth" before injection. Claude gets clean, reconciled context instead of a wall of messy fragments.

**Extraction (after each session):** The local **Gatekeeper** (Qwen2.5-0.5B) performs high-quality JSON extraction of technical decisions from your chat history. It automatically archives older, contradicting facts (e.g., switching from Redux to Zustand), keeping your memory pool refined and accurate.

**Storage:** Per-project SQLite database at `.scrooge/memory.db`. A background **daemon** keeps the models in memory for instant responses. Everything stays 100% local.

---

## Requirements

- Claude Code ≥ 2.0.12
- macOS or Linux
- ~500MB RAM/VRAM for the local SLM

---

[Advanced configuration & technical details →](advanced.md)
