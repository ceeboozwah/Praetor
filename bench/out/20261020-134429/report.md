# PraetorBench-1.1 — 20261020-134429 (lab-grade)

| workload | lang | mode | n | median s | 95% CI | rss B | size B | loc |
|---|---|---|---|---|---|---|---|---|
| httpsrv.sm | c | debug | 30 | 0.0244 | 0.0242–0.0248 | 1392640 | 34080 | 35 |
| httpsrv.sm | c | release | 30 | 0.0247 | 0.0243–0.0249 | 1392640 | 34008 | 35 |
| httpsrv.sm | cxx | debug | 30 | 0.0244 | 0.0240–0.0248 | 1392640 | 34088 | 37 |
| httpsrv.sm | cxx | release | 30 | 0.0246 | 0.0240–0.0249 | 1392640 | 34008 | 37 |
| httpsrv.sm | ours | debug | 30 | 0.0245 | 0.0241–0.0249 | 1392640 | 78504 | 39 |
| httpsrv.sm | ours | release | 30 | 0.0243 | 0.0239–0.0246 | 1392640 | 62040 | 39 |
| httpsrv.sm | rs | debug | 30 | 0.0246 | 0.0240–0.0250 | 1605632 | 548496 | 25 |
| httpsrv.sm | rs | release | 30 | 0.0241 | 0.0240–0.0244 | 1572864 | 498152 | 25 |

## paired ours − {lang} wall diff (median, 95% CI)

| workload | mode | vs | median diff s | 95% CI |
|---|---|---|---|---|
| httpsrv.sm | release | c | -0.0002 | -0.0006–-0.0000 |
| httpsrv.sm | release | rs | -0.0001 | -0.0004–+0.0000 |
| httpsrv.sm | debug | c | +0.0001 | -0.0002–+0.0003 |
| httpsrv.sm | debug | rs | +0.0002 | -0.0001–+0.0004 |
| httpsrv.sm | release | cxx | -0.0003 | -0.0006–+0.0001 |
| httpsrv.sm | debug | cxx | +0.0001 | -0.0003–+0.0007 |
