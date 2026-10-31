# PraetorBench-1.3 — 20261031-123252 (lab-grade)

| workload | lang | mode | n | median s | 95% CI | rss B | size B | loc |
|---|---|---|---|---|---|---|---|---|
| jsonparse.lg | c | debug | 10 | 0.3000 | 0.3000–0.3200 | 196591616 | 34184 | 165 |
| jsonparse.lg | c | release | 10 | 0.2200 | 0.2200–0.2200 | 196608000 | 33928 | 165 |
| jsonparse.lg | cxx | debug | 10 | 0.3000 | 0.3000–0.3050 | 196591616 | 34280 | 167 |
| jsonparse.lg | cxx | release | 10 | 0.2200 | 0.2200–0.2300 | 196591616 | 33960 | 167 |
| jsonparse.lg | ours | debug | 10 | 0.5000 | 0.5000–0.5100 | 325681152 | 112680 | 273 |
| jsonparse.lg | ours | release | 10 | 0.1900 | 0.1850–0.1900 | 325861376 | 96232 | 273 |
| jsonparse.lg | rs | debug | 10 | 0.2700 | 0.2700–0.2700 | 202113024 | 526280 | 120 |
| jsonparse.lg | rs | release | 10 | 0.1200 | 0.1200–0.1200 | 202063872 | 492200 | 120 |
| jsonparse.md | c | debug | 20 | 0.0700 | 0.0700–0.0700 | 50561024 | 34184 | 165 |
| jsonparse.md | c | release | 20 | 0.0500 | 0.0500–0.0500 | 50577408 | 33928 | 165 |
| jsonparse.md | cxx | debug | 20 | 0.0700 | 0.0700–0.0700 | 50544640 | 34280 | 167 |
| jsonparse.md | cxx | release | 20 | 0.0500 | 0.0500–0.0500 | 50593792 | 33960 | 167 |
| jsonparse.md | ours | debug | 20 | 0.1200 | 0.1200–0.1200 | 83066880 | 112680 | 273 |
| jsonparse.md | ours | release | 20 | 0.0400 | 0.0400–0.0400 | 82722816 | 96232 | 273 |
| jsonparse.md | rs | debug | 20 | 0.0600 | 0.0600–0.0600 | 51445760 | 526280 | 120 |
| jsonparse.md | rs | release | 20 | 0.0300 | 0.0300–0.0300 | 51380224 | 492200 | 120 |
| jsonparse.sm | c | debug | 30 | 0.0000 | 0.0000–0.0000 | 3670016 | 34184 | 165 |
| jsonparse.sm | c | release | 30 | 0.0000 | 0.0000–0.0000 | 3735552 | 33928 | 165 |
| jsonparse.sm | cxx | debug | 30 | 0.0000 | 0.0000–0.0000 | 3670016 | 34280 | 167 |
| jsonparse.sm | cxx | release | 30 | 0.0000 | 0.0000–0.0000 | 3719168 | 33960 | 167 |
| jsonparse.sm | ours | debug | 30 | 0.0000 | 0.0000–0.0000 | 5144576 | 112680 | 273 |
| jsonparse.sm | ours | release | 30 | 0.0000 | 0.0000–0.0000 | 5111808 | 96232 | 273 |
| jsonparse.sm | rs | debug | 30 | 0.0000 | 0.0000–0.0000 | 3899392 | 526280 | 120 |
| jsonparse.sm | rs | release | 30 | 0.0000 | 0.0000–0.0000 | 3850240 | 492200 | 120 |

## paired ours − {lang} wall diff (median, 95% CI)

| workload | mode | vs | median diff s | 95% CI |
|---|---|---|---|---|
| jsonparse.sm | release | c | +0.0000 | +0.0000–+0.0000 |
| jsonparse.sm | release | rs | +0.0000 | +0.0000–+0.0000 |
| jsonparse.sm | debug | c | +0.0000 | +0.0000–+0.0000 |
| jsonparse.sm | debug | cxx | +0.0000 | +0.0000–+0.0000 |
| jsonparse.lg | release | rs | +0.0700 | +0.0650–+0.0700 |
| jsonparse.md | release | cxx | -0.0100 | -0.0100–-0.0100 |
| jsonparse.lg | release | cxx | -0.0300 | -0.0400–-0.0300 |
| jsonparse.md | debug | cxx | +0.0500 | +0.0500–+0.0500 |
| jsonparse.lg | debug | c | +0.2000 | +0.1850–+0.2050 |
| jsonparse.sm | debug | rs | +0.0000 | +0.0000–+0.0000 |
| jsonparse.md | debug | rs | +0.0600 | +0.0600–+0.0600 |
| jsonparse.md | debug | c | +0.0500 | +0.0500–+0.0500 |
| jsonparse.md | release | rs | +0.0100 | +0.0100–+0.0100 |
| jsonparse.lg | release | c | -0.0300 | -0.0400–-0.0300 |
| jsonparse.lg | debug | cxx | +0.2000 | +0.2000–+0.2050 |
| jsonparse.sm | release | cxx | +0.0000 | +0.0000–+0.0000 |
| jsonparse.md | release | c | -0.0100 | -0.0100–-0.0100 |
| jsonparse.lg | debug | rs | +0.2300 | +0.2300–+0.2400 |
