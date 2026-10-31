# PraetorBench-1.3 — 20261031-123342 (lab-grade)

| workload | lang | mode | n | median s | 95% CI | rss B | size B | loc |
|---|---|---|---|---|---|---|---|---|
| wordfreq.lg | c | debug | 10 | 0.3200 | 0.3200–0.3200 | 51740672 | 33920 | 108 |
| wordfreq.lg | c | release | 10 | 0.1500 | 0.1500–0.1500 | 51757056 | 33800 | 108 |
| wordfreq.lg | cxx | debug | 10 | 0.3200 | 0.3200–0.3200 | 51789824 | 33992 | 115 |
| wordfreq.lg | cxx | release | 10 | 0.1500 | 0.1500–0.1500 | 51789824 | 33816 | 115 |
| wordfreq.lg | ours | debug | 10 | 0.6600 | 0.6600–0.6600 | 118996992 | 95128 | 62 |
| wordfreq.lg | ours | release | 10 | 0.2400 | 0.2400–0.2500 | 118980608 | 78696 | 62 |
| wordfreq.lg | rs | debug | 10 | 0.7300 | 0.7250–0.7400 | 52002816 | 526664 | 79 |
| wordfreq.lg | rs | release | 10 | 0.1400 | 0.1400–0.1400 | 51970048 | 491640 | 79 |
| wordfreq.md | c | debug | 20 | 0.0700 | 0.0700–0.0700 | 14008320 | 33920 | 108 |
| wordfreq.md | c | release | 20 | 0.0300 | 0.0300–0.0300 | 14008320 | 33800 | 108 |
| wordfreq.md | cxx | debug | 20 | 0.0750 | 0.0700–0.0800 | 14041088 | 33992 | 115 |
| wordfreq.md | cxx | release | 20 | 0.0300 | 0.0300–0.0300 | 14041088 | 33816 | 115 |
| wordfreq.md | ours | debug | 20 | 0.1600 | 0.1600–0.1600 | 30900224 | 95128 | 62 |
| wordfreq.md | ours | release | 20 | 0.0600 | 0.0600–0.0600 | 30883840 | 78696 | 62 |
| wordfreq.md | rs | debug | 20 | 0.1800 | 0.1800–0.1800 | 14254080 | 526664 | 79 |
| wordfreq.md | rs | release | 20 | 0.0300 | 0.0300–0.0300 | 14237696 | 491640 | 79 |
| wordfreq.sm | c | debug | 30 | 0.0000 | 0.0000–0.0000 | 2441216 | 33920 | 108 |
| wordfreq.sm | c | release | 30 | 0.0000 | 0.0000–0.0000 | 2457600 | 33800 | 108 |
| wordfreq.sm | cxx | debug | 30 | 0.0000 | 0.0000–0.0000 | 2473984 | 33992 | 115 |
| wordfreq.sm | cxx | release | 30 | 0.0000 | 0.0000–0.0000 | 2490368 | 33816 | 115 |
| wordfreq.sm | ours | debug | 30 | 0.0100 | 0.0100–0.0100 | 4603904 | 95128 | 62 |
| wordfreq.sm | ours | release | 30 | 0.0000 | 0.0000–0.0000 | 4587520 | 78696 | 62 |
| wordfreq.sm | rs | debug | 30 | 0.0100 | 0.0100–0.0100 | 2719744 | 526664 | 79 |
| wordfreq.sm | rs | release | 30 | 0.0000 | 0.0000–0.0000 | 2670592 | 491640 | 79 |

## paired ours − {lang} wall diff (median, 95% CI)

| workload | mode | vs | median diff s | 95% CI |
|---|---|---|---|---|
| wordfreq.sm | release | c | +0.0000 | +0.0000–+0.0000 |
| wordfreq.sm | release | rs | +0.0000 | +0.0000–+0.0000 |
| wordfreq.sm | debug | c | +0.0100 | +0.0100–+0.0100 |
| wordfreq.sm | debug | cxx | +0.0100 | +0.0100–+0.0100 |
| wordfreq.lg | release | rs | +0.1000 | +0.1000–+0.1100 |
| wordfreq.md | release | cxx | +0.0300 | +0.0300–+0.0300 |
| wordfreq.lg | release | cxx | +0.0900 | +0.0900–+0.1000 |
| wordfreq.md | debug | cxx | +0.0850 | +0.0800–+0.0900 |
| wordfreq.lg | debug | c | +0.3400 | +0.3400–+0.3400 |
| wordfreq.sm | debug | rs | +0.0000 | +0.0000–+0.0000 |
| wordfreq.md | debug | rs | -0.0200 | -0.0200–-0.0200 |
| wordfreq.md | debug | c | +0.0900 | +0.0900–+0.0900 |
| wordfreq.md | release | rs | +0.0300 | +0.0300–+0.0300 |
| wordfreq.lg | release | c | +0.0900 | +0.0900–+0.1000 |
| wordfreq.lg | debug | cxx | +0.3400 | +0.3400–+0.3400 |
| wordfreq.sm | release | cxx | +0.0000 | +0.0000–+0.0000 |
| wordfreq.md | release | c | +0.0300 | +0.0300–+0.0300 |
| wordfreq.lg | debug | rs | -0.0700 | -0.0800–-0.0650 |
