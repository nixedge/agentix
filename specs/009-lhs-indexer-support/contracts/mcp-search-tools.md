# MCP Tool Contracts: search_code / search_docs

This feature makes no breaking changes to the MCP tool interface. All changes are additive and transparent to callers.

## Additive Changes

### `search_code` — `language=haskell` filter

**Before**: Returns only results from `.hs` files.

**After**: Returns results from `.hs` AND `.lhs` AND `.hsc` AND `.chs` AND `.hs-boot` files (all mapped to `language = "haskell"`).

No caller changes required. Callers using `language=haskell` automatically gain `.lhs` results.

### `search_docs` — new `doc_kind` values

**Before**: `doc_kind` values: `readme`, `docs`, `workflow`, `skill`, `sop`, `plan`, `agent_instruction`, `agent_index`.

**After**: Adds `lhs_prose` and `changelog`.

The `doc_kind` field is informational. Callers that do not filter on `doc_kind` are unaffected. Callers that filter for specific `doc_kind` values will not see the new types unless they add them.

---

## Existing Tool Contracts (unchanged)

### `search_code`

```
Input:
  query:       string          -- natural language or code query
  language:    string | null   -- filter by language; "haskell" now includes .lhs/.hsc/.chs
  symbol_kind: string | null   -- "function" | "class" | "type" | "impl" | "chunk" | null
  repo_path:   string | null   -- exact repo path or SQL LIKE pattern
  limit:       int             -- default 10

Output (per result):
  file_path:   string          -- e.g. "src/Cardano/Tracer/Observe/ObserveOutcome.lhs"
  repo_path:   string          -- e.g. "chap::iohk-monitoring-0.2.1.2"
  start_line:  int             -- original .lhs file line (not renumbered)
  end_line:    int             -- original .lhs file line
  language:    "haskell"       -- for all Haskell-family files
  symbol_kind: string | null   -- from tree-sitter; null for fallback line chunks
  content:     string          -- stripped Haskell (Bird prefix removed / LaTeX delimiters removed)
  score:       float           -- RRF relevance score
```

### `search_docs`

```
Input:
  query:    string          -- natural language query
  doc_kind: string | null   -- filter; new values: "lhs_prose", "changelog"
  repo_path: string | null
  limit:    int             -- default 10

Output (per result):
  source_path: string       -- e.g. "src/WithThreadAndTime.lhs"
  repo_path:   string
  doc_kind:    string       -- "lhs_prose" for prose from .lhs files
  title:       string       -- file stem (e.g. "WithThreadAndTime")
  content:     string       -- raw prose text from .lhs (LaTeX markup preserved)
  preview:     string       -- whitespace-collapsed first 280 chars
  score:       float
```

---

## Stability Guarantee

Per the constitution's MCP protocol section: tool schemas are stable across patch releases. This change is additive — new `language` values and `doc_kind` values in results. This is non-breaking by the constitution's definition: "additive changes (new tools, new optional fields) are non-breaking."
