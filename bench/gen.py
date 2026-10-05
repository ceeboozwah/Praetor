#!/usr/bin/env python3
"""bench/gen.py — deterministic workload inputs into bench/data/.

  corpus.txt  ~12 MB prose-like tokens for wordfreq
  doc.json    ~6 MB nested JSON for jsonparse
  copy.bin    64 MB binary blob for fcopy
"""
import os, random, json

OUT = os.path.join(os.path.dirname(__file__), "data")
os.makedirs(OUT, exist_ok=True)
rng = random.Random(0xC0FFEE)

WORDS = """the quick brown fox jumps over lazy dog time year people way day
man thing woman life child world school state family student group country
problem hand part place case week company system program question work
night point home water room mother area money story fact month lot right
study book eye job word business issue side kind head house service friend
father power hour game line end member law car city community name team""".split()


def corpus():
    path = os.path.join(OUT, "corpus.txt")
    if os.path.exists(path):
        return
    with open(path, "w") as f:
        left = 12 * 1024 * 1024
        while left > 0:
            n = rng.randint(4, 12)
            line = " ".join(rng.choice(WORDS) for _ in range(n)) + "\n"
            f.write(line)
            left -= len(line)


def node(depth):
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
        return [node(depth + 1) for _ in range(rng.randint(1, 6))]
    return {rng.choice(WORDS) + str(i): node(depth + 1)
            for i in range(rng.randint(1, 6))}


def doc():
    path = os.path.join(OUT, "doc.json")
    if os.path.exists(path):
        return
    with open(path, "w") as f:
        f.write("[")
        left = 6 * 1024 * 1024
        first = True
        while left > 0:
            s = json.dumps(node(0))
            f.write(("" if first else ",") + s)
            first = False
            left -= len(s)
        f.write("]")


def blob():
    path = os.path.join(OUT, "copy.bin")
    if os.path.exists(path):
        return
    with open(path, "wb") as f:
        chunk = bytes(rng.randrange(256) for _ in range(1 << 20))
        for _ in range(64):
            f.write(chunk)


if __name__ == "__main__":
    corpus()
    doc()
    blob()
    for n in sorted(os.listdir(OUT)):
        print(n, os.path.getsize(os.path.join(OUT, n)))
