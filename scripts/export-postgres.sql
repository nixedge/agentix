-- psql "$SOURCE_DSN" -X -qAt -v ON_ERROR_STOP=1 -f scripts/export-postgres.sql > export.jsonl
-- Read-only snapshot of all search data. Keep this output private and immutable.
BEGIN TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY;
SELECT json_build_object('table','code_chunks','row',to_jsonb(c)-'embedding','embedding',c.embedding::text) FROM code_chunks c
UNION ALL SELECT json_build_object('table','documents','row',to_jsonb(c)-'embedding','embedding',c.embedding::text) FROM documents c
UNION ALL SELECT json_build_object('table','github_issues','row',to_jsonb(c)-'embedding','embedding',c.embedding::text) FROM github_issues c
UNION ALL SELECT json_build_object('table','github_prs','row',to_jsonb(c)-'embedding','embedding',c.embedding::text) FROM github_prs c
UNION ALL SELECT json_build_object('table','github_issue_comments','row',to_jsonb(c)-'embedding','embedding',c.embedding::text) FROM github_issue_comments c
UNION ALL SELECT json_build_object('table','github_pr_comments','row',to_jsonb(c)-'embedding','embedding',c.embedding::text) FROM github_pr_comments c
UNION ALL SELECT json_build_object('table','repo_index','row',to_jsonb(c),'embedding',NULL) FROM repo_index c
UNION ALL SELECT json_build_object('table','sync_state','row',to_jsonb(c),'embedding',NULL) FROM sync_state c;
COMMIT;
