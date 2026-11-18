# Index Agda and Lean

Run `just index /path/to/repo` with the updated indexer. Search using `search_code` or `bm25_search` with `language="agda"` or `language="lean"`.

Supported source: `.agda`, plain `.lagda`, Lean 4 `.lean` (case-insensitive). Tree-sitter extracts declaration chunks preserving Unicode and original line numbers. Filter by `symbol_kind="function"`, `"theorem"` (Lean), `"type"` or `"struct"`. Agda signatures and consecutive same-name equations are grouped. Lean additionally supports class/instance/axiom/const kinds for recognized named declarations. Anonymous instances use fallback if no other symbols exist. Unrecognized/no-symbol source uses bounded overlapping text windows.

Plain `.lagda` parses LaTeX code blocks and preserves original source coordinates. Compound formats such as `.lagda.md` are outside this feature. Lean `.lake` directories are excluded, as are existing ignored/generated directories and Git-ignored files. `.agdai` and `.olean` compiled outputs are not discovered.

Refresh existing repositories with normal indexing. For an immutable Git import already marked complete, repeat `ingest git <url> --rev <revision>` with `--force`.

Go (`.go`, `language="go"`) also uses tree-sitter for functions, receiver-qualified methods (`method`), structs/interfaces, type aliases, constants and variables. Elm (`.elm`, `language="elm"`) uses tree-sitter for functions, grouped annotations and definitions, custom types and aliases; `elm-stuff` is excluded. Run `just reindex /path/to/repo` (or `ingest code /path/to/repo --force`) for existing Go indexes so unchanged files are rebuilt into symbol chunks.

Tree-sitter is best-effort syntax extraction, not a type checker. Agda grammar recovery nodes are traversed for declarations; indented constructor/field source recovered as siblings is retained in its type/record chunk. Lean custom syntax may require fallback text indexing.
