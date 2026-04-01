# Token Scrooge — Advanced

## Retrieval pipeline

```
your prompt
  → 1. FTS5 BM25 search  → 15 keyword-matching candidates
  → 2. Semantic re-rank  → query vector (all-MiniLM-L6-v2, 80MB)
                           score = (bm25 + cosine_boost)
                                   × category_weight
                                   × recency
                                   × access_boost
  → 3. Conflict check    → if 2+ facts share a category or topic:
  → 4. Librarian         → Qwen2.5-0.5B reconciles into one paragraph
  → inject into Claude
```

If no conflict is detected, step 4 is skipped entirely — raw facts are injected with zero model latency.

---

## Ingestion pipeline

```
session ends
  → Gatekeeper (Qwen2.5-0.5B) → structured JSON extraction
  → heuristic regex             → fallback / coverage for short phrases
  → semantic dedup (0.85)       → Decision/Convention: supersede old fact
                                   Fix/File/User: merge (bump access count)
  → auto-archive stale facts (180 days)
```

The Gatekeeper handles natural phrasing ("let's switch to Zustand", "I think we should move away from Redux") that regex cannot. Regex runs in parallel as a fallback for very short or formulaic messages.

---

## Configuration

Create `.scrooge/config.toml` in your project, or run:

```bash
scrooge config init    # write default config to .scrooge/config.toml
scrooge config show    # print resolved config (including env overrides)
```

| Key | Default | Description |
|---|---|---|
| `max_injected_facts` | 4 | Maximum facts injected per prompt |
| `candidate_fetch` | 15 | BM25 candidates to re-rank |
| `recency_decay_days` | 90 | Linear decay window (1.0 today → 0.5 at N days) |
| `archive_after_days` | 180 | Inactivity threshold for auto-archival |
| `min_fact_priority` | 6 | Minimum SLM priority score (1–10) for auto-extracted facts to be stored (env: `SCROOGE_MIN_PRIORITY`) |

Category weights (higher = injected first):

| Category | Default |
|---|---|
| `convention` | 2.0 |
| `decision` | 1.5 |
| `fix` | 1.2 |
| `user` | 1.0 |
| `context` | 1.0 |
| `file` | 0.5 |

---

## All CLI commands

```bash
scrooge setup                           # install hooks, download models, start daemon
scrooge init                            # initialise DB for this project
scrooge claude [args...]                # run claude with memory (auto-starts/stops daemon)
scrooge daemon start [--foreground]     # start daemon manually
scrooge daemon stop                     # stop daemon
scrooge daemon status                   # check if daemon is running
scrooge remember "text" [--tag T]      # save a fact (tags: decision|fix|convention|context)
scrooge recall "query" [--limit N]     # search memory
scrooge recall --include-archived "q"  # include archived facts
scrooge forget <id>                     # delete a fact by ID
scrooge expire [--days N] [--dry-run]  # archive stale facts
scrooge gain                            # token savings report
scrooge config show                     # print resolved config
scrooge config init [--force]           # write default config.toml
scrooge uninstall                       # remove .scrooge/ for this project
scrooge uninstall --global              # also remove hooks, models, and binary
```

**Daemon lifecycle:** `scrooge claude` starts the daemon automatically if needed and always stops it when claude exits — it owns the lifecycle. For persistent setups (e.g. multiple concurrent sessions), manage the daemon manually with `scrooge daemon start` / `scrooge daemon stop` and invoke `claude` directly instead of `scrooge claude`.

---

## Memory maintenance

### Gatekeeper (ingestion)

When a session ends, the Gatekeeper reads the last N turns of the transcript and outputs structured JSON facts. For each new fact:

- **Decision or Convention** with >0.85 cosine similarity to an existing fact → the old fact is archived and the new one is inserted. "We use Zustand" correctly supersedes "We use Redux."
- **Fix, File, User** → if too similar, the existing fact's access count is bumped instead (historical records are not superseded).

### Librarian (retrieval)

During retrieval, if two or more of the top candidates share a category or are semantically close (cosine > 0.65), the Librarian synthesises a single coherent paragraph. Claude sees one clean "current state" rather than a timeline of contradictions.

If the daemon is not running, both steps fall back gracefully: ingestion uses regex heuristics, retrieval injects raw facts.

---

## Storage layout

```
~/.scrooge/
  memory.db                       # global fallback (outside any project)
  models/                         # Qwen2.5-0.5B + all-MiniLM-L6-v2 cache
  daemon.sock                     # IPC socket
<project-root>/.scrooge/
  memory.db                       # per-project facts
  config.toml                     # optional config overrides
  session-<id>.seen               # injected IDs for this session (cleaned on exit)
~/.claude/settings.json           # UserPromptSubmit + Stop hooks (added by scrooge setup)
```

The `.scrooge/` directory is automatically added to `.gitignore`. Nothing leaves your machine.

---

## Session deduplication

Within a single session, each fact is injected at most once. Scrooge tracks injected IDs in a `.seen` file that is cleaned up when the session ends.

---

## Heuristic extraction (regex fallback)

Runs alongside the Gatekeeper as a coverage layer for short, formulaic phrases.

**From user messages:**

| Pattern | Example | Category |
|---|---|---|
| `remember: X`, `note: X` | "remember: JWT goes in httpOnly cookies" | user |
| `let's use/go with/switch to X` | "let's switch to Zustand" | decision |
| `we're using X`, `we decided to X` | "we decided to drop Redux" | decision |
| `from now on X`, `always X` | "from now on use snake_case" | convention |
| `don't / never / avoid X` | "never store tokens in localStorage" | convention |
| `the approach / rule is X` | "the approach is thin handlers" | convention |

**From assistant messages:**

| Pattern | Example | Category |
|---|---|---|
| `I've fixed / resolved X` | "I've fixed the null pointer in LoginForm" | fix |
| `the bug/issue was X` | "the bug was a missing await" | fix |
| file created / modified | transcript tool events | file |

Filters: cognitive verbs ("don't understand"), transient instructions ("do not change any code for now"), multi-sentence captures, and XML system tags injected by Claude Code are all suppressed.
