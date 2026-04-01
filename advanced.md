# Token Scrooge — Advanced

## Retrieval pipeline

```
your prompt
  → 1. FTS5 BM25 search → 15 keyword-matching candidates
  → 2. Local Embedding (all-MiniLM-L6-v2) → query vector
  → 3. Re-ranked: score = (bm25 + cosine_similarity_boost)
                         × category_weight 
                         × recency 
                         × access_boost
  → 4. Librarian (Qwen2.5-0.5B) → Reconciled Context
  → top result injected as invisible context
```

**Step 4: Librarian Reconciliation**. If Scrooge finds conflicting facts (e.g. 2 "decisions" with high similarity or same category), it invokes the local SLM to summarize them into a single coherent truth. If no conflicts are found, it falls back to raw fact injection for zero latency.

---

## Configuration

Create `.scrooge/config.toml` in your project, or run:

```bash
scrooge config init    # write default config to .scrooge/config.toml
scrooge config show    # print resolved config (including env overrides)
```

---

## All CLI commands

```bash
scrooge setup                           # install Claude Code hooks and download models
scrooge init                            # opt this project in — creates DB
scrooge claude [args...]                # run claude with memory assistant (starts daemon)
scrooge daemon start|stop|status        # manage the background memory assistant
scrooge remember "text" [--tag T]      # save a fact (tags: decision|fix|file|convention|context)
scrooge recall "query" [--limit N]     # search memory
scrooge forget <id>                     # delete a fact
scrooge expire [--days N] [--dry-run]  # archive stale facts
scrooge gain                            # token savings report
scrooge config show                     # print resolved config
scrooge config init [--force]           # write default config.toml
scrooge uninstall                       # delete .scrooge/ for this project
scrooge uninstall --global              # also remove hooks and local models
```

**Daemon**: The `scrooge daemon` manages the local SLM (Small Language Model). It is started automatically by `scrooge claude` and stays resident in memory to provide instant summarization and extraction.

---

## Memory maintenance

### 1. The Gatekeeper (Structured Ingestion)

When a session ends, the **Gatekeeper** uses the local SLM to scan the entire transcript for high-quality technical facts. It outputs structured JSON, which allows Scrooge to archive contradicting information. For example, if a new decision to "use Vite" is found, the Gatekeeper will mark an older "use Webpack" decision as archived.

### 2. The Librarian (Context Reconciliation)

During retrieval, the **Librarian** handles the "messy" cases where multiple facts about the same topic might be injected. It ensures that Claude sees a single, consistent paragraph rather than five different fragments with different timestamps.

---

## Storage layout

```
~/.scrooge/
  memory.db                       # global fallback (outside any project)
  models/                         # local models (Qwen2.5 + Embeddings)
  daemon.sock                     # IPC socket for daemon communication
<project-root>/.scrooge/
  memory.db                       # per-project facts
  config.toml                     # optional config overrides
  session-<id>.seen               # dedup state (cleaned up after session)
~/.claude/settings.json           # two hooks added on `scrooge setup`
```

The per-project DB is local-only and automatically added to `.gitignore`. Nothing leaves your machine.

---

## Session dedup

Within a single session, each fact is injected at most once. Scrooge tracks injected IDs in a `.seen` file that's cleaned up when the session ends.

---

## Extraction patterns

Extraction runs on the JSONL transcript after each session — no model calls. Messages are processed as adjacent User → Assistant pairs: when an assistant fix is vague ("I've fixed it"), the preceding user message is automatically appended as context so the fact remains searchable.

**From user messages:**

| Pattern | Example | Category |
|---|---|---|
| `remember: X`, `note: X`, `important: X` | "remember: JWT goes in httpOnly cookies" | user |
| `let's/lets use/go with X` | "let's use Zod for validation" | decision |
| `we're/are using X`, `we use X` | "we're using Postgres" | decision |
| `we decided / agreed to X` | "we decided to drop Redux" | decision |
| `from now on / always X` | "from now on use snake_case" | convention |
| `don't / never / avoid X` | "never store tokens in localStorage" | convention |
| `the approach / rule / pattern is X` | "the approach is to keep handlers thin" | convention |

**From assistant messages:**

| Pattern | Example | Category |
|---|---|---|
| `I've fixed / resolved X` | "I've fixed the null pointer in LoginForm" | fix |
| `the bug was / issue was X` | "the bug was a missing await" | fix |
| `<file> created / updated` | "created src/auth/jwt.ts" | file |

Summary bullet points are also scanned and matched against the same patterns, falling back to keyword detection.
