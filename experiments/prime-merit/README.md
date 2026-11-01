# RBLabs Prime Merit Lab — Experiment 008 (Praetor)

This is the first implementation of the composite-first prime-gap search in **Praetor**, using the self-hosted `our` compiler. It lives under `experiments/` so it does not change compiler internals.

## Run

From the repository root with Praetor `our` (v0.2.0 or a current main-branch build) and clang on PATH:

```sh
our experiments/prime-merit/main.lang
./experiments/prime-merit/main
```

Depending on the platform and compiler's output path convention, the executable may instead be `./main` (or `main.exe` on Windows). Use the output path reported by `our`.

## What it does

- Builds a 210-position repeating mask for divisibility by 2, 3, 5, and 7.
- Reuses the mask for 65,536 integers starting at 1,000,000.
- Tests surviving candidates for divisibility by odd integers beginning at 11.
- Independently checks **every integer** using trial division starting at 2.
- Exits with a nonzero code if any classification differs.
- Reports candidate count, prime count, longest run covered by the small-prime mask, and largest prime gap fully contained in the interval.

This implementation makes no claim to be faster than a segmented sieve. In particular, its independent correctness oracle deliberately adds substantial work. Separate performance measurements must disable oracle work *only after correctness has been established*.

## Merit

The code emits integer endpoints. Compute the observed merit as `(right - left) / ln(left)` when evaluating the results. This first version deliberately avoids depending on floating-point logarithm support or third-party libraries.

## Current validation

The source was checked against the main-branch README, `stdlib/io.lang`, and representative native corpus fixtures. The Praetor compiler is not installed in the current execution environment; consequently, the source has **not** been successfully compiled or run here. A successful local build and an exit code of zero are mandatory before reporting correctness.

## Next steps

1. Compile with both `our` and `ourc` if available, and fix any differences revealed by the compiler.
2. Add known prime-gap fixtures and edge-case tests (including a gap spanning window boundaries).
3. Separate the oracle from a fair benchmark executable; compare with an optimized Praetor segmented sieve.
4. Introduce configurable wheel periods and bit-packed storage (if current compiler operations support them).
5. Add CRT residue-cover searches and validated endpoint proofs at larger scales.
