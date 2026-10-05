# N43 — Task card

Build a small CLI. Use the project tool.

**wordcount** — a program that:

1. takes a file path from argv
2. reads the file (handle the missing-file case — don't just crash)
3. counts each whitespace-separated token
4. prints the total token count and the number of distinct tokens
5. prints the most frequent token and its count

Constraints:

- create the project with `our new wordcount`
- use `Result`/`?` for the file-read path
- use a map for the counts
- add one test in `tests/` and get `our test` green
- run `our fmt` and confirm it's a no-op (or accept its reformat)
- open `src/main.lang` with the LSP attached at least once — hover
  or diagnostics are enough

When done, say "done". If you're blocked >10 min on one thing, say
"stuck: <what>" and move on — that counts as data, not failure.
