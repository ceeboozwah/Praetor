# PraetorBench-1.1 — 20261020-132032 (lab-grade)

| workload | lang | mode | n | median s | 95% CI | rss B | size B | loc |
|---|---|---|---|---|---|---|---|---|
| fcopy.lg | c | debug | 10 | 0.0200 | 0.0200–0.0300 | 1409024 | 33792 | 26 |
| fcopy.lg | c | release | 10 | 0.0200 | 0.0200–0.0400 | 1425408 | 33792 | 26 |
| fcopy.lg | cxx | debug | 10 | 0.0200 | 0.0200–0.0350 | 1441792 | 33792 | 27 |
| fcopy.lg | cxx | release | 10 | 0.0200 | 0.0200–0.0500 | 1425408 | 33800 | 27 |
| fcopy.lg | ours | debug | 10 | 0.0200 | 0.0200–0.0600 | 1441792 | 77152 | 39 |
| fcopy.lg | ours | release | 10 | 0.0200 | 0.0200–0.0200 | 1425408 | 60712 | 39 |
| fcopy.lg | rs | debug | 10 | 0.0200 | 0.0200–0.0250 | 1638400 | 521648 | 19 |
| fcopy.lg | rs | release | 10 | 0.0200 | 0.0200–0.0200 | 1605632 | 491952 | 19 |
| fcopy.md | c | debug | 20 | 0.0100 | 0.0100–0.0100 | 1425408 | 33792 | 26 |
| fcopy.md | c | release | 20 | 0.0100 | 0.0100–0.0100 | 1425408 | 33792 | 26 |
| fcopy.md | cxx | debug | 20 | 0.0100 | 0.0100–0.0100 | 1458176 | 33792 | 27 |
| fcopy.md | cxx | release | 20 | 0.0100 | 0.0100–0.0100 | 1441792 | 33800 | 27 |
| fcopy.md | ours | debug | 20 | 0.0100 | 0.0100–0.0100 | 1425408 | 77152 | 39 |
| fcopy.md | ours | release | 20 | 0.0100 | 0.0100–0.0100 | 1425408 | 60712 | 39 |
| fcopy.md | rs | debug | 20 | 0.0100 | 0.0100–0.0150 | 1654784 | 521648 | 19 |
| fcopy.md | rs | release | 20 | 0.0100 | 0.0100–0.0100 | 1622016 | 491952 | 19 |
| fcopy.sm | c | debug | 30 | 0.0000 | 0.0000–0.0000 | 1425408 | 33792 | 26 |
| fcopy.sm | c | release | 30 | 0.0000 | 0.0000–0.0000 | 1409024 | 33792 | 26 |
| fcopy.sm | cxx | debug | 30 | 0.0000 | 0.0000–0.0000 | 1441792 | 33792 | 27 |
| fcopy.sm | cxx | release | 30 | 0.0000 | 0.0000–0.0000 | 1458176 | 33800 | 27 |
| fcopy.sm | ours | debug | 30 | 0.0000 | 0.0000–0.0000 | 1441792 | 77152 | 39 |
| fcopy.sm | ours | release | 30 | 0.0000 | 0.0000–0.0000 | 1441792 | 60712 | 39 |
| fcopy.sm | rs | debug | 30 | 0.0000 | 0.0000–0.0000 | 1654784 | 521648 | 19 |
| fcopy.sm | rs | release | 30 | 0.0000 | 0.0000–0.0000 | 1605632 | 491952 | 19 |
| httpsrv.lg | c | debug | 10 | 0.1807 | 0.1761–0.1866 | 1376256 | 34080 | 35 |
| httpsrv.lg | c | release | 10 | 0.1912 | 0.1801–0.2113 | 1376256 | 34008 | 35 |
| httpsrv.lg | cxx | debug | 10 | 0.1873 | 0.1852–0.2012 | 1409024 | 34088 | 37 |
| httpsrv.lg | cxx | release | 10 | 0.1842 | 0.1807–0.2111 | 1392640 | 34008 | 37 |
| httpsrv.lg | ours | debug | 10 | 0.1861 | 0.1793–0.2288 | 1392640 | 78504 | 39 |
| httpsrv.lg | ours | release | 10 | 0.1880 | 0.1757–0.1984 | 1376256 | 62040 | 39 |
| httpsrv.lg | rs | debug | 10 | 0.1869 | 0.1810–0.2154 | 1589248 | 548496 | 25 |
| httpsrv.lg | rs | release | 10 | 0.1924 | 0.1794–0.2064 | 1589248 | 498152 | 25 |
| httpsrv.md | c | debug | 20 | 0.0756 | 0.0721–0.0817 | 1392640 | 34080 | 35 |
| httpsrv.md | c | release | 20 | 0.0728 | 0.0712–0.0748 | 1392640 | 34008 | 35 |
| httpsrv.md | cxx | debug | 20 | 0.0728 | 0.0718–0.0743 | 1392640 | 34088 | 37 |
| httpsrv.md | cxx | release | 20 | 0.0741 | 0.0722–0.0766 | 1409024 | 34008 | 37 |
| httpsrv.md | ours | debug | 20 | 0.0745 | 0.0724–0.0759 | 1409024 | 78504 | 39 |
| httpsrv.md | ours | release | 20 | 0.0729 | 0.0713–0.0739 | 1392640 | 62040 | 39 |
| httpsrv.md | rs | debug | 20 | 0.0721 | 0.0714–0.0744 | 1605632 | 548496 | 25 |
| httpsrv.md | rs | release | 20 | 0.0724 | 0.0708–0.0746 | 1572864 | 498152 | 25 |
| httpsrv.sm | c | debug | 30 | 0.0252 | 0.0230–0.0264 | 1392640 | 34080 | 35 |
| httpsrv.sm | c | release | 30 | 0.0230 | 0.0225–0.0248 | 1392640 | 34008 | 35 |
| httpsrv.sm | cxx | debug | 30 | 0.0236 | 0.0230–0.0250 | 1409024 | 34088 | 37 |
| httpsrv.sm | ours | debug | 30 | 0.0244 | 0.0232–0.0252 | 1392640 | 78504 | 39 |
| httpsrv.sm | ours | release | 30 | 0.0246 | 0.0228–0.0252 | 1392640 | 62040 | 39 |
| httpsrv.sm | rs | debug | 30 | 0.0241 | 0.0230–0.0262 | 1605632 | 548496 | 25 |
| httpsrv.sm | rs | release | 30 | 0.0238 | 0.0228–0.0247 | 1572864 | 498152 | 25 |
| jsonparse.lg | c | debug | 10 | 0.2950 | 0.2900–0.3000 | 196608000 | 34184 | 165 |
| jsonparse.lg | c | release | 10 | 0.2200 | 0.2100–0.2250 | 196624384 | 33928 | 165 |
| jsonparse.lg | cxx | debug | 10 | 0.2950 | 0.2900–0.3000 | 196608000 | 34280 | 167 |
| jsonparse.lg | cxx | release | 10 | 0.2200 | 0.2100–0.2200 | 196657152 | 33960 | 167 |
| jsonparse.lg | ours | debug | 10 | 0.4800 | 0.4700–0.5000 | 327237632 | 95080 | 273 |
| jsonparse.lg | ours | release | 10 | 0.1800 | 0.1700–0.1900 | 325648384 | 78648 | 273 |
| jsonparse.lg | rs | debug | 10 | 0.2550 | 0.2500–0.2650 | 202096640 | 526280 | 120 |
| jsonparse.lg | rs | release | 10 | 0.1200 | 0.1150–0.1200 | 202063872 | 492200 | 120 |
| jsonparse.md | c | debug | 20 | 0.0700 | 0.0700–0.0700 | 50593792 | 34184 | 165 |
| jsonparse.md | c | release | 20 | 0.0500 | 0.0500–0.0600 | 50659328 | 33928 | 165 |
| jsonparse.md | cxx | debug | 20 | 0.0700 | 0.0700–0.0700 | 50577408 | 34280 | 167 |
| jsonparse.md | cxx | release | 20 | 0.0500 | 0.0500–0.0500 | 50610176 | 33960 | 167 |
| jsonparse.md | ours | debug | 20 | 0.1200 | 0.1200–0.1200 | 82673664 | 95080 | 273 |
| jsonparse.md | ours | release | 20 | 0.0400 | 0.0400–0.0400 | 82640896 | 78648 | 273 |
| jsonparse.md | rs | debug | 20 | 0.0600 | 0.0600–0.0600 | 51445760 | 526280 | 120 |
| jsonparse.md | rs | release | 20 | 0.0300 | 0.0300–0.0300 | 51380224 | 492200 | 120 |
| jsonparse.sm | c | debug | 30 | 0.0000 | 0.0000–0.0000 | 3653632 | 34184 | 165 |
| jsonparse.sm | c | release | 30 | 0.0000 | 0.0000–0.0000 | 3719168 | 33928 | 165 |
| jsonparse.sm | cxx | debug | 30 | 0.0000 | 0.0000–0.0000 | 3637248 | 34280 | 167 |
| jsonparse.sm | cxx | release | 30 | 0.0000 | 0.0000–0.0000 | 3702784 | 33960 | 167 |
| jsonparse.sm | ours | debug | 30 | 0.0000 | 0.0000–0.0000 | 5144576 | 95080 | 273 |
| jsonparse.sm | ours | release | 30 | 0.0000 | 0.0000–0.0000 | 5079040 | 78648 | 273 |
| jsonparse.sm | rs | debug | 30 | 0.0000 | 0.0000–0.0000 | 3899392 | 526280 | 120 |
| jsonparse.sm | rs | release | 30 | 0.0000 | 0.0000–0.0000 | 3850240 | 492200 | 120 |
| kvsrv.lg | c | debug | 10 | 0.1929 | 0.1829–0.1998 | 1376256 | 34448 | 127 |
| kvsrv.lg | c | release | 10 | 0.1893 | 0.1816–0.2035 | 1376256 | 34288 | 127 |
| kvsrv.lg | cxx | debug | 10 | 0.1873 | 0.1799–0.2211 | 1409024 | 34480 | 134 |
| kvsrv.lg | cxx | release | 10 | 0.1890 | 0.1872–0.2320 | 1409024 | 34312 | 134 |
| kvsrv.lg | ours | debug | 10 | 0.1944 | 0.1821–0.2335 | 1425408 | 95840 | 142 |
| kvsrv.lg | ours | release | 10 | 0.1984 | 0.1858–0.2282 | 1409024 | 79704 | 142 |
| kvsrv.lg | rs | debug | 10 | 0.1947 | 0.1799–0.2093 | 1687552 | 559168 | 95 |
| kvsrv.lg | rs | release | 10 | 0.1873 | 0.1808–0.1985 | 1638400 | 515552 | 95 |
| kvsrv.md | c | debug | 20 | 0.0742 | 0.0733–0.0756 | 1392640 | 34448 | 127 |
| kvsrv.md | c | release | 20 | 0.0743 | 0.0711–0.0804 | 1376256 | 34288 | 127 |
| kvsrv.md | cxx | debug | 20 | 0.0740 | 0.0722–0.0763 | 1409024 | 34480 | 134 |
| kvsrv.md | ours | debug | 20 | 0.0737 | 0.0726–0.0783 | 1425408 | 95840 | 142 |
| kvsrv.md | ours | release | 20 | 0.0744 | 0.0729–0.0753 | 1425408 | 79704 | 142 |
| kvsrv.md | rs | debug | 20 | 0.0746 | 0.0726–0.0823 | 1687552 | 559168 | 95 |
| kvsrv.md | rs | release | 20 | 0.0749 | 0.0734–0.0776 | 1638400 | 515552 | 95 |
| kvsrv.sm | c | debug | 30 | 0.0237 | 0.0227–0.0253 | 1376256 | 34448 | 127 |
| kvsrv.sm | c | release | 30 | 0.0236 | 0.0227–0.0248 | 1376256 | 34288 | 127 |
| kvsrv.sm | cxx | debug | 30 | 0.0237 | 0.0226–0.0250 | 1409024 | 34480 | 134 |
| kvsrv.sm | cxx | release | 30 | 0.0246 | 0.0231–0.0255 | 1392640 | 34312 | 134 |
| kvsrv.sm | ours | debug | 30 | 0.0233 | 0.0228–0.0244 | 1425408 | 95840 | 142 |
| kvsrv.sm | ours | release | 30 | 0.0230 | 0.0225–0.0238 | 1425408 | 79704 | 142 |
| kvsrv.sm | rs | debug | 30 | 0.0245 | 0.0231–0.0262 | 1687552 | 559168 | 95 |
| kvsrv.sm | rs | release | 30 | 0.0233 | 0.0227–0.0247 | 1654784 | 515552 | 95 |
| mapbench.lg | c | debug | 10 | 1.8050 | 1.7150–1.9150 | 1612201984 | 33680 | 75 |
| mapbench.lg | c | release | 10 | 1.0300 | 0.9900–1.2600 | 1612201984 | 33608 | 75 |
| mapbench.lg | cxx | debug | 10 | 1.8050 | 1.7200–1.9400 | 1612218368 | 33736 | 76 |
| mapbench.lg | cxx | release | 10 | 1.0200 | 0.9600–1.0700 | 1612218368 | 33624 | 76 |
| mapbench.lg | ours | debug | 10 | 2.5250 | 2.3700–2.6850 | 1612234752 | 57320 | 109 |
| mapbench.lg | ours | release | 10 | 1.0650 | 1.0100–1.2350 | 1612218368 | 40760 | 109 |
| mapbench.lg | rs | debug | 10 | 3.6600 | 3.4750–3.7800 | 1612398592 | 503992 | 62 |
| mapbench.lg | rs | release | 10 | 1.0600 | 1.0150–1.2850 | 1612414976 | 490568 | 62 |
| mapbench.md | c | debug | 20 | 0.5000 | 0.4750–0.5250 | 404226048 | 33680 | 75 |
| mapbench.md | c | release | 20 | 0.2600 | 0.2450–0.2750 | 404226048 | 33608 | 75 |
| mapbench.md | cxx | debug | 20 | 0.4850 | 0.4700–0.5000 | 404226048 | 33736 | 76 |
| mapbench.md | cxx | release | 20 | 0.2500 | 0.2400–0.2700 | 404242432 | 33624 | 76 |
| mapbench.md | ours | debug | 20 | 0.6950 | 0.6800–0.7100 | 404242432 | 57320 | 109 |
| mapbench.md | ours | release | 20 | 0.2650 | 0.2600–0.2750 | 404226048 | 40760 | 109 |
| mapbench.md | rs | debug | 20 | 1.0800 | 1.0150–1.1400 | 404422656 | 503992 | 62 |
| mapbench.md | rs | release | 20 | 0.2600 | 0.2500–0.2650 | 404422656 | 490568 | 62 |
| mapbench.sm | c | debug | 30 | 0.1000 | 0.1000–0.1050 | 102203392 | 33680 | 75 |
| mapbench.sm | c | release | 30 | 0.0500 | 0.0500–0.0500 | 102203392 | 33608 | 75 |
| mapbench.sm | cxx | debug | 30 | 0.0950 | 0.0900–0.1000 | 102203392 | 33736 | 76 |
| mapbench.sm | cxx | release | 30 | 0.0500 | 0.0500–0.0500 | 102203392 | 33624 | 76 |
| mapbench.sm | ours | debug | 30 | 0.1400 | 0.1300–0.1500 | 102219776 | 57320 | 109 |
| mapbench.sm | ours | release | 30 | 0.0500 | 0.0500–0.0500 | 102203392 | 40760 | 109 |
| mapbench.sm | rs | debug | 30 | 0.2150 | 0.2000–0.2300 | 102400000 | 503992 | 62 |
| mapbench.sm | rs | release | 30 | 0.0500 | 0.0500–0.0500 | 102400000 | 490568 | 62 |
| prodcons.lg | c | debug | 10 | 0.3700 | 0.3600–0.3850 | 17842176 | 34256 | 75 |
| prodcons.lg | c | release | 10 | 0.3450 | 0.3300–0.3650 | 15499264 | 34168 | 75 |
| prodcons.lg | cxx | debug | 10 | 0.3750 | 0.3600–0.3800 | 27607040 | 34296 | 76 |
| prodcons.lg | cxx | release | 10 | 0.3300 | 0.3200–0.3400 | 13926400 | 34184 | 76 |
| prodcons.lg | ours | debug | 10 | 0.6200 | 0.6000–0.6600 | 4997120 | 40632 | 42 |
| prodcons.lg | ours | release | 10 | 0.6450 | 0.6350–0.6700 | 4489216 | 40632 | 42 |
| prodcons.lg | rs | debug | 10 | 0.6800 | 0.6600–0.7900 | 2293760 | 693464 | 24 |
| prodcons.lg | rs | release | 10 | 0.8550 | 0.8350–0.9500 | 8912896 | 575880 | 24 |
| prodcons.md | c | debug | 20 | 0.0900 | 0.0900–0.0900 | 7913472 | 34256 | 75 |
| prodcons.md | c | release | 20 | 0.0800 | 0.0800–0.0800 | 7258112 | 34168 | 75 |
| prodcons.md | cxx | debug | 20 | 0.0900 | 0.0900–0.0900 | 8273920 | 34296 | 76 |
| prodcons.md | cxx | release | 20 | 0.0800 | 0.0800–0.0800 | 10534912 | 34184 | 76 |
| prodcons.md | ours | debug | 20 | 0.1600 | 0.1550–0.1650 | 2768896 | 40632 | 42 |
| prodcons.md | ours | release | 20 | 0.1600 | 0.1600–0.1600 | 6307840 | 40632 | 42 |
| prodcons.md | rs | debug | 20 | 0.2300 | 0.1750–0.2900 | 6668288 | 693464 | 24 |
| prodcons.md | rs | release | 20 | 0.2500 | 0.2300–0.2650 | 9420800 | 575880 | 24 |
| prodcons.sm | c | debug | 30 | 0.0200 | 0.0200–0.0200 | 4833280 | 34256 | 75 |
| prodcons.sm | c | release | 30 | 0.0200 | 0.0200–0.0200 | 3145728 | 34168 | 75 |
| prodcons.sm | cxx | debug | 30 | 0.0200 | 0.0200–0.0200 | 4866048 | 34296 | 76 |
| prodcons.sm | cxx | release | 30 | 0.0200 | 0.0200–0.0200 | 3293184 | 34184 | 76 |
| prodcons.sm | ours | debug | 30 | 0.0400 | 0.0400–0.0400 | 2850816 | 40632 | 42 |
| prodcons.sm | ours | release | 30 | 0.0400 | 0.0400–0.0400 | 2523136 | 40632 | 42 |
| prodcons.sm | rs | debug | 30 | 0.0500 | 0.0500–0.0600 | 3014656 | 693464 | 24 |
| prodcons.sm | rs | release | 30 | 0.0600 | 0.0600–0.0600 | 3899392 | 575880 | 24 |
| psum.lg | c | debug | 10 | 0.1050 | 0.0900–0.1500 | 961544192 | 33840 | 37 |
| psum.lg | c | release | 10 | 0.0450 | 0.0400–0.1050 | 961560576 | 33840 | 37 |
| psum.lg | cxx | debug | 10 | 0.0900 | 0.0850–0.1250 | 961576960 | 34272 | 38 |
| psum.lg | cxx | release | 10 | 0.0400 | 0.0400–0.0400 | 961593344 | 33840 | 38 |
| psum.lg | ours | debug | 10 | 0.1500 | 0.1450–0.1700 | 961560576 | 57312 | 40 |
| psum.lg | ours | release | 10 | 0.0400 | 0.0400–0.0400 | 961576960 | 40808 | 40 |
| psum.lg | rs | debug | 10 | 0.3100 | 0.3000–0.3500 | 961806336 | 586144 | 31 |
| psum.lg | rs | release | 10 | 0.0500 | 0.0400–0.1000 | 961757184 | 524224 | 31 |
| psum.md | c | debug | 20 | 0.0300 | 0.0300–0.0300 | 321552384 | 33840 | 37 |
| psum.md | c | release | 20 | 0.0100 | 0.0100–0.0100 | 321536000 | 33840 | 37 |
| psum.md | cxx | debug | 20 | 0.0300 | 0.0300–0.0350 | 321552384 | 34272 | 38 |
| psum.md | cxx | release | 20 | 0.0100 | 0.0100–0.0100 | 321552384 | 33840 | 38 |
| psum.md | ours | debug | 20 | 0.0500 | 0.0500–0.0500 | 321568768 | 57312 | 40 |
| psum.md | ours | release | 20 | 0.0100 | 0.0100–0.0100 | 321568768 | 40808 | 40 |
| psum.md | rs | debug | 20 | 0.1000 | 0.1000–0.1050 | 321781760 | 586144 | 31 |
| psum.md | rs | release | 20 | 0.0100 | 0.0100–0.0200 | 321748992 | 524224 | 31 |
| psum.sm | c | debug | 30 | 0.0100 | 0.0000–0.0100 | 81543168 | 33840 | 37 |
| psum.sm | c | release | 30 | 0.0000 | 0.0000–0.0000 | 81575936 | 33840 | 37 |
| psum.sm | cxx | debug | 30 | 0.0000 | 0.0000–0.0100 | 81559552 | 34272 | 38 |
| psum.sm | cxx | release | 30 | 0.0000 | 0.0000–0.0000 | 81592320 | 33840 | 38 |
| psum.sm | ours | debug | 30 | 0.0100 | 0.0100–0.0100 | 81575936 | 57312 | 40 |
| psum.sm | ours | release | 30 | 0.0000 | 0.0000–0.0000 | 81559552 | 40808 | 40 |
| psum.sm | rs | debug | 30 | 0.0200 | 0.0200–0.0300 | 81805312 | 586144 | 31 |
| psum.sm | rs | release | 30 | 0.0000 | 0.0000–0.0000 | 81772544 | 524224 | 31 |
| wordfreq.lg | c | debug | 10 | 0.3100 | 0.3000–0.3200 | 51757056 | 33920 | 108 |
| wordfreq.lg | c | release | 10 | 0.1400 | 0.1400–0.1500 | 51757056 | 33800 | 108 |
| wordfreq.lg | cxx | debug | 10 | 0.3150 | 0.3050–0.3250 | 51773440 | 33992 | 115 |
| wordfreq.lg | cxx | release | 10 | 0.1500 | 0.1400–0.1500 | 51773440 | 33816 | 115 |
| wordfreq.lg | ours | debug | 10 | 0.6400 | 0.6200–0.6600 | 118996992 | 77528 | 62 |
| wordfreq.lg | ours | release | 10 | 0.2300 | 0.2300–0.2400 | 118980608 | 61096 | 62 |
| wordfreq.lg | rs | debug | 10 | 0.7000 | 0.6900–0.7200 | 52019200 | 526664 | 79 |
| wordfreq.lg | rs | release | 10 | 0.1400 | 0.1300–0.1400 | 51986432 | 491640 | 79 |
| wordfreq.md | c | debug | 20 | 0.0700 | 0.0700–0.0700 | 14008320 | 33920 | 108 |
| wordfreq.md | c | release | 20 | 0.0300 | 0.0300–0.0300 | 14024704 | 33800 | 108 |
| wordfreq.md | cxx | debug | 20 | 0.0700 | 0.0700–0.0750 | 14024704 | 33992 | 115 |
| wordfreq.md | cxx | release | 20 | 0.0300 | 0.0300–0.0300 | 14008320 | 33816 | 115 |
| wordfreq.md | ours | debug | 20 | 0.1600 | 0.1600–0.1600 | 30883840 | 77528 | 62 |
| wordfreq.md | ours | release | 20 | 0.0600 | 0.0550–0.0600 | 30867456 | 61096 | 62 |
| wordfreq.md | rs | debug | 20 | 0.1800 | 0.1700–0.1800 | 14270464 | 526664 | 79 |
| wordfreq.md | rs | release | 20 | 0.0300 | 0.0300–0.0300 | 14237696 | 491640 | 79 |
| wordfreq.sm | c | debug | 30 | 0.0000 | 0.0000–0.0000 | 2457600 | 33920 | 108 |
| wordfreq.sm | c | release | 30 | 0.0000 | 0.0000–0.0000 | 2457600 | 33800 | 108 |
| wordfreq.sm | cxx | debug | 30 | 0.0000 | 0.0000–0.0000 | 2490368 | 33992 | 115 |
| wordfreq.sm | cxx | release | 30 | 0.0000 | 0.0000–0.0000 | 2473984 | 33816 | 115 |
| wordfreq.sm | ours | debug | 30 | 0.0100 | 0.0100–0.0100 | 4636672 | 77528 | 62 |
| wordfreq.sm | ours | release | 30 | 0.0000 | 0.0000–0.0000 | 4587520 | 61096 | 62 |
| wordfreq.sm | rs | debug | 30 | 0.0100 | 0.0100–0.0100 | 2719744 | 526664 | 79 |
| wordfreq.sm | rs | release | 30 | 0.0000 | 0.0000–0.0000 | 2670592 | 491640 | 79 |

