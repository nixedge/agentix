# Verification

- `nix develop .#storage --command cargo fmt --all --check`: passed.
- `nix develop .#storage --command cargo test -p agentix-indexer`: passed, 96 unit tests and 8 integration/grammar tests (104 total).
- `nix develop .#storage --command cargo clippy -p agentix-indexer --all-targets -- -D warnings`: passed.
- `nix build .#agentix-indexer --no-link`: passed for the final four-language source; canonical package derivation `8nkcrkwsm763rw56x01xsyzj9h6mygsp-agentix-indexer-0.1.0.drv`. The shared dependency build was the long-running step.
- `git diff --check`: passed.

Coverage includes discovery and case-insensitive extensions for Agda/literate Agda/Lean/Go/Elm, `.lake` and `elm-stuff` exclusions, Git ignores, bounded Unicode fallback windows, exact original source coordinates, Agda signature/equation/mixfix grouping and nested/error-recovered declarations, record fields and constructors, Lean theorem/lemma/attribute/namespace traversal, Go receiver methods/types/bindings, Elm annotation grouping/types/aliases, and both local and authenticated remote storage. CLI ingestion uses actual disposable LanceDB plus deterministic fixture embeddings and checks language/kind/project filters, unchanged runs, replacement and deletion.

Initial grammar testing exposed Agda qid names and error-recovery wrappers that the visitor did not handle. Those cases were corrected within the tree-sitter visitor and the full test suite passed afterward. Agentic-nix source indexing failed due embedding-model NullResult; the user authorized direct published-source inspection.

Workspace-wide tests and CI remain merge gates and have not been run for this feature. No deployed service or production repository index was refreshed. Existing Go indexes need a forced reindex to regenerate declaration chunks from unchanged files.
