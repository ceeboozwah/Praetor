# N43A-2 — Task card

Build a small contact database CLI. Use the project tool.

**contacts** — a program that:

```text
contacts <dbfile> add <name> <phone>   append "name phone" to the db
contacts <dbfile> find <name>          print the phone, or exit 2
contacts <dbfile> remove <name>        rewrite the db without the entry
contacts <dbfile> list                 print every entry, one per line
```

The db file is plain text, one `name phone` pair per line. A missing
db file on `find`/`list` is an error; on `add` it means a new db.

Constraints:

- create the project with `our new contacts`
- take all inputs from argv
- use `Result`/`?` on the file paths
- structure it so a `load` step parses the file into a **map keyed by
  contact name** and returns it; the file's byte buffer must be
  released before the map is used
- `add`/`remove` must write the db back out (building output text
  yourself)
- put the core lookup function in `src/` as its own unit, and write
  a test in `tests/` that **imports that function and exercises it**
- run `our fmt` and confirm it's a no-op (or accept its reformat)
- run `our check`, `our test`, `our run` green
- attach the LSP to `src/main.lang` at least once — a client script
  is provided at `n43/lsp_client.py` (`python3 n43/lsp_client.py
  <diag|hover|def|comp> <file> [line col]`)

When done, say "done". If you're blocked >10 min on one thing, say
"stuck: <what>" and move on — that counts as data, not failure.
