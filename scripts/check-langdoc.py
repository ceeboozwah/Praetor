#!/usr/bin/env python3
"""check-langdoc.py — every ```lang block in LANGUAGE.md must still
compile under the current toolchain.

Rules:
  * a block containing `fn main` compiles as-is
  * a block with top-level decls (fn/struct/enum/extern/import) but
    no `main` gets a stub `fn main() -> i32 { return 0; }` appended
  * a statements-only block is wrapped in a main body; any `import`
    lines are hoisted out first
  * ```lang-frag is an explicit escape hatch for snippets that can
    never compile (use sparingly — every block should prefer the
    real compile path)

Run from the repo root. The compiler resolves stdlib via the usual
OUR_STDLIB / $OUR_HOME rules — no repo-relative tricks.

    python3 scripts/check-langdoc.py            # uses target/our
    OUR=./target/release/our python3 scripts/check-langdoc.py
"""

import os
import re
import subprocess
import sys
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUR = os.environ.get("OUR", os.path.join(ROOT, "target", "our"))

BLOCK = re.compile(r"```lang(-frag)?\n(.*?)```", re.S)
DECL = re.compile(r"^(fn |pub fn |struct |pub struct |enum |pub enum |extern )", re.M)
IMPORT = re.compile(r"^import [\w.]+;\s*$", re.M)


def compile_block(src: str, stem: str, work: str) -> tuple[bool, str]:
    """Returns (ok, stderr). Decl blocks get a stub main; statement
    blocks get wrapped."""
    if "fn main" in src:
        prog = src
    elif DECL.search(src):
        prog = src + "\nfn main() -> i32 {\n    return 0;\n}\n"
    else:
        imports = IMPORT.findall(src)
        body = IMPORT.sub("", src)
        prog = "\n".join(imports)
        prog += "\nfn main() -> i32 {\n" + body + "    return 0;\n}\n"
    f = os.path.join(work, stem + ".lang")
    with open(f, "w") as fh:
        fh.write(prog)
    r = subprocess.run([OUR, f, "-o", os.path.join(work, stem)],
                       capture_output=True, text=True)
    return r.returncode == 0, r.stderr


def main() -> int:
    src = open(os.path.join(ROOT, "LANGUAGE.md")).read()
    blocks = BLOCK.findall(src)
    work = tempfile.mkdtemp(prefix="langdoc-")
    ok = frag = fail = 0
    for i, (mark, body) in enumerate(blocks):
        if mark:
            frag += 1
            continue
        good, err = compile_block(body, f"b{i:02}", work)
        if good:
            ok += 1
        else:
            fail += 1
            line = src[: src.index(body)].count("\n") + 1
            print(f"FAIL block {i} (LANGUAGE.md ~line {line}):\n{err.strip()}")
    print(f"langdoc: {ok} compiled, {frag} frag-skipped, {fail} failed")
    return 1 if fail else 0


if __name__ == "__main__":
    sys.exit(main())
