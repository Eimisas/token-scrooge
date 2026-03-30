# Token Scrooge — Advanced

## Retrieval pipeline

```
your prompt
  → FTS5 BM25 search → 15 candidates
  → re-ranked: score = bm25 × category_weight × recency × access_boost
  → top N injected as invisible context
```

**Category weights** (higher = injected first):

| Category | Weight | Captured from |
|---|---|---|
| `convention` | 2.0 | "from now on…", "always…", "never…", "the approach is…" |
| `decision` | 1.5 | "we decided…", "let's go with…", "we're using…" |
| `fix` | 1.2 | "I've fixed…", "the bug was…" |
| `user` | 1.0 | Explicit `scrooge remember "…"` commands |
| `context` | 1.0 | Session summary bullet points |
| `file` | 0.5 | Files created or significantly modified |

**Recency decay**: score decays linearly from 1.0 to 0.5 over 90 days.
**Access boost**: facts retrieved often get up to 1.5× multiplier (logarithmic, capped).

Empty-query fallback (no search terms) uses the same scoring formula ranked by category + recency + access count.

---

## Configuration

Create `.scrooge/config.toml` in your project, or run:

```bash
scrooge config init    # write default config to .scrooge/config.toml
scrooge config show    # print resolved config (including env overrides)
```

```toml
## Maximum facts injected per prompt (env: SCROOGE_MAX_FACTS)
max_injected_facts = 4

## BM25 candidates fetched before re-ranking
candidate_fetch = 15

## Days over which recency score decays from 1.0 to 0.5
recency_decay_days = 90

## Facts inactive this many days are automatically archived
archive_after_days = 180

[category_weights]
convention = 2.0
decision   = 1.5
fix        = 1.2
user       = 1.0
context    = 1.0
file       = 0.5
```

Partial files are supported — omitted keys use the defaults above.
`SCROOGE_MAX_FACTS=N` env var overrides `max_injected_facts` at runtime.

---

## All CLI commands

```bash
scrooge setup                           # install Claude Code hooks globally (once per machine)
scrooge init                            # opt this project in — creates DB, adds .gitignore entry
scrooge claude [args...]                # run claude with memory active
scrooge remember "text" [--tag T]      # save a fact (tags: decision|fix|file|convention|context)
scrooge recall "query" [--limit N]     # search memory
scrooge recall "query" --include-archived  # include archived facts
scrooge forget <id>                     # delete a fact
scrooge expire [--days N] [--dry-run]  # archive stale facts
scrooge gain                            # token savings report
scrooge config show                     # print resolved config
scrooge config init [--force]           # write default config.toml
scrooge uninstall                       # delete .scrooge/ for this project
scrooge uninstall --global              # also remove hooks from ~/.claude/settings.json
```

**`setup` vs `init`**: `setup` installs the hooks into `~/.claude/settings.json` — do this once. `init` opts a specific project in by creating `.scrooge/memory.db` — the hooks only activate for projects that have been initialised.

---

## Archival

Facts are soft-deleted, not hard-deleted. After a session ends, scrooge automatically archives facts not accessed in `archive_after_days` (default: 180). Archived facts are hidden from search but recoverable.

```bash
scrooge expire --dry-run            # preview what would be archived
scrooge expire --days 90            # archive facts idle for 90+ days
scrooge recall "query" --include-archived  # search including archived
```

---

## Storage layout

```
~/.scrooge/memory.db              # global fallback (outside any project)
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
