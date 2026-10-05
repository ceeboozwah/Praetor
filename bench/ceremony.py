#!/usr/bin/env python3
"""bench/ceremony.py — the 'how much machinery did the programmer
write' column. Deliberately a token count, documented as such:

Per-language regexes count explicit resource/ownership ceremony:
  ours : read/write/own/slice mut qualifiers, free, alloc*, destroy,
         replace, swap, `?`, `&`-borrows, explicit `let x: T`
  c    : malloc/calloc/realloc/free, memcpy, sizeof, pthread_*,
         manual loop-control over the same
  rust : mut, unsafe, Box/Rc/Arc, clone, unwrap/expect, `?`,
         `&`/`&mut`, explicit `let x: T`, `as` casts, move closures

Not a precision instrument — the same operation counts once in C and
once in ours. Its only claim: how many tokens did the author spend on
ownership/resource plumbing per workload, next to the runtime it bought.
"""
import os, re, sys

SRC = os.path.join(os.path.dirname(os.path.abspath(__file__)), "src")

PATTERNS = {
    "ours": r"\b(read|write|own)\b|\bfree\b|\balloc\w*\b|\bdestroy\b|"
            r"\breplace\b|\bswap\b|\bslice mut\b|\?|&|\blet \w+\s*:",
    "c":    r"\bmalloc\b|\bcalloc\b|\brealloc\b|\bfree\b|\bmemcpy\b|"
            r"\bmemmove\b|\bmemset\b|\bsizeof\b|\bpthread_\w+|\bbzero\b",
    "rs":   r"\bmut\b|\bunsafe\b|\bBox\b|\bRc\b|\bArc\b|\bclone\b|"
            r"\bunwrap\b|\bexpect\b|\?|&mut|&[a-zA-Z_(]|"
            r"\blet \w+\s*:|\bas \w+|\bmove\b|\bto_vec\b|\bcollect\b",
}

def main():
    names = sorted({f.rsplit(".", 1)[0] for f in os.listdir(SRC)
                    if f.rsplit(".", 1)[-1] in ("lang", "c", "rs")})
    print(f"{'workload':<12} {'ours':>6} {'c':>6} {'rust':>6}")
    for n in names:
        if n in ("httpload", "kvload"):
            continue
        row = [n]
        for ext, lang in (("lang", "ours"), ("c", "c"), ("rs", "rs")):
            p = os.path.join(SRC, f"{n}.{ext}")
            if not os.path.exists(p):
                row.append("-")
                continue
            src = open(p).read()
            row.append(str(len(re.findall(PATTERNS[lang], src))))
        print(f"{row[0]:<12} {row[1]:>6} {row[2]:>6} {row[3]:>6}")

if __name__ == "__main__":
    main()
