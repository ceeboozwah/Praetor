# N43 — Participant setup card

*This is the whole sheet the participant gets. Nothing more.*

---

You have a new systems language. Here's everything you get:

```sh
git clone <this repo>
cd memory-model-lab/compiler && cargo build --release
export PATH="$PWD/../target/release:$PATH"
```

Two binaries are now on your PATH:

- `ourc <file.lang>` — compiles a single file to a native binary
- `our` — project tool (`our new`, `our build`, `our run`, `our test`, `our fmt`)

Two docs if you want them: `SPEC.md` (the language) and `ERRORS.md`
(error codes). The formatter is `our fmt`. The LSP runs as
`ourc lsp` over stdio — point your editor's generic LSP client at it.

Go build the program on `TASK.md`.
