#!/usr/bin/env python3
"""bench/gen.py — deterministic multi-scale workload inputs.

Scales (methodology §5): identical content across languages:
  corpus_{sm,md,lg}.txt  ~1/12/48 MB prose-like tokens (wordfreq)
  doc_{sm,md,lg}.json    ~256K/6/24 MB nested JSON (jsonparse)
  copy_{sm,md,lg}.bin    8/64/128 MB binary blob (fcopy)

Every file derives from its own fixed seed — GEN_SEED + scale index —
so content is reproducible and per-scale distinct. CPU workloads
(mapbench/psum/prodcons) and servers scale by argument, not files.

Provenance: writes data/manifest.json — {file: {seed, size, sha256}}.
On rerun, an existing file is VERIFIED against the manifest (hashed
and compared); it is regenerated when absent, unrecorded, or corrupt —
never trusted on existence alone. harness.py re-verifies against the
same manifest at measure time.
"""
import os, random, json, hashlib

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
MANIFEST = os.path.join(OUT, "manifest.json")


def sha256_of(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 22), b""):
            h.update(chunk)
    return h.hexdigest()


def emit(name, seed, write_fn):
    """Generate `name` via write_fn(path) unless a manifest-verified
    copy already exists; always (re)record its manifest entry."""
    path = os.path.join(OUT, name)
    try:
        with open(MANIFEST) as f:
            manifest = json.load(f)
    except (OSError, json.JSONDecodeError):
        manifest = {}
    ent = manifest.get(name)
    if not (os.path.exists(path) and ent
            and ent["sha256"] == sha256_of(path)):
        write_fn(path)
    manifest[name] = {"seed": seed, "size": os.path.getsize(path),
                      "sha256": sha256_of(path)}
    with open(MANIFEST, "w") as f:
        json.dump(manifest, f, indent=1, sort_keys=True)


def corpus(scale, rng):
    def write(path):
        with open(path, "w") as f:
            left = SIZES["corpus"][scale]
            while left > 0:
                n = rng.randint(4, 12)
                line = " ".join(rng.choice(WORDS) for _ in range(n)) + "\n"
                f.write(line)
                left -= len(line)
    emit(f"corpus_{scale}.txt", GEN_SEED + SCALES.index(scale), write)


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
    def write(path):
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
    emit(f"doc_{scale}.json", GEN_SEED + SCALES.index(scale), write)


def blob(scale, rng):
    def write(path):
        with open(path, "wb") as f:
            chunk = bytes(rng.randrange(256) for _ in range(1 << 20))
            for _ in range(SIZES["copy"][scale] >> 20):
                f.write(chunk)
    emit(f"copy_{scale}.bin", GEN_SEED + SCALES.index(scale), write)


if __name__ == "__main__":
    if not os.path.exists(MANIFEST):
        with open(MANIFEST, "w") as f:
            json.dump({}, f)
    for i, scale in enumerate(SCALES):
        rng = random.Random(GEN_SEED + i)  # per-scale seed
        corpus(scale, rng)
        doc(scale, rng)
        blob(scale, rng)
    print("generated+verified:", *sorted(
        n for n in os.listdir(OUT) if n != "manifest.json"))
