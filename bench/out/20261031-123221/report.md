# PraetorBench-1.3 — 20261031-123221 (lab-grade)

| workload | lang | mode | n | median s | 95% CI | rss B | size B | loc |
|---|---|---|---|---|---|---|---|---|
| fcopy.lg | c | debug | 10 | 0.0200 | 0.0200–0.0300 | 1425408 | 33792 | 26 |
| fcopy.lg | c | release | 10 | 0.0200 | 0.0200–0.0200 | 1425408 | 33792 | 26 |
| fcopy.lg | cxx | debug | 10 | 0.0200 | 0.0200–0.0250 | 1441792 | 33792 | 27 |
| fcopy.lg | cxx | release | 10 | 0.0200 | 0.0200–0.0250 | 1441792 | 33800 | 27 |
| fcopy.lg | ours | debug | 10 | 0.0200 | 0.0200–0.0350 | 1441792 | 94720 | 39 |
| fcopy.lg | ours | release | 10 | 0.0200 | 0.0200–0.0300 | 1425408 | 78280 | 39 |
| fcopy.lg | rs | debug | 10 | 0.0200 | 0.0200–0.0400 | 1654784 | 521648 | 19 |
| fcopy.lg | rs | release | 10 | 0.0200 | 0.0200–0.0300 | 1622016 | 491952 | 19 |
| fcopy.md | c | debug | 20 | 0.0100 | 0.0100–0.0100 | 1409024 | 33792 | 26 |
| fcopy.md | c | release | 20 | 0.0100 | 0.0100–0.0100 | 1425408 | 33792 | 26 |
| fcopy.md | cxx | debug | 20 | 0.0100 | 0.0100–0.0100 | 1441792 | 33792 | 27 |
| fcopy.md | cxx | release | 20 | 0.0100 | 0.0100–0.0100 | 1441792 | 33800 | 27 |
| fcopy.md | ours | debug | 20 | 0.0100 | 0.0100–0.0100 | 1441792 | 94720 | 39 |
| fcopy.md | ours | release | 20 | 0.0100 | 0.0100–0.0100 | 1441792 | 78280 | 39 |
| fcopy.md | rs | debug | 20 | 0.0100 | 0.0100–0.0100 | 1638400 | 521648 | 19 |
| fcopy.md | rs | release | 20 | 0.0100 | 0.0100–0.0100 | 1622016 | 491952 | 19 |
| fcopy.sm | c | debug | 30 | 0.0000 | 0.0000–0.0000 | 1441792 | 33792 | 26 |
| fcopy.sm | c | release | 30 | 0.0000 | 0.0000–0.0000 | 1441792 | 33792 | 26 |
| fcopy.sm | cxx | debug | 30 | 0.0000 | 0.0000–0.0000 | 1441792 | 33792 | 27 |
| fcopy.sm | cxx | release | 30 | 0.0000 | 0.0000–0.0000 | 1458176 | 33800 | 27 |
| fcopy.sm | ours | debug | 30 | 0.0000 | 0.0000–0.0000 | 1458176 | 94720 | 39 |
| fcopy.sm | ours | release | 30 | 0.0000 | 0.0000–0.0000 | 1425408 | 78280 | 39 |
| fcopy.sm | rs | debug | 30 | 0.0000 | 0.0000–0.0000 | 1638400 | 521648 | 19 |
| fcopy.sm | rs | release | 30 | 0.0000 | 0.0000–0.0000 | 1622016 | 491952 | 19 |

## paired ours − {lang} wall diff (median, 95% CI)

| workload | mode | vs | median diff s | 95% CI |
|---|---|---|---|---|
| fcopy.sm | release | c | +0.0000 | +0.0000–+0.0000 |
| fcopy.sm | release | rs | +0.0000 | +0.0000–+0.0000 |
| fcopy.sm | debug | c | +0.0000 | +0.0000–+0.0000 |
| fcopy.sm | debug | cxx | +0.0000 | +0.0000–+0.0000 |
| fcopy.lg | release | rs | +0.0000 | -0.0100–+0.0100 |
| fcopy.md | release | cxx | +0.0000 | +0.0000–+0.0000 |
| fcopy.lg | release | cxx | +0.0000 | +0.0000–+0.0050 |
| fcopy.md | debug | cxx | +0.0000 | +0.0000–+0.0000 |
| fcopy.lg | debug | c | +0.0000 | +0.0000–+0.0050 |
| fcopy.sm | debug | rs | +0.0000 | +0.0000–+0.0000 |
| fcopy.md | debug | rs | +0.0000 | +0.0000–+0.0000 |
| fcopy.md | debug | c | +0.0000 | +0.0000–+0.0000 |
| fcopy.md | release | rs | +0.0000 | +0.0000–+0.0000 |
| fcopy.lg | release | c | +0.0000 | +0.0000–+0.0100 |
| fcopy.lg | debug | cxx | +0.0000 | +0.0000–+0.0050 |
| fcopy.sm | release | cxx | +0.0000 | +0.0000–+0.0000 |
| fcopy.md | release | c | +0.0000 | +0.0000–+0.0000 |
| fcopy.lg | debug | rs | +0.0000 | -0.0100–+0.0000 |
