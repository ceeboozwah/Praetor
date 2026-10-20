#!/usr/bin/env python3
"""bench/bce/selftest.py — regression tests for the BCE runner."""
import os, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import run as R

T = 0

def check(name, cond):
    global T
    T += 1
    print(("PASS" if cond else "FAIL"), name)
    assert cond, name


# 1. asm_sites_text — call-instruction counting, both formats
OT = """\
_scan:
0000000100000e38	cbz	x1, 0x100000e50
0000000100000e44	bl	_mml_bounds_fail
0000000100000e50	add	x8, x0, x9
_mml_bounds_fail:
0000000100000f00	ret
_main:
0000000100000f04	bl	_mml_bounds_fail
0000000100000f08	ldr	x9, _mml_bounds_fail_ref
"""
s = R.asm_sites_text(OT)
check("otool: per-fn call sites",
      s == {"_scan": 1, "_main": 1})
OBJ = """\
0000000000001140 <scan>:
    1140:	cmpq	%rsi, %rdx
    1144:	callq	1150 <mml_bounds_fail>
    1148:	movq	(%rax), %rax
0000000000001150 <mml_bounds_fail>:
    1150:	ret
0000000000001200 <main>:
    1204:	callq	1150 <mml_bounds_fail>
    1208:	b	_mml_bounds_fail_dataref
"""
s = R.asm_sites_text(OBJ)
check("objdump: per-fn call sites",
      s == {"scan": 1, "main": 1})
check("symbol defs/refs not counted",
      "mml_bounds_fail" not in s and s.get("?") is None)

# non-call mention (literal pool) doesn't count
OT2 = "0000000100001a00	add	x1, x1, #0xf5e ; literal: mml_bounds_fail\n"
check("literal-pool mention ignored",
      R.asm_sites_text(OT2) == {})

# 2. ll_sites — per-fn frontend/opt counting via a synthetic .ll
import tempfile
d = tempfile.mkdtemp()
ll = os.path.join(d, "x.ll")
with open(ll, "w") as f:
    f.write("define i64 @scan(ptr %p) {\n"
            "  call void @mml_bounds_fail(i64 0, i64 4)\n"
            "  ret i64 0\n}\n"
            "define i64 @main() {\n"
            "  call void @mml_bounds_fail(i64 1, i64 2)\n"
            "  call void @mml_bounds_fail(i64 3, i64 4)\n"
            "  ret i64 0\n}\n"
            "declare void @mml_bounds_fail(i64, i64)\n")
s = R.ll_sites(ll)
check("ll per-fn sites", s == {"scan": 1, "main": 2})
check("declare line not counted", all(v > 0 for v in s.values()))

# 3. median + bootstrap CI
import random, statistics
med, lo, hi = R.boot_median([1.0, 2.0, 3.0, 4.0], random.Random(1))
check("median even-count", med == 2.5)
check("CI brackets median", lo <= med <= hi)
r = R.boot_ratio([1.0] * 5, [2.0] * 5, random.Random(1))
check("ratio CI point", abs(r[0] - 0.5) < 1e-9 and r[1] == r[2] == 0.5)

# 4. alloc escape control — the C source MUST contain a volatile
#    escape so the alloc cycle can't be eliminated
src = open(os.path.join(R.SRC, "alloc_loop.c")).read()
check("alloc_loop.c has volatile escape", "volatile" in src)

print(f"\n{T} checks, ALL PASS")
