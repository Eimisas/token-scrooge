# Token Scrooge

<p align="center"><img src="scrooge.png" alt="Token Scrooge" width="180"/></p>

> Zero-setup persistent memory for Claude Code.

Every session starts blank. You re-explain your stack, your conventions, your past decisions — again. Scrooge fixes that. It watches your sessions, extracts what matters, and injects it silently before Claude reads your next message.

---

## Why Scrooge?

*   **Stop Repeating Yourself:** Never explain your auth flow, CSS conventions, or "the way we do things here" twice.
*   **30–50% Token Savings:** Persistent memory reduces the need for long context-setting messages. Combined with [RTK](https://github.com/rtk-ai/rtk), it's the most aggressive way to cut your bill.
*   **Zero-Overhead Memory:** No LLM calls for extraction. No external vector DBs. No latency. Just a local SQLite file and a pure-Rust semantic engine.

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

Manual memory control:

```bash
scrooge remember "we use JWT in httpOnly cookies"   # save a fact
scrooge recall "authentication"                      # search memory
scrooge forget <id>                                  # remove a fact
scrooge uninstall                                    # remove this project's memory
scrooge uninstall --global                           # also remove hooks
```

---

## How it works

Before each message, your prompt is matched against stored facts using **Hybrid Search** (BM25 keyword matching + Semantic Vector similarity). The best matches are re-ranked by type (conventions score highest), relevance, recency, and usage frequency — then injected as invisible context before Claude sees your prompt.

After each session, the transcript is scanned with heuristic regex to extract decisions, conventions, and fixes automatically. Scrooge **compacts memory semantically**, deduplicating similar facts to ensure your context stays high-signal and low-noise.

---

## Requirements

- Claude Code ≥ 2.0.12
- macOS or Linux

SQLite is bundled — no other runtime dependencies.

---

[Advanced configuration & technical details →](advanced.md)