## paired ours − {lang} wall diff (median, 95% CI)

| workload | mode | vs | median diff s | 95% CI |
|---|---|---|---|---|
| psum.lg | release | c | -0.0050 | -0.0650–+0.0000 |
| fcopy.sm | debug | cxx | +0.0000 | +0.0000–+0.0000 |
| fcopy.lg | release | cxx | +0.0000 | -0.0300–+0.0000 |
| wordfreq.md | release | c | +0.0250 | +0.0200–+0.0300 |
| mapbench.lg | debug | c | +0.6200 | +0.5600–+0.9250 |
| psum.md | release | cxx | +0.0000 | +0.0000–+0.0000 |
| jsonparse.md | release | c | -0.0100 | -0.0100–-0.0100 |
| psum.sm | release | rs | +0.0000 | +0.0000–+0.0000 |
| psum.md | release | c | +0.0000 | +0.0000–+0.0000 |
| mapbench.sm | debug | rs | -0.0700 | -0.0750–-0.0600 |
| mapbench.lg | release | rs | +0.0000 | -0.1600–+0.1400 |
| wordfreq.lg | debug | cxx | +0.3200 | +0.3100–+0.3400 |
| mapbench.sm | release | c | +0.0000 | +0.0000–+0.0000 |
| psum.lg | debug | rs | -0.1550 | -0.2000–-0.1300 |
| prodcons.sm | debug | rs | -0.0100 | -0.0200–-0.0100 |
| mapbench.lg | debug | rs | -1.1400 | -1.2400–-0.9900 |
| fcopy.lg | debug | rs | +0.0000 | +0.0000–+0.0400 |
| fcopy.sm | debug | rs | +0.0000 | +0.0000–+0.0000 |
| mapbench.md | release | rs | +0.0050 | +0.0000–+0.0200 |
| jsonparse.md | release | cxx | -0.0100 | -0.0100–-0.0100 |
| jsonparse.sm | debug | rs | +0.0000 | +0.0000–+0.0000 |
| fcopy.md | debug | rs | +0.0000 | -0.0050–+0.0000 |
| mapbench.lg | release | cxx | +0.0700 | +0.0050–+0.1850 |
| psum.lg | release | cxx | +0.0000 | +0.0000–+0.0050 |
| psum.lg | debug | cxx | +0.0600 | +0.0400–+0.0800 |
| wordfreq.sm | release | cxx | +0.0000 | +0.0000–+0.0000 |
| jsonparse.md | debug | cxx | +0.0500 | +0.0500–+0.0500 |
| mapbench.lg | release | c | +0.0150 | -0.1850–+0.1100 |
| fcopy.lg | release | c | +0.0000 | -0.0200–+0.0000 |
| fcopy.lg | debug | c | +0.0000 | -0.0050–+0.0300 |
| fcopy.md | debug | c | +0.0000 | +0.0000–+0.0000 |
| mapbench.md | debug | rs | -0.3650 | -0.4250–-0.3500 |
| mapbench.md | release | cxx | +0.0200 | +0.0050–+0.0200 |
| fcopy.lg | release | rs | +0.0000 | +0.0000–+0.0000 |
| fcopy.md | release | rs | +0.0000 | +0.0000–+0.0000 |
| mapbench.sm | release | cxx | +0.0000 | +0.0000–+0.0000 |
| prodcons.md | release | rs | -0.0950 | -0.1150–-0.0700 |
| prodcons.lg | release | c | +0.3050 | +0.2700–+0.3300 |
| jsonparse.lg | debug | c | +0.1850 | +0.1800–+0.2000 |
| wordfreq.sm | debug | c | +0.0100 | +0.0100–+0.0100 |
| fcopy.md | debug | cxx | +0.0000 | +0.0000–+0.0000 |
| prodcons.lg | debug | rs | -0.0600 | -0.1600–-0.0300 |
| wordfreq.sm | release | rs | +0.0000 | +0.0000–+0.0000 |
| psum.md | debug | c | +0.0200 | +0.0200–+0.0250 |
| jsonparse.lg | release | cxx | -0.0400 | -0.0500–-0.0300 |
| jsonparse.lg | debug | cxx | +0.1850 | +0.1800–+0.2000 |
| prodcons.md | debug | rs | -0.0650 | -0.1200–-0.0200 |
| mapbench.md | release | c | +0.0100 | +0.0000–+0.0200 |
| jsonparse.md | release | rs | +0.0100 | +0.0100–+0.0150 |
| jsonparse.sm | debug | c | +0.0000 | +0.0000–+0.0000 |
| fcopy.sm | release | rs | +0.0000 | +0.0000–+0.0000 |
| prodcons.lg | release | cxx | +0.3200 | +0.3150–+0.3400 |
| psum.md | debug | cxx | +0.0200 | +0.0150–+0.0200 |
| wordfreq.md | release | rs | +0.0300 | +0.0200–+0.0300 |
| wordfreq.lg | debug | rs | -0.0650 | -0.0800–-0.0500 |
| prodcons.sm | debug | cxx | +0.0200 | +0.0200–+0.0200 |
| prodcons.md | debug | c | +0.0700 | +0.0700–+0.0800 |
| psum.lg | debug | c | +0.0600 | +0.0100–+0.0800 |
| jsonparse.lg | debug | rs | +0.2200 | +0.2200–+0.2400 |
| psum.sm | debug | cxx | +0.0100 | +0.0000–+0.0100 |
| wordfreq.lg | release | rs | +0.0950 | +0.0900–+0.1000 |
| prodcons.lg | debug | cxx | +0.2500 | +0.2300–+0.2800 |
| mapbench.md | debug | cxx | +0.2100 | +0.1950–+0.2300 |
| prodcons.sm | debug | c | +0.0200 | +0.0200–+0.0200 |
| jsonparse.md | debug | c | +0.0500 | +0.0400–+0.0500 |
| psum.md | release | rs | +0.0000 | -0.0100–+0.0000 |
| fcopy.sm | debug | c | +0.0000 | +0.0000–+0.0000 |
| wordfreq.md | debug | cxx | +0.0900 | +0.0800–+0.0900 |
| jsonparse.sm | debug | cxx | +0.0000 | +0.0000–+0.0000 |
| prodcons.lg | debug | c | +0.2600 | +0.2250–+0.2850 |
| jsonparse.sm | release | c | +0.0000 | +0.0000–+0.0000 |
| mapbench.sm | release | rs | +0.0000 | +0.0000–+0.0000 |
| fcopy.sm | release | c | +0.0000 | +0.0000–+0.0000 |
| fcopy.md | release | c | +0.0000 | +0.0000–+0.0000 |
| wordfreq.lg | release | c | +0.0900 | +0.0900–+0.0900 |
| jsonparse.sm | release | cxx | +0.0000 | +0.0000–+0.0000 |
| prodcons.md | release | cxx | +0.0800 | +0.0700–+0.0800 |
| psum.sm | debug | c | +0.0000 | +0.0000–+0.0100 |
| wordfreq.sm | release | c | +0.0000 | +0.0000–+0.0000 |
| wordfreq.sm | debug | cxx | +0.0100 | +0.0100–+0.0100 |
| jsonparse.lg | release | c | -0.0400 | -0.0500–-0.0300 |
| mapbench.sm | debug | c | +0.0400 | +0.0400–+0.0450 |
| wordfreq.md | debug | c | +0.0900 | +0.0900–+0.0900 |
| psum.lg | release | rs | -0.0050 | -0.0400–+0.0000 |
| wordfreq.md | debug | rs | -0.0200 | -0.0200–-0.0150 |
| psum.sm | debug | rs | -0.0100 | -0.0200–-0.0100 |
| prodcons.sm | release | c | +0.0200 | +0.0200–+0.0200 |
| prodcons.lg | release | rs | -0.2000 | -0.3050–-0.1700 |
| jsonparse.lg | release | rs | +0.0600 | +0.0550–+0.0700 |
| fcopy.sm | release | cxx | +0.0000 | +0.0000–+0.0000 |
| jsonparse.sm | release | rs | +0.0000 | +0.0000–+0.0000 |
| prodcons.md | debug | cxx | +0.0700 | +0.0650–+0.0800 |
| psum.sm | release | cxx | +0.0000 | +0.0000–+0.0000 |
| prodcons.sm | release | cxx | +0.0200 | +0.0200–+0.0200 |
| fcopy.lg | debug | cxx | +0.0000 | -0.0150–+0.0400 |
| jsonparse.md | debug | rs | +0.0600 | +0.0500–+0.0600 |
| mapbench.lg | debug | cxx | +0.5950 | +0.5700–+0.8750 |
| wordfreq.lg | debug | c | +0.3300 | +0.3200–+0.3450 |
| fcopy.md | release | cxx | +0.0000 | +0.0000–+0.0000 |
| mapbench.md | debug | c | +0.2000 | +0.1900–+0.2100 |
| prodcons.md | release | c | +0.0800 | +0.0700–+0.0800 |
| wordfreq.md | release | cxx | +0.0300 | +0.0200–+0.0300 |
| psum.sm | release | c | +0.0000 | +0.0000–+0.0000 |
| wordfreq.lg | release | cxx | +0.0900 | +0.0800–+0.0900 |
| prodcons.sm | release | rs | -0.0200 | -0.0200–-0.0200 |
| wordfreq.sm | debug | rs | +0.0000 | +0.0000–+0.0000 |
| psum.md | debug | rs | -0.0500 | -0.0600–-0.0500 |
| mapbench.sm | debug | cxx | +0.0400 | +0.0400–+0.0500 |
| httpsrv.lg | debug | rs | +0.0036 | -0.0107–+0.0268 |
| kvsrv.sm | debug | cxx | -0.0003 | -0.0012–+0.0002 |
| kvsrv.lg | debug | rs | +0.0017 | -0.0070–+0.0412 |
| httpsrv.sm | debug | cxx | -0.0000 | -0.0018–+0.0012 |
| httpsrv.md | debug | rs | +0.0004 | -0.0008–+0.0027 |
| kvsrv.sm | debug | c | +0.0000 | -0.0010–+0.0005 |
| httpsrv.md | release | rs | +0.0011 | -0.0012–+0.0021 |
| kvsrv.sm | release | c | +0.0000 | -0.0005–+0.0008 |
| kvsrv.lg | release | rs | +0.0099 | +0.0005–+0.0348 |
| kvsrv.sm | release | cxx | -0.0008 | -0.0020–+0.0001 |
| kvsrv.sm | debug | rs | -0.0005 | -0.0019–-0.0001 |
| httpsrv.md | debug | c | -0.0007 | -0.0053–+0.0012 |
| httpsrv.lg | release | c | -0.0080 | -0.0183–+0.0086 |
| httpsrv.lg | release | rs | -0.0032 | -0.0162–+0.0104 |
| kvsrv.lg | release | c | +0.0027 | -0.0060–+0.0168 |
| kvsrv.md | debug | rs | +0.0002 | -0.0026–+0.0020 |
| kvsrv.md | debug | cxx | -0.0004 | -0.0026–+0.0017 |
| kvsrv.sm | release | rs | +0.0000 | -0.0012–+0.0004 |
| kvsrv.lg | debug | cxx | +0.0013 | -0.0138–+0.0336 |
| httpsrv.lg | debug | cxx | -0.0026 | -0.0081–+0.0326 |
| httpsrv.sm | debug | c | -0.0005 | -0.0033–+0.0009 |
| httpsrv.sm | debug | rs | +0.0003 | -0.0014–+0.0014 |
| httpsrv.lg | release | cxx | +0.0008 | -0.0129–+0.0153 |
| httpsrv.md | debug | cxx | +0.0015 | +0.0005–+0.0036 |
| kvsrv.lg | debug | c | +0.0071 | -0.0065–+0.0498 |
| kvsrv.lg | release | cxx | +0.0004 | -0.0253–+0.0202 |
| httpsrv.md | release | c | +0.0006 | -0.0012–+0.0022 |
| kvsrv.md | debug | c | -0.0007 | -0.0024–+0.0013 |
| httpsrv.md | release | cxx | -0.0024 | -0.0052–+0.0015 |
| httpsrv.lg | debug | c | +0.0076 | -0.0022–+0.0399 |

## invalid cells (excluded above)

| workload | lang | mode | reason | n |
|---|---|---|---|---|
| httpsrv.sm | cxx | release | run-failed | 29 |
| kvsrv.md | cxx | release | run-failed | 19 |
