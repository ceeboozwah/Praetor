#!/usr/bin/env python3
"""bench/gen.py — deterministic multi-scale workload inputs.

Scales (methodology §11 — identical content across languages):
  corpus_{sm,md,lg}.txt  ~1/12/48 MB prose-like tokens (wordfreq)
  doc_{sm,md,lg}.json    ~256K/6/24 MB nested JSON (jsonparse)
  copy_{sm,md,lg}.bin    8/64/128 MB binary blob (fcopy)

Every file derives from its own fixed seed — GEN_SEED + scale index —
so content is reproducible and per-scale distinct. CPU workloads
(mapbench/psum/prodcons) and servers scale by argument, not files.
"""
import os, random, json

OUT = os.path.join(os.path.dirname(__file__), "data")
os.makedirs(OUT, exist_ok=True)

GEN_SEED = 0xC0FFEE
SIZES = {  # approximate byte targets per scale
    "corpus": {"sm": 1 << 20, "md": 12 << 20, "lg": 48 << 20},
    "doc":    {"sm": 1 << 18, "md": 6 << 20, "lg": 24 << 20},
    "copy":   {"sm": 8 << 20, "md": 64 << 20, "lg": 128 << 20},
}
WORDS = """the quick brown fox jumps over lazy dog time year people way day
man thing woman life child world school state family student group country
problem hand part place case week company system program question work
night point home water room mother area money story fact month lot right
study book eye job word business issue side kind head house service friend
father power hour game line end member law car city community name team""".split()

SCALES = ("sm", "md", "lg")


def corpus(scale, rng):
    path = os.path.join(OUT, f"corpus_{scale}.txt")
    if os.path.exists(path):
        return
    with open(path, "w") as f:
        left = SIZES["corpus"][scale]
        while left > 0:
            n = rng.randint(4, 12)
            line = " ".join(rng.choice(WORDS) for _ in range(n)) + "\n"
            f.write(line)
            left -= len(line)


def node(rng, depth):
    r = rng.random()
    if depth > 6 or r < 0.30:
        return rng.randint(-10**6, 10**6)
    if r < 0.45:
        return rng.choice(WORDS) + str(rng.randint(0, 99))
    if r < 0.55:
        return rng.random() < 0.5
    if r < 0.60:
        return None
    if r < 0.85:
        return [node(rng, depth + 1) for _ in range(rng.randint(1, 6))]
    return {rng.choice(WORDS) + str(i): node(rng, depth + 1)
            for i in range(rng.randint(1, 6))}


def doc(scale, rng):
    path = os.path.join(OUT, f"doc_{scale}.json")
    if os.path.exists(path):
        return
    with open(path, "w") as f:
        f.write("[")
        left = SIZES["doc"][scale]
        first = True
        while left > 0:
            s = json.dumps(node(rng, 0))
            f.write(("" if first else ",") + s)
            first = False
            left -= len(s)
        f.write("]")


def blob(scale, rng):
    path = os.path.join(OUT, f"copy_{scale}.bin")
    if os.path.exists(path):
        return
    with open(path, "wb") as f:
        chunk = bytes(rng.randrange(256) for _ in range(1 << 20))
        for _ in range(SIZES["copy"][scale] >> 20):
            f.write(chunk)


if __name__ == "__main__":
    for i, scale in enumerate(SCALES):
        rng = random.Random(GEN_SEED + i)  # per-scale seed
        corpus(scale, rng)
        doc(scale, rng)
        blob(scale, rng)
    print("generated:", *sorted(os.listdir(OUT)))
