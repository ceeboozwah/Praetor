# PraetorBench-1.1 — 20261020-134357 (lab-grade)

| workload | lang | mode | n | median s | 95% CI | rss B | size B | loc |
|---|---|---|---|---|---|---|---|---|
| kvsrv.md | c | debug | 20 | 0.0747 | 0.0733–0.1008 | 1376256 | 34448 | 127 |
| kvsrv.md | c | release | 20 | 0.0749 | 0.0730–0.0851 | 1376256 | 34288 | 127 |
| kvsrv.md | cxx | debug | 20 | 0.0750 | 0.0744–0.0851 | 1392640 | 34480 | 134 |
| kvsrv.md | cxx | release | 20 | 0.0754 | 0.0734–0.0849 | 1392640 | 34312 | 134 |
| kvsrv.md | ours | debug | 20 | 0.0754 | 0.0730–0.0989 | 1425408 | 95840 | 142 |
| kvsrv.md | ours | release | 20 | 0.0741 | 0.0732–0.0775 | 1425408 | 79704 | 142 |
| kvsrv.md | rs | debug | 20 | 0.0736 | 0.0731–0.0783 | 1687552 | 559168 | 95 |
| kvsrv.md | rs | release | 20 | 0.0759 | 0.0738–0.0951 | 1638400 | 515552 | 95 |

## paired ours − {lang} wall diff (median, 95% CI)

| workload | mode | vs | median diff s | 95% CI |
|---|---|---|---|---|
| kvsrv.md | release | c | -0.0003 | -0.0009–+0.0007 |
| kvsrv.md | release | rs | -0.0007 | -0.0156–+0.0008 |
| kvsrv.md | debug | c | -0.0013 | -0.0057–+0.0042 |
| kvsrv.md | debug | rs | +0.0008 | -0.0002–+0.0057 |
| kvsrv.md | release | cxx | -0.0004 | -0.0013–+0.0004 |
| kvsrv.md | debug | cxx | -0.0008 | -0.0034–+0.0008 |
