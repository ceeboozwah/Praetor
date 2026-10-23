# PraetorBench-1.2 — 20261022-201613 (lab-grade)

| workload | lang | mode | n | median s | 95% CI | rss B | size B | loc |
|---|---|---|---|---|---|---|---|---|
| fcopy.lg | c | debug | 10 | 0.0550 | 0.0200–0.0600 | 1425408 | 33792 | 26 |
| fcopy.lg | c | release | 10 | 0.0550 | 0.0200–0.0600 | 1425408 | 33792 | 26 |
| fcopy.lg | cxx | debug | 10 | 0.0200 | 0.0200–0.0600 | 1441792 | 33792 | 27 |
| fcopy.lg | cxx | release | 10 | 0.0250 | 0.0200–0.0650 | 1425408 | 33800 | 27 |
| fcopy.lg | ours | debug | 10 | 0.0300 | 0.0200–0.0600 | 1441792 | 94608 | 39 |
| fcopy.lg | ours | release | 10 | 0.0550 | 0.0200–0.0600 | 1441792 | 78200 | 39 |
| fcopy.lg | rs | debug | 10 | 0.0600 | 0.0300–0.0600 | 1638400 | 521648 | 19 |
| fcopy.lg | rs | release | 10 | 0.0450 | 0.0200–0.0650 | 1605632 | 491952 | 19 |
| fcopy.md | c | debug | 20 | 0.0100 | 0.0100–0.0300 | 1425408 | 33792 | 26 |
| fcopy.md | c | release | 20 | 0.0100 | 0.0100–0.0300 | 1425408 | 33792 | 26 |
| fcopy.md | cxx | debug | 20 | 0.0150 | 0.0100–0.0300 | 1441792 | 33792 | 27 |
| fcopy.md | cxx | release | 20 | 0.0300 | 0.0100–0.0300 | 1441792 | 33800 | 27 |
| fcopy.md | ours | debug | 20 | 0.0200 | 0.0100–0.0300 | 1458176 | 94608 | 39 |
| fcopy.md | ours | release | 20 | 0.0100 | 0.0100–0.0300 | 1441792 | 78200 | 39 |
| fcopy.md | rs | debug | 20 | 0.0100 | 0.0100–0.0200 | 1654784 | 521648 | 19 |
| fcopy.md | rs | release | 20 | 0.0250 | 0.0100–0.0300 | 1622016 | 491952 | 19 |
| fcopy.sm | c | debug | 30 | 0.0000 | 0.0000–0.0000 | 1425408 | 33792 | 26 |
| fcopy.sm | c | release | 30 | 0.0000 | 0.0000–0.0000 | 1425408 | 33792 | 26 |
| fcopy.sm | cxx | debug | 30 | 0.0000 | 0.0000–0.0000 | 1441792 | 33792 | 27 |
| fcopy.sm | cxx | release | 30 | 0.0000 | 0.0000–0.0000 | 1458176 | 33800 | 27 |
| fcopy.sm | ours | debug | 30 | 0.0000 | 0.0000–0.0000 | 1458176 | 94608 | 39 |
| fcopy.sm | ours | release | 30 | 0.0000 | 0.0000–0.0000 | 1425408 | 78200 | 39 |
| fcopy.sm | rs | debug | 30 | 0.0000 | 0.0000–0.0000 | 1654784 | 521648 | 19 |
| fcopy.sm | rs | release | 30 | 0.0000 | 0.0000–0.0000 | 1605632 | 491952 | 19 |
| httpsrv.lg | c | debug | 10 | 0.1820 | 0.1781–0.1988 | 1392640 | 34080 | 35 |
| httpsrv.lg | c | release | 10 | 0.1927 | 0.1820–0.2226 | 1376256 | 34008 | 35 |
| httpsrv.lg | cxx | debug | 10 | 0.1944 | 0.1743–0.2124 | 1409024 | 34088 | 37 |
| httpsrv.lg | cxx | release | 10 | 0.1882 | 0.1767–0.2087 | 1392640 | 34008 | 37 |
| httpsrv.lg | ours | debug | 10 | 0.1830 | 0.1766–0.2118 | 1392640 | 96024 | 39 |
| httpsrv.lg | ours | release | 10 | 0.1797 | 0.1746–0.2734 | 1376256 | 79560 | 39 |
| httpsrv.lg | rs | debug | 10 | 0.1969 | 0.1797–0.2156 | 1589248 | 548496 | 25 |
| httpsrv.lg | rs | release | 10 | 0.1891 | 0.1833–0.1982 | 1589248 | 498152 | 25 |
| httpsrv.md | c | debug | 20 | 0.0731 | 0.0700–0.0783 | 1376256 | 34080 | 35 |
| httpsrv.md | c | release | 20 | 0.0733 | 0.0716–0.0812 | 1392640 | 34008 | 35 |
| httpsrv.md | cxx | debug | 20 | 0.0732 | 0.0704–0.0844 | 1392640 | 34088 | 37 |
| httpsrv.md | cxx | release | 20 | 0.0716 | 0.0701–0.0740 | 1409024 | 34008 | 37 |
| httpsrv.md | ours | debug | 20 | 0.0711 | 0.0701–0.0725 | 1409024 | 96024 | 39 |
| httpsrv.md | ours | release | 20 | 0.0727 | 0.0712–0.0755 | 1392640 | 79560 | 39 |
| httpsrv.md | rs | debug | 20 | 0.0732 | 0.0709–0.0758 | 1589248 | 548496 | 25 |
| httpsrv.md | rs | release | 20 | 0.0707 | 0.0693–0.0723 | 1572864 | 498152 | 25 |
| httpsrv.sm | c | debug | 30 | 0.0239 | 0.0225–0.0244 | 1376256 | 34080 | 35 |
| httpsrv.sm | c | release | 30 | 0.0234 | 0.0224–0.0243 | 1392640 | 34008 | 35 |
| httpsrv.sm | cxx | debug | 30 | 0.0243 | 0.0228–0.0253 | 1409024 | 34088 | 37 |
| httpsrv.sm | cxx | release | 30 | 0.0233 | 0.0219–0.0249 | 1392640 | 34008 | 37 |
| httpsrv.sm | ours | debug | 30 | 0.0231 | 0.0225–0.0240 | 1392640 | 96024 | 39 |
| httpsrv.sm | ours | release | 30 | 0.0234 | 0.0227–0.0247 | 1392640 | 79560 | 39 |
| httpsrv.sm | rs | debug | 30 | 0.0237 | 0.0225–0.0261 | 1605632 | 548496 | 25 |
| httpsrv.sm | rs | release | 30 | 0.0235 | 0.0225–0.0258 | 1572864 | 498152 | 25 |
| jsonparse.lg | c | debug | 10 | 0.3000 | 0.3000–0.3100 | 196575232 | 34184 | 165 |
| jsonparse.lg | c | release | 10 | 0.2300 | 0.2200–0.2300 | 196640768 | 33928 | 165 |
| jsonparse.lg | cxx | debug | 10 | 0.3050 | 0.3000–0.3100 | 196608000 | 34280 | 167 |
| jsonparse.lg | cxx | release | 10 | 0.2300 | 0.2200–0.2400 | 196657152 | 33960 | 167 |
| jsonparse.lg | ours | debug | 10 | 0.5000 | 0.4850–0.5100 | 326844416 | 112584 | 273 |
| jsonparse.lg | ours | release | 10 | 0.1900 | 0.1800–0.1950 | 325648384 | 96152 | 273 |
| jsonparse.lg | rs | debug | 10 | 0.2700 | 0.2700–0.2750 | 202113024 | 526280 | 120 |
| jsonparse.lg | rs | release | 10 | 0.1200 | 0.1200–0.1200 | 202047488 | 492200 | 120 |
| jsonparse.md | c | debug | 20 | 0.0800 | 0.0700–0.0800 | 50561024 | 34184 | 165 |
| jsonparse.md | c | release | 20 | 0.0500 | 0.0500–0.0500 | 50626560 | 33928 | 165 |
| jsonparse.md | cxx | debug | 20 | 0.0800 | 0.0700–0.0800 | 50544640 | 34280 | 167 |
| jsonparse.md | cxx | release | 20 | 0.0600 | 0.0500–0.0600 | 50642944 | 33960 | 167 |
| jsonparse.md | ours | debug | 20 | 0.1200 | 0.1200–0.1250 | 82673664 | 112584 | 273 |
| jsonparse.md | ours | release | 20 | 0.0500 | 0.0500–0.0500 | 82640896 | 96152 | 273 |
| jsonparse.md | rs | debug | 20 | 0.0700 | 0.0600–0.0700 | 51429376 | 526280 | 120 |
| jsonparse.md | rs | release | 20 | 0.0300 | 0.0300–0.0300 | 51380224 | 492200 | 120 |
| jsonparse.sm | c | debug | 30 | 0.0000 | 0.0000–0.0000 | 3637248 | 34184 | 165 |
| jsonparse.sm | c | release | 30 | 0.0000 | 0.0000–0.0000 | 3719168 | 33928 | 165 |
| jsonparse.sm | cxx | debug | 30 | 0.0000 | 0.0000–0.0000 | 3670016 | 34280 | 167 |
| jsonparse.sm | cxx | release | 30 | 0.0000 | 0.0000–0.0000 | 3719168 | 33960 | 167 |
| jsonparse.sm | ours | debug | 30 | 0.0100 | 0.0000–0.0100 | 5144576 | 112584 | 273 |
| jsonparse.sm | ours | release | 30 | 0.0000 | 0.0000–0.0000 | 5111808 | 96152 | 273 |
| jsonparse.sm | rs | debug | 30 | 0.0000 | 0.0000–0.0000 | 3899392 | 526280 | 120 |
| jsonparse.sm | rs | release | 30 | 0.0000 | 0.0000–0.0000 | 3833856 | 492200 | 120 |
| kvsrv.lg | c | debug | 10 | 0.1922 | 0.1768–0.2247 | 1376256 | 34448 | 127 |
| kvsrv.lg | c | release | 10 | 0.1808 | 0.1776–0.1844 | 1376256 | 34288 | 127 |
| kvsrv.lg | cxx | debug | 10 | 0.1870 | 0.1793–0.2207 | 1392640 | 34480 | 134 |
| kvsrv.lg | cxx | release | 10 | 0.2044 | 0.1849–0.2209 | 1409024 | 34312 | 134 |
| kvsrv.lg | ours | debug | 10 | 0.1914 | 0.1820–0.2165 | 1441792 | 113312 | 142 |
| kvsrv.lg | ours | release | 10 | 0.1972 | 0.1863–0.2099 | 1409024 | 97176 | 142 |
| kvsrv.lg | rs | debug | 10 | 0.1878 | 0.1757–0.1974 | 1687552 | 559168 | 95 |
| kvsrv.lg | rs | release | 10 | 0.1843 | 0.1766–0.2033 | 1638400 | 515552 | 95 |
| kvsrv.md | c | debug | 20 | 0.0712 | 0.0703–0.0725 | 1392640 | 34448 | 127 |
| kvsrv.md | c | release | 20 | 0.0736 | 0.0714–0.0770 | 1376256 | 34288 | 127 |
| kvsrv.md | cxx | debug | 20 | 0.0711 | 0.0700–0.0734 | 1392640 | 34480 | 134 |
| kvsrv.md | cxx | release | 20 | 0.0722 | 0.0697–0.0729 | 1392640 | 34312 | 134 |
| kvsrv.md | ours | debug | 20 | 0.0728 | 0.0706–0.0736 | 1441792 | 113312 | 142 |
| kvsrv.md | ours | release | 20 | 0.0726 | 0.0721–0.0785 | 1409024 | 97176 | 142 |
| kvsrv.md | rs | debug | 20 | 0.0727 | 0.0702–0.0793 | 1687552 | 559168 | 95 |
| kvsrv.md | rs | release | 20 | 0.0742 | 0.0723–0.0805 | 1638400 | 515552 | 95 |
| kvsrv.sm | c | debug | 30 | 0.0227 | 0.0225–0.0238 | 1376256 | 34448 | 127 |
| kvsrv.sm | c | release | 30 | 0.0230 | 0.0222–0.0242 | 1376256 | 34288 | 127 |
| kvsrv.sm | cxx | debug | 30 | 0.0234 | 0.0225–0.0244 | 1409024 | 34480 | 134 |
| kvsrv.sm | cxx | release | 30 | 0.0234 | 0.0228–0.0242 | 1392640 | 34312 | 134 |
| kvsrv.sm | ours | debug | 30 | 0.0236 | 0.0227–0.0250 | 1458176 | 113312 | 142 |
| kvsrv.sm | ours | release | 30 | 0.0231 | 0.0222–0.0240 | 1409024 | 97176 | 142 |
| kvsrv.sm | rs | debug | 30 | 0.0231 | 0.0225–0.0248 | 1687552 | 559168 | 95 |
| kvsrv.sm | rs | release | 30 | 0.0237 | 0.0227–0.0253 | 1638400 | 515552 | 95 |
| mapbench.lg | c | debug | 10 | 1.8150 | 1.7250–1.9700 | 1612201984 | 33680 | 75 |
| mapbench.lg | c | release | 10 | 1.1300 | 1.0300–1.2000 | 1612201984 | 33608 | 75 |
| mapbench.lg | cxx | debug | 10 | 1.8650 | 1.7550–2.0000 | 1612234752 | 33736 | 76 |
| mapbench.lg | cxx | release | 10 | 1.1000 | 1.0000–1.1700 | 1612218368 | 33624 | 76 |
| mapbench.lg | ours | debug | 10 | 2.5550 | 2.3100–2.6800 | 1612234752 | 74840 | 109 |
| mapbench.lg | ours | release | 10 | 1.1200 | 1.0700–1.2800 | 1612234752 | 74792 | 109 |
| mapbench.lg | rs | debug | 10 | 3.7350 | 3.5800–3.8500 | 1612414976 | 503992 | 62 |
| mapbench.lg | rs | release | 10 | 1.1800 | 1.0800–1.2350 | 1612414976 | 490568 | 62 |
| mapbench.md | c | debug | 20 | 0.4950 | 0.4700–0.5300 | 404209664 | 33680 | 75 |
| mapbench.md | c | release | 20 | 0.2600 | 0.2400–0.2700 | 404209664 | 33608 | 75 |
| mapbench.md | cxx | debug | 20 | 0.5050 | 0.4850–0.5400 | 404226048 | 33736 | 76 |
| mapbench.md | cxx | release | 20 | 0.2600 | 0.2500–0.2800 | 404242432 | 33624 | 76 |
| mapbench.md | ours | debug | 20 | 0.7250 | 0.6750–0.7550 | 404242432 | 74840 | 109 |
| mapbench.md | ours | release | 20 | 0.2750 | 0.2600–0.3000 | 404226048 | 74792 | 109 |
| mapbench.md | rs | debug | 20 | 1.0650 | 1.0150–1.1300 | 404422656 | 503992 | 62 |
| mapbench.md | rs | release | 20 | 0.2750 | 0.2600–0.3000 | 404422656 | 490568 | 62 |
| mapbench.sm | c | debug | 30 | 0.1000 | 0.0950–0.1100 | 102203392 | 33680 | 75 |
| mapbench.sm | c | release | 30 | 0.0500 | 0.0500–0.0550 | 102203392 | 33608 | 75 |
| mapbench.sm | cxx | debug | 30 | 0.1100 | 0.1000–0.1100 | 102219776 | 33736 | 76 |
| mapbench.sm | cxx | release | 30 | 0.0500 | 0.0500–0.0500 | 102203392 | 33624 | 76 |
| mapbench.sm | ours | debug | 30 | 0.1600 | 0.1500–0.1600 | 102236160 | 74840 | 109 |
| mapbench.sm | ours | release | 30 | 0.0500 | 0.0500–0.0600 | 102203392 | 74792 | 109 |
| mapbench.sm | rs | debug | 30 | 0.2400 | 0.2300–0.2400 | 102400000 | 503992 | 62 |
| mapbench.sm | rs | release | 30 | 0.0500 | 0.0500–0.0600 | 102400000 | 490568 | 62 |
| prodcons.lg | c | debug | 10 | 0.3800 | 0.3700–0.3950 | 16318464 | 34256 | 75 |
| prodcons.lg | c | release | 10 | 0.3500 | 0.3350–0.3500 | 10190848 | 34168 | 75 |
| prodcons.lg | cxx | debug | 10 | 0.3700 | 0.3550–0.4100 | 38453248 | 34296 | 76 |
| prodcons.lg | cxx | release | 10 | 0.3500 | 0.3300–0.3800 | 13205504 | 34184 | 76 |
| prodcons.lg | ours | debug | 10 | 0.6750 | 0.6600–0.7050 | 82493440 | 74664 | 42 |
| prodcons.lg | ours | release | 10 | 0.6700 | 0.6450–0.6950 | 78725120 | 74664 | 42 |
| prodcons.lg | rs | debug | 10 | 0.7200 | 0.6800–1.1100 | 2277376 | 693464 | 24 |
| prodcons.lg | rs | release | 10 | 0.9300 | 0.8700–1.0300 | 2113536 | 575880 | 24 |
| prodcons.md | c | debug | 20 | 0.1000 | 0.0900–0.1000 | 4489216 | 34256 | 75 |
| prodcons.md | c | release | 20 | 0.0900 | 0.0800–0.0900 | 5455872 | 34168 | 75 |
| prodcons.md | cxx | debug | 20 | 0.1000 | 0.0900–0.1000 | 8519680 | 34296 | 76 |
| prodcons.md | cxx | release | 20 | 0.0800 | 0.0800–0.0900 | 12255232 | 34184 | 76 |
| prodcons.md | ours | debug | 20 | 0.1600 | 0.1600–0.1700 | 50003968 | 74664 | 42 |
| prodcons.md | ours | release | 20 | 0.1800 | 0.1700–0.1800 | 18530304 | 74664 | 42 |
| prodcons.md | rs | debug | 20 | 0.1950 | 0.1800–0.2900 | 2326528 | 693464 | 24 |
| prodcons.md | rs | release | 20 | 0.2700 | 0.2350–0.2800 | 2129920 | 575880 | 24 |
| prodcons.sm | c | debug | 30 | 0.0200 | 0.0200–0.0200 | 4571136 | 34256 | 75 |
| prodcons.sm | c | release | 30 | 0.0200 | 0.0200–0.0200 | 3719168 | 34168 | 75 |
| prodcons.sm | cxx | debug | 30 | 0.0200 | 0.0200–0.0200 | 3801088 | 34296 | 76 |
| prodcons.sm | cxx | release | 30 | 0.0200 | 0.0200–0.0200 | 3227648 | 34184 | 76 |
| prodcons.sm | ours | debug | 30 | 0.0400 | 0.0400–0.0400 | 7340032 | 74664 | 42 |
| prodcons.sm | ours | release | 30 | 0.0400 | 0.0400–0.0400 | 7274496 | 74664 | 42 |
| prodcons.sm | rs | debug | 30 | 0.0500 | 0.0500–0.0650 | 2064384 | 693464 | 24 |
| prodcons.sm | rs | release | 30 | 0.0600 | 0.0600–0.0600 | 2310144 | 575880 | 24 |
| prodcons_bchan_c1.lg | c | debug | 10 | 28.3250 | 26.2400–29.0200 | 1425408 | 34272 | 91 |
| prodcons_bchan_c1.lg | c | release | 10 | 27.1500 | 25.5950–28.8300 | 1409024 | 34176 | 91 |
| prodcons_bchan_c1.lg | cxx | debug | 10 | 29.5900 | 28.5000–31.1450 | 1474560 | 49680 | 72 |
| prodcons_bchan_c1.lg | cxx | release | 10 | 26.4300 | 25.1000–28.1900 | 1474560 | 36544 | 72 |
| prodcons_bchan_c1.lg | ours | debug | 10 | 28.1350 | 27.1200–29.3600 | 1507328 | 74672 | 58 |
| prodcons_bchan_c1.lg | ours | release | 10 | 29.0850 | 28.6700–29.9800 | 1490944 | 74672 | 58 |
| prodcons_bchan_c1.lg | rs | debug | 10 | 29.4250 | 28.8900–314.6100 | 1802240 | 706912 | 27 |
| prodcons_bchan_c1.lg | rs | release | 10 | 26.0000 | 24.7800–26.6700 | 1720320 | 580224 | 27 |
| prodcons_bchan_c1.md | c | debug | 20 | 6.9750 | 6.7550–7.2150 | 1458176 | 34272 | 91 |
| prodcons_bchan_c1.md | c | release | 20 | 6.8650 | 6.4250–7.0500 | 1425408 | 34176 | 91 |
| prodcons_bchan_c1.md | cxx | debug | 20 | 7.2850 | 6.8650–7.6350 | 1441792 | 49680 | 72 |
| prodcons_bchan_c1.md | cxx | release | 20 | 6.5850 | 6.1400–6.8500 | 1458176 | 36544 | 72 |
| prodcons_bchan_c1.md | ours | debug | 20 | 7.0350 | 6.7300–7.2750 | 1490944 | 74672 | 58 |
| prodcons_bchan_c1.md | ours | release | 20 | 7.0850 | 6.7750–7.3750 | 1507328 | 74672 | 58 |
| prodcons_bchan_c1.md | rs | debug | 20 | 7.1650 | 6.8700–7.4150 | 1785856 | 706912 | 27 |
| prodcons_bchan_c1.md | rs | release | 20 | 6.6150 | 6.3000–7.0300 | 1736704 | 580224 | 27 |
| prodcons_bchan_c1.sm | c | debug | 30 | 1.6700 | 1.6000–1.7550 | 1425408 | 34272 | 91 |
| prodcons_bchan_c1.sm | c | release | 30 | 1.6300 | 1.5250–1.7150 | 1441792 | 34176 | 91 |
| prodcons_bchan_c1.sm | cxx | debug | 30 | 1.8300 | 1.7449–1.9750 | 1425408 | 49680 | 72 |
| prodcons_bchan_c1.sm | cxx | release | 30 | 1.7000 | 1.5846–1.7500 | 1441792 | 36544 | 72 |
| prodcons_bchan_c1.sm | ours | debug | 30 | 1.7200 | 1.6349–1.8150 | 1490944 | 74672 | 58 |
| prodcons_bchan_c1.sm | ours | release | 30 | 1.7300 | 1.5900–1.7500 | 1458176 | 74672 | 58 |
| prodcons_bchan_c1.sm | rs | debug | 30 | 1.8250 | 1.7700–1.8600 | 1785856 | 706912 | 27 |
| prodcons_bchan_c1.sm | rs | release | 30 | 1.6350 | 1.6000–1.6950 | 1736704 | 580224 | 27 |
| prodcons_bchan_c16.lg | c | debug | 10 | 2.2750 | 2.1700–2.3400 | 1409024 | 34272 | 91 |
| prodcons_bchan_c16.lg | c | release | 10 | 2.2950 | 2.2200–2.3700 | 1409024 | 34176 | 91 |
| prodcons_bchan_c16.lg | cxx | debug | 10 | 2.0050 | 1.9650–2.0350 | 1425408 | 49680 | 72 |
| prodcons_bchan_c16.lg | cxx | release | 10 | 2.0450 | 1.9700–2.1000 | 1441792 | 36544 | 72 |
| prodcons_bchan_c16.lg | ours | debug | 10 | 1.9000 | 1.8700–1.9300 | 1490944 | 74672 | 58 |
| prodcons_bchan_c16.lg | ours | release | 10 | 1.9100 | 1.8650–1.9700 | 1474560 | 74672 | 58 |
| prodcons_bchan_c16.lg | rs | debug | 10 | 2.1400 | 2.0900–2.1950 | 1916928 | 706912 | 27 |
| prodcons_bchan_c16.lg | rs | release | 10 | 1.9800 | 1.9500–2.0100 | 1851392 | 580224 | 27 |
| prodcons_bchan_c16.md | c | debug | 20 | 0.5600 | 0.5400–0.5800 | 1409024 | 34272 | 91 |
| prodcons_bchan_c16.md | c | release | 20 | 0.5300 | 0.4900–0.5400 | 1392640 | 34176 | 91 |
| prodcons_bchan_c16.md | cxx | debug | 20 | 0.4900 | 0.4650–0.5000 | 1425408 | 49680 | 72 |
| prodcons_bchan_c16.md | cxx | release | 20 | 0.5000 | 0.4800–0.5150 | 1458176 | 36544 | 72 |
| prodcons_bchan_c16.md | ours | debug | 20 | 0.4800 | 0.4600–0.4900 | 1474560 | 74672 | 58 |
| prodcons_bchan_c16.md | ours | release | 20 | 0.4900 | 0.4700–0.5000 | 1458176 | 74672 | 58 |
| prodcons_bchan_c16.md | rs | debug | 20 | 0.5000 | 0.4750–0.5050 | 1900544 | 706912 | 27 |
| prodcons_bchan_c16.md | rs | release | 20 | 0.4850 | 0.4700–0.4900 | 1720320 | 580224 | 27 |
| prodcons_bchan_c16.sm | c | debug | 30 | 0.1400 | 0.1350–0.1450 | 1409024 | 34272 | 91 |
| prodcons_bchan_c16.sm | c | release | 30 | 0.1400 | 0.1400–0.1400 | 1409024 | 34176 | 91 |
| prodcons_bchan_c16.sm | cxx | debug | 30 | 0.1200 | 0.1200–0.1300 | 1441792 | 49680 | 72 |
| prodcons_bchan_c16.sm | cxx | release | 30 | 0.1200 | 0.1200–0.1300 | 1425408 | 36544 | 72 |
| prodcons_bchan_c16.sm | ours | debug | 30 | 0.1200 | 0.1100–0.1200 | 1458176 | 74672 | 58 |
| prodcons_bchan_c16.sm | ours | release | 30 | 0.1200 | 0.1200–0.1200 | 1441792 | 74672 | 58 |
| prodcons_bchan_c16.sm | rs | debug | 30 | 0.1300 | 0.1200–0.1300 | 1884160 | 706912 | 27 |
| prodcons_bchan_c16.sm | rs | release | 30 | 0.1200 | 0.1200–0.1200 | 1720320 | 580224 | 27 |
| prodcons_bchan_c256.lg | c | debug | 10 | 0.6750 | 0.6500–0.6950 | 1409024 | 34272 | 91 |
| prodcons_bchan_c256.lg | c | release | 10 | 0.6200 | 0.5600–0.6550 | 1425408 | 34176 | 91 |
| prodcons_bchan_c256.lg | cxx | debug | 10 | 0.8800 | 0.8700–0.9150 | 1425408 | 49680 | 72 |
| prodcons_bchan_c256.lg | cxx | release | 10 | 0.6900 | 0.6750–0.7050 | 1458176 | 36544 | 72 |
| prodcons_bchan_c256.lg | ours | debug | 10 | 1.3050 | 1.2850–1.3500 | 1490944 | 74672 | 58 |
| prodcons_bchan_c256.lg | ours | release | 10 | 1.2350 | 1.2100–1.2750 | 1474560 | 74672 | 58 |
| prodcons_bchan_c256.lg | rs | debug | 10 | 1.4500 | 1.4300–1.5000 | 1916928 | 706912 | 27 |
| prodcons_bchan_c256.lg | rs | release | 10 | 1.4400 | 1.4000–1.4800 | 1851392 | 580224 | 27 |
| prodcons_bchan_c256.md | c | debug | 20 | 0.1600 | 0.1600–0.1750 | 1409024 | 34272 | 91 |
| prodcons_bchan_c256.md | c | release | 20 | 0.1550 | 0.1400–0.1600 | 1409024 | 34176 | 91 |
| prodcons_bchan_c256.md | cxx | debug | 20 | 0.2200 | 0.2200–0.2300 | 1458176 | 49680 | 72 |
| prodcons_bchan_c256.md | cxx | release | 20 | 0.1700 | 0.1700–0.1750 | 1441792 | 36544 | 72 |
| prodcons_bchan_c256.md | ours | debug | 20 | 0.3450 | 0.3400–0.3650 | 1507328 | 74672 | 58 |
| prodcons_bchan_c256.md | ours | release | 20 | 0.2900 | 0.2800–0.3000 | 1441792 | 74672 | 58 |
| prodcons_bchan_c256.md | rs | debug | 20 | 0.3650 | 0.3600–0.3800 | 1900544 | 706912 | 27 |
| prodcons_bchan_c256.md | rs | release | 20 | 0.3500 | 0.3400–0.3600 | 1835008 | 580224 | 27 |
| prodcons_bchan_c256.sm | c | debug | 30 | 0.0400 | 0.0400–0.0400 | 1425408 | 34272 | 91 |
| prodcons_bchan_c256.sm | c | release | 30 | 0.0300 | 0.0300–0.0400 | 1425408 | 34176 | 91 |
| prodcons_bchan_c256.sm | cxx | debug | 30 | 0.0500 | 0.0500–0.0500 | 1441792 | 49680 | 72 |
| prodcons_bchan_c256.sm | cxx | release | 30 | 0.0400 | 0.0400–0.0400 | 1425408 | 36544 | 72 |
| prodcons_bchan_c256.sm | ours | debug | 30 | 0.0800 | 0.0800–0.0800 | 1458176 | 74672 | 58 |
| prodcons_bchan_c256.sm | ours | release | 30 | 0.0700 | 0.0700–0.0750 | 1458176 | 74672 | 58 |
| prodcons_bchan_c256.sm | rs | debug | 30 | 0.0900 | 0.0900–0.0900 | 1916928 | 706912 | 27 |
| prodcons_bchan_c256.sm | rs | release | 30 | 0.0800 | 0.0600–0.0800 | 1851392 | 580224 | 27 |
| prodcons_bchan_c4096.lg | c | debug | 10 | 0.4750 | 0.4550–0.5000 | 1540096 | 34272 | 91 |
| prodcons_bchan_c4096.lg | c | release | 10 | 0.4200 | 0.4100–0.4600 | 1540096 | 34176 | 91 |
| prodcons_bchan_c4096.lg | cxx | debug | 10 | 0.6300 | 0.6200–0.6450 | 1540096 | 49680 | 72 |
| prodcons_bchan_c4096.lg | cxx | release | 10 | 0.4800 | 0.4700–0.5000 | 1540096 | 36544 | 72 |
| prodcons_bchan_c4096.lg | ours | debug | 10 | 0.9000 | 0.8800–0.9300 | 1638400 | 74672 | 58 |
| prodcons_bchan_c4096.lg | ours | release | 10 | 0.8400 | 0.8100–0.8900 | 1638400 | 74680 | 58 |
| prodcons_bchan_c4096.lg | rs | debug | 10 | 1.1550 | 1.1000–1.3850 | 2015232 | 706912 | 27 |
| prodcons_bchan_c4096.lg | rs | release | 10 | 1.4500 | 1.1300–1.4850 | 1966080 | 580224 | 27 |
| prodcons_bchan_c4096.md | c | debug | 20 | 0.1200 | 0.1100–0.1200 | 1523712 | 34272 | 91 |
| prodcons_bchan_c4096.md | c | release | 20 | 0.1000 | 0.1000–0.1100 | 1523712 | 34176 | 91 |
| prodcons_bchan_c4096.md | cxx | debug | 20 | 0.1600 | 0.1550–0.1700 | 1572864 | 49680 | 72 |
| prodcons_bchan_c4096.md | cxx | release | 20 | 0.1250 | 0.1200–0.1300 | 1556480 | 36544 | 72 |
| prodcons_bchan_c4096.md | ours | debug | 20 | 0.2100 | 0.1900–0.2350 | 1622016 | 74672 | 58 |
| prodcons_bchan_c4096.md | ours | release | 20 | 0.2000 | 0.2000–0.2150 | 1605632 | 74680 | 58 |
| prodcons_bchan_c4096.md | rs | debug | 20 | 0.3200 | 0.2800–0.3400 | 2015232 | 706912 | 27 |
| prodcons_bchan_c4096.md | rs | release | 20 | 0.3150 | 0.2800–0.3550 | 1933312 | 580224 | 27 |
| prodcons_bchan_c4096.sm | c | debug | 30 | 0.0300 | 0.0300–0.0300 | 1540096 | 34272 | 91 |
| prodcons_bchan_c4096.sm | c | release | 30 | 0.0200 | 0.0200–0.0300 | 1523712 | 34176 | 91 |
| prodcons_bchan_c4096.sm | cxx | debug | 30 | 0.0400 | 0.0400–0.0400 | 1540096 | 49680 | 72 |
| prodcons_bchan_c4096.sm | cxx | release | 30 | 0.0300 | 0.0300–0.0300 | 1540096 | 36544 | 72 |
| prodcons_bchan_c4096.sm | ours | debug | 30 | 0.0600 | 0.0500–0.0600 | 1638400 | 74672 | 58 |
| prodcons_bchan_c4096.sm | ours | release | 30 | 0.0500 | 0.0500–0.0600 | 1622016 | 74680 | 58 |
| prodcons_bchan_c4096.sm | rs | debug | 30 | 0.0800 | 0.0700–0.0900 | 2015232 | 706912 | 27 |
| prodcons_bchan_c4096.sm | rs | release | 30 | 0.0700 | 0.0700–0.0900 | 1949696 | 580224 | 27 |
| psum.lg | c | debug | 10 | 0.1600 | 0.1000–0.1900 | 961560576 | 33840 | 37 |
| psum.lg | c | release | 10 | 0.0600 | 0.0400–0.1400 | 961544192 | 33840 | 37 |
| psum.lg | cxx | debug | 10 | 0.0950 | 0.0800–0.1050 | 961560576 | 34272 | 38 |
| psum.lg | cxx | release | 10 | 0.0500 | 0.0400–0.1300 | 961609728 | 33840 | 38 |
| psum.lg | ours | debug | 10 | 0.1700 | 0.1400–0.2200 | 961609728 | 74832 | 40 |
| psum.lg | ours | release | 10 | 0.1100 | 0.0400–0.1550 | 961593344 | 74840 | 40 |
| psum.lg | rs | debug | 10 | 0.3150 | 0.3000–0.3600 | 961789952 | 586144 | 31 |
| psum.lg | rs | release | 10 | 0.1200 | 0.0500–0.1300 | 961757184 | 524224 | 31 |
| psum.md | c | debug | 20 | 0.0300 | 0.0300–0.0350 | 321552384 | 33840 | 37 |
| psum.md | c | release | 20 | 0.0100 | 0.0100–0.0250 | 321536000 | 33840 | 37 |
| psum.md | cxx | debug | 20 | 0.0300 | 0.0300–0.0400 | 321568768 | 34272 | 38 |
| psum.md | cxx | release | 20 | 0.0100 | 0.0100–0.0200 | 321552384 | 33840 | 38 |
| psum.md | ours | debug | 20 | 0.0500 | 0.0500–0.0500 | 321585152 | 74832 | 40 |
| psum.md | ours | release | 20 | 0.0200 | 0.0100–0.0400 | 321585152 | 74840 | 40 |
| psum.md | rs | debug | 20 | 0.1000 | 0.1000–0.1100 | 321781760 | 586144 | 31 |
| psum.md | rs | release | 20 | 0.0200 | 0.0100–0.0200 | 321748992 | 524224 | 31 |
| psum.sm | c | debug | 30 | 0.0100 | 0.0100–0.0100 | 81543168 | 33840 | 37 |
| psum.sm | c | release | 30 | 0.0000 | 0.0000–0.0000 | 81592320 | 33840 | 37 |
| psum.sm | cxx | debug | 30 | 0.0100 | 0.0100–0.0100 | 81559552 | 34272 | 38 |
| psum.sm | cxx | release | 30 | 0.0000 | 0.0000–0.0000 | 81608704 | 33840 | 38 |
| psum.sm | ours | debug | 30 | 0.0100 | 0.0100–0.0100 | 81592320 | 74832 | 40 |
| psum.sm | ours | release | 30 | 0.0000 | 0.0000–0.0000 | 81592320 | 74840 | 40 |
| psum.sm | rs | debug | 30 | 0.0200 | 0.0200–0.0300 | 81805312 | 586144 | 31 |
| psum.sm | rs | release | 30 | 0.0000 | 0.0000–0.0000 | 81788928 | 524224 | 31 |
| wordfreq.lg | c | debug | 10 | 0.3200 | 0.3200–0.3200 | 51740672 | 33920 | 108 |
| wordfreq.lg | c | release | 10 | 0.1500 | 0.1500–0.1500 | 51757056 | 33800 | 108 |
| wordfreq.lg | cxx | debug | 10 | 0.3200 | 0.3200–0.3200 | 51757056 | 33992 | 115 |
| wordfreq.lg | cxx | release | 10 | 0.1500 | 0.1500–0.1600 | 51773440 | 33816 | 115 |
| wordfreq.lg | ours | debug | 10 | 0.6400 | 0.6400–0.6500 | 118996992 | 95016 | 62 |
| wordfreq.lg | ours | release | 10 | 0.2450 | 0.2400–0.2550 | 118996992 | 78568 | 62 |
| wordfreq.lg | rs | debug | 10 | 0.7200 | 0.7100–0.7500 | 52019200 | 526664 | 79 |
| wordfreq.lg | rs | release | 10 | 0.1400 | 0.1400–0.1500 | 51970048 | 491640 | 79 |
| wordfreq.md | c | debug | 20 | 0.0700 | 0.0700–0.0800 | 14008320 | 33920 | 108 |
| wordfreq.md | c | release | 20 | 0.0400 | 0.0300–0.0400 | 14008320 | 33800 | 108 |
| wordfreq.md | cxx | debug | 20 | 0.0800 | 0.0700–0.0800 | 14024704 | 33992 | 115 |
| wordfreq.md | cxx | release | 20 | 0.0400 | 0.0350–0.0400 | 14008320 | 33816 | 115 |
| wordfreq.md | ours | debug | 20 | 0.1600 | 0.1600–0.1600 | 30883840 | 95016 | 62 |
| wordfreq.md | ours | release | 20 | 0.0600 | 0.0600–0.0600 | 30867456 | 78568 | 62 |
| wordfreq.md | rs | debug | 20 | 0.1800 | 0.1800–0.1800 | 14270464 | 526664 | 79 |
| wordfreq.md | rs | release | 20 | 0.0350 | 0.0300–0.0400 | 14237696 | 491640 | 79 |
| wordfreq.sm | c | debug | 30 | 0.0050 | 0.0000–0.0100 | 2457600 | 33920 | 108 |
| wordfreq.sm | c | release | 30 | 0.0000 | 0.0000–0.0000 | 2457600 | 33800 | 108 |
| wordfreq.sm | cxx | debug | 30 | 0.0100 | 0.0000–0.0100 | 2490368 | 33992 | 115 |
| wordfreq.sm | cxx | release | 30 | 0.0000 | 0.0000–0.0000 | 2473984 | 33816 | 115 |
| wordfreq.sm | ours | debug | 30 | 0.0100 | 0.0100–0.0100 | 4603904 | 95016 | 62 |
| wordfreq.sm | ours | release | 30 | 0.0000 | 0.0000–0.0050 | 4587520 | 78568 | 62 |
| wordfreq.sm | rs | debug | 30 | 0.0200 | 0.0100–0.0200 | 2719744 | 526664 | 79 |
| wordfreq.sm | rs | release | 30 | 0.0000 | 0.0000–0.0000 | 2670592 | 491640 | 79 |

## paired ours − {lang} wall diff (median, 95% CI)

| workload | mode | vs | median diff s | 95% CI |
|---|---|---|---|---|
| prodcons_bchan_c256.md | debug | rs | -0.0200 | -0.0300–-0.0100 |
| jsonparse.sm | debug | c | +0.0100 | +0.0000–+0.0100 |
| fcopy.lg | debug | cxx | +0.0000 | -0.0300–+0.0400 |
| prodcons_bchan_c1.sm | release | cxx | +0.0450 | -0.0400–+0.1100 |
| prodcons_bchan_c16.lg | debug | c | -0.3650 | -0.4500–-0.2800 |
| prodcons_bchan_c1.lg | debug | c | +1.1500 | -0.7300–+2.3550 |
| prodcons.sm | release | cxx | +0.0200 | +0.0200–+0.0200 |
| fcopy.md | debug | rs | +0.0000 | +0.0000–+0.0150 |
| prodcons_bchan_c256.lg | debug | c | +0.6400 | +0.6100–+0.6700 |
| prodcons_bchan_c1.lg | release | cxx | +2.3850 | +0.8100–+4.1700 |
| prodcons.sm | debug | cxx | +0.0200 | +0.0200–+0.0200 |
| mapbench.lg | release | c | +0.0250 | -0.0850–+0.1400 |
| prodcons_bchan_c1.lg | release | rs | +3.3050 | +2.4200–+4.3600 |
| jsonparse.lg | release | cxx | -0.0450 | -0.0500–-0.0300 |
| prodcons_bchan_c4096.sm | release | cxx | +0.0200 | +0.0200–+0.0300 |
| prodcons_bchan_c16.md | release | rs | +0.0000 | -0.0150–+0.0050 |
| jsonparse.md | release | cxx | -0.0100 | -0.0100–-0.0100 |
| wordfreq.md | release | rs | +0.0200 | +0.0200–+0.0300 |
| jsonparse.md | release | rs | +0.0200 | +0.0150–+0.0200 |
| prodcons_bchan_c16.md | debug | rs | -0.0200 | -0.0300–-0.0050 |
| fcopy.sm | release | c | +0.0000 | +0.0000–+0.0000 |
| jsonparse.sm | release | cxx | +0.0000 | +0.0000–+0.0000 |
| prodcons_bchan_c4096.sm | release | rs | -0.0200 | -0.0300–-0.0200 |
| prodcons_bchan_c4096.lg | release | c | +0.3950 | +0.3850–+0.4600 |
| prodcons_bchan_c4096.md | debug | c | +0.0900 | +0.0800–+0.1200 |
| jsonparse.sm | release | c | +0.0000 | +0.0000–+0.0000 |
| fcopy.lg | release | rs | +0.0000 | -0.0200–+0.0300 |
| prodcons.md | debug | rs | -0.0250 | -0.1350–-0.0200 |
| fcopy.lg | release | c | +0.0000 | -0.0150–+0.0200 |
| prodcons_bchan_c256.sm | release | cxx | +0.0300 | +0.0300–+0.0300 |
| prodcons_bchan_c16.lg | debug | cxx | -0.0950 | -0.1750–-0.0350 |
| psum.sm | release | c | +0.0000 | +0.0000–+0.0000 |
| prodcons_bchan_c1.sm | release | rs | +0.1000 | +0.0150–+0.1400 |
| prodcons_bchan_c1.md | release | c | +0.2750 | +0.0050–+0.5550 |
| wordfreq.md | debug | cxx | +0.0800 | +0.0800–+0.0900 |
| prodcons.md | debug | cxx | +0.0700 | +0.0600–+0.0700 |
| prodcons_bchan_c1.sm | debug | cxx | -0.1550 | -0.2650–-0.0250 |
| prodcons_bchan_c1.md | debug | c | +0.3700 | -0.2200–+0.5450 |
| psum.lg | debug | cxx | +0.0800 | +0.0500–+0.1300 |
| prodcons_bchan_c16.lg | release | cxx | -0.1450 | -0.2300–-0.0500 |
| fcopy.md | debug | c | +0.0000 | -0.0100–+0.0150 |
| prodcons_bchan_c4096.sm | debug | c | +0.0300 | +0.0200–+0.0300 |
| fcopy.lg | debug | rs | -0.0050 | -0.0400–+0.0000 |
| prodcons.lg | debug | c | +0.3050 | +0.2700–+0.3200 |
| psum.md | debug | cxx | +0.0200 | +0.0100–+0.0200 |
| prodcons_bchan_c4096.lg | debug | cxx | +0.2750 | +0.2350–+0.3000 |
| prodcons.md | release | rs | -0.0800 | -0.1000–-0.0650 |
| mapbench.md | release | c | +0.0200 | +0.0100–+0.0300 |
| fcopy.sm | release | rs | +0.0000 | +0.0000–+0.0000 |
| mapbench.lg | release | cxx | +0.0750 | -0.0600–+0.1550 |
| mapbench.sm | release | cxx | +0.0000 | +0.0000–+0.0100 |
| jsonparse.lg | release | c | -0.0400 | -0.0450–-0.0300 |
| fcopy.lg | release | cxx | +0.0000 | -0.0250–+0.0400 |
| prodcons.sm | debug | rs | -0.0100 | -0.0200–-0.0050 |
| psum.sm | release | cxx | +0.0000 | +0.0000–+0.0000 |
| prodcons_bchan_c16.lg | release | c | -0.3550 | -0.4600–-0.2900 |
| prodcons.lg | debug | rs | -0.0550 | -0.4600–+0.0100 |
| prodcons.lg | release | c | +0.3150 | +0.3050–+0.3500 |
| psum.lg | debug | rs | -0.1450 | -0.2050–-0.1100 |
| wordfreq.md | release | c | +0.0200 | +0.0200–+0.0300 |
| prodcons.sm | release | c | +0.0200 | +0.0200–+0.0200 |
| wordfreq.lg | debug | cxx | +0.3200 | +0.3200–+0.3300 |
| prodcons.lg | release | cxx | +0.3150 | +0.2850–+0.3500 |
| wordfreq.lg | debug | c | +0.3250 | +0.3200–+0.3300 |
| jsonparse.md | debug | cxx | +0.0500 | +0.0400–+0.0500 |
| mapbench.sm | release | rs | +0.0000 | +0.0000–+0.0000 |
| prodcons_bchan_c4096.lg | debug | c | +0.4350 | +0.4000–+0.4650 |
| mapbench.md | debug | cxx | +0.2000 | +0.1900–+0.2100 |
| prodcons_bchan_c256.lg | release | cxx | +0.5600 | +0.5200–+0.5750 |
| prodcons_bchan_c16.sm | debug | c | -0.0200 | -0.0300–-0.0200 |
| prodcons_bchan_c16.md | release | c | -0.0350 | -0.0450–-0.0200 |
| jsonparse.lg | debug | cxx | +0.2000 | +0.1800–+0.2050 |
| prodcons_bchan_c1.sm | debug | c | +0.0500 | +0.0100–+0.1250 |
| prodcons_bchan_c16.sm | release | cxx | -0.0100 | -0.0100–+0.0000 |
| wordfreq.sm | debug | c | +0.0100 | +0.0000–+0.0100 |
| prodcons_bchan_c256.lg | release | rs | -0.1900 | -0.2400–-0.1500 |
| psum.sm | debug | cxx | +0.0000 | +0.0000–+0.0000 |
| jsonparse.md | debug | rs | +0.0550 | +0.0500–+0.0600 |
| prodcons.sm | debug | c | +0.0200 | +0.0200–+0.0200 |
| prodcons_bchan_c1.md | release | cxx | +0.4350 | +0.0400–+0.8850 |
| prodcons_bchan_c16.lg | debug | rs | -0.2400 | -0.2750–-0.1900 |
| fcopy.sm | debug | c | +0.0000 | +0.0000–+0.0000 |
| mapbench.lg | debug | c | +0.6950 | +0.5000–+0.8200 |
| prodcons_bchan_c256.sm | release | rs | +0.0000 | -0.0100–+0.0100 |
| jsonparse.sm | release | rs | +0.0000 | +0.0000–+0.0000 |
| prodcons_bchan_c256.sm | debug | cxx | +0.0300 | +0.0200–+0.0300 |
| prodcons_bchan_c16.sm | release | c | -0.0200 | -0.0250–-0.0200 |
| psum.md | release | c | +0.0050 | +0.0000–+0.0150 |
| prodcons_bchan_c16.md | debug | cxx | -0.0100 | -0.0250–+0.0050 |
| fcopy.sm | debug | rs | +0.0000 | +0.0000–+0.0000 |
| fcopy.lg | debug | c | +0.0000 | -0.0400–+0.0200 |
| prodcons_bchan_c16.sm | release | rs | +0.0000 | -0.0100–+0.0000 |
| prodcons_bchan_c1.md | debug | cxx | -0.0500 | -0.5750–+0.2200 |
| psum.lg | release | c | +0.0050 | -0.0100–+0.0600 |
| jsonparse.lg | release | rs | +0.0700 | +0.0600–+0.0750 |
| jsonparse.lg | debug | rs | +0.2250 | +0.2100–+0.2350 |
| jsonparse.md | release | c | +0.0000 | -0.0100–+0.0000 |
| wordfreq.md | release | cxx | +0.0200 | +0.0200–+0.0300 |
| wordfreq.lg | release | cxx | +0.0900 | +0.0850–+0.1050 |
| prodcons_bchan_c16.lg | release | rs | -0.0700 | -0.1550–+0.0100 |
| wordfreq.md | debug | c | +0.0900 | +0.0800–+0.0900 |
| prodcons_bchan_c4096.md | release | c | +0.1000 | +0.0900–+0.1100 |
| psum.sm | release | rs | +0.0000 | +0.0000–+0.0000 |
| prodcons_bchan_c16.md | release | cxx | -0.0050 | -0.0300–+0.0000 |
| jsonparse.lg | debug | c | +0.1950 | +0.1800–+0.2050 |
| fcopy.sm | release | cxx | +0.0000 | +0.0000–+0.0000 |
| prodcons_bchan_c4096.md | debug | rs | -0.1000 | -0.1350–-0.0550 |
| wordfreq.lg | debug | rs | -0.0750 | -0.1100–-0.0700 |
| prodcons_bchan_c16.sm | debug | cxx | +0.0000 | -0.0100–+0.0000 |
| mapbench.sm | debug | rs | -0.0800 | -0.0800–-0.0700 |
| fcopy.md | release | rs | +0.0000 | -0.0200–+0.0000 |
| prodcons_bchan_c1.lg | debug | rs | -2.0650 | -286.3550–+1.1350 |
| prodcons_bchan_c4096.md | release | cxx | +0.0750 | +0.0600–+0.0900 |
| prodcons_bchan_c4096.md | release | rs | -0.1050 | -0.1500–-0.0850 |
| psum.md | release | rs | +0.0000 | -0.0100–+0.0100 |
| prodcons_bchan_c256.md | release | rs | -0.0550 | -0.0700–-0.0400 |
| wordfreq.sm | debug | rs | +0.0000 | -0.0100–+0.0000 |
| prodcons_bchan_c4096.md | debug | cxx | +0.0450 | +0.0300–+0.0650 |
| psum.lg | release | cxx | +0.0350 | -0.0250–+0.0900 |
| mapbench.md | release | rs | +0.0000 | -0.0150–+0.0100 |
| prodcons.md | release | cxx | +0.0900 | +0.0900–+0.1000 |
| prodcons_bchan_c256.sm | debug | c | +0.0400 | +0.0400–+0.0400 |
| jsonparse.sm | debug | cxx | +0.0100 | +0.0000–+0.0100 |
| prodcons_bchan_c1.lg | release | c | +2.4400 | +0.8600–+3.4900 |
| prodcons_bchan_c256.sm | debug | rs | -0.0100 | -0.0100–-0.0100 |
| mapbench.lg | debug | rs | -1.1850 | -1.4100–-1.1000 |
| fcopy.sm | debug | cxx | +0.0000 | +0.0000–+0.0000 |
| prodcons_bchan_c1.lg | debug | cxx | -1.0600 | -2.5350–+0.4100 |
| mapbench.sm | release | c | +0.0000 | +0.0000–+0.0100 |
| prodcons_bchan_c16.md | debug | c | -0.0900 | -0.1050–-0.0650 |
| jsonparse.md | debug | c | +0.0450 | +0.0400–+0.0500 |
| wordfreq.sm | release | cxx | +0.0000 | +0.0000–+0.0050 |
| mapbench.md | debug | rs | -0.3650 | -0.3900–-0.3350 |
| prodcons_bchan_c1.md | release | rs | +0.4600 | +0.1000–+0.8500 |
| fcopy.md | debug | cxx | +0.0000 | -0.0050–+0.0050 |
| jsonparse.sm | debug | rs | +0.0100 | +0.0000–+0.0100 |
| prodcons_bchan_c4096.sm | debug | rs | -0.0250 | -0.0300–-0.0200 |
| prodcons_bchan_c4096.sm | release | c | +0.0300 | +0.0300–+0.0300 |
| psum.md | debug | c | +0.0200 | +0.0100–+0.0250 |
| prodcons.lg | debug | cxx | +0.3000 | +0.2800–+0.3200 |
| prodcons.md | debug | c | +0.0600 | +0.0600–+0.0700 |
| prodcons_bchan_c1.sm | debug | rs | -0.0450 | -0.2200–+0.0250 |
| mapbench.sm | debug | c | +0.0500 | +0.0500–+0.0500 |
| fcopy.md | release | c | +0.0000 | +0.0000–+0.0000 |
| prodcons_bchan_c4096.sm | debug | cxx | +0.0150 | +0.0100–+0.0200 |
| prodcons_bchan_c256.md | debug | cxx | +0.1200 | +0.1100–+0.1400 |
| prodcons_bchan_c4096.lg | release | cxx | +0.3750 | +0.3400–+0.3900 |
| prodcons.lg | release | rs | -0.2650 | -0.3400–-0.2250 |
| mapbench.md | release | cxx | +0.0200 | +0.0000–+0.0200 |
| prodcons_bchan_c256.lg | release | c | +0.6350 | +0.6050–+0.6600 |
| wordfreq.md | debug | rs | -0.0200 | -0.0200–-0.0200 |
| wordfreq.lg | release | rs | +0.1000 | +0.1000–+0.1100 |
| mapbench.lg | release | rs | -0.0500 | -0.1600–+0.0650 |
| psum.sm | debug | c | +0.0000 | +0.0000–+0.0000 |
| mapbench.lg | debug | cxx | +0.6750 | +0.5000–+0.8050 |
| prodcons_bchan_c4096.lg | release | rs | -0.5350 | -0.6000–-0.3100 |
| psum.md | debug | rs | -0.0500 | -0.0550–-0.0400 |
| wordfreq.sm | release | rs | +0.0000 | +0.0000–+0.0050 |
| fcopy.md | release | cxx | +0.0000 | +0.0000–+0.0000 |
| mapbench.md | debug | c | +0.2050 | +0.1900–+0.2300 |
| prodcons.md | release | c | +0.0900 | +0.0800–+0.1000 |
| prodcons_bchan_c256.md | release | cxx | +0.1200 | +0.1100–+0.1250 |
| psum.lg | release | rs | +0.0100 | -0.0900–+0.0500 |
| prodcons_bchan_c1.sm | release | c | +0.0900 | -0.0300–+0.1150 |
| prodcons_bchan_c256.md | debug | c | +0.1800 | +0.1700–+0.1950 |
| prodcons_bchan_c256.lg | debug | rs | -0.1550 | -0.1700–-0.1300 |
| prodcons_bchan_c256.lg | debug | cxx | +0.4200 | +0.3850–+0.4750 |
| prodcons_bchan_c4096.lg | debug | rs | -0.2650 | -0.4900–-0.1600 |
| psum.md | release | cxx | +0.0000 | +0.0000–+0.0150 |
| psum.sm | debug | rs | -0.0100 | -0.0200–-0.0100 |
| wordfreq.lg | release | c | +0.1000 | +0.0900–+0.1050 |
| prodcons.sm | release | rs | -0.0200 | -0.0200–-0.0200 |
| prodcons_bchan_c16.sm | debug | rs | -0.0100 | -0.0100–-0.0100 |
| prodcons_bchan_c1.md | debug | rs | -0.1100 | -0.4150–+0.0950 |
| psum.lg | debug | c | +0.0500 | -0.0300–+0.1000 |
| wordfreq.sm | release | c | +0.0000 | +0.0000–+0.0050 |
| prodcons_bchan_c256.sm | release | c | +0.0400 | +0.0300–+0.0400 |
| mapbench.sm | debug | cxx | +0.0500 | +0.0400–+0.0500 |
| prodcons_bchan_c256.md | release | c | +0.1400 | +0.1400–+0.1400 |
| wordfreq.sm | debug | cxx | +0.0000 | +0.0000–+0.0100 |
| httpsrv.lg | release | rs | -0.0085 | -0.0229–+0.0205 |
| kvsrv.md | debug | cxx | -0.0003 | -0.0022–+0.0021 |
| httpsrv.lg | release | cxx | -0.0017 | -0.0116–+0.0094 |
| httpsrv.md | release | rs | +0.0011 | -0.0007–+0.0030 |
| httpsrv.md | debug | rs | -0.0017 | -0.0031–-0.0001 |
| httpsrv.lg | debug | cxx | +0.0026 | -0.0133–+0.0124 |
| httpsrv.sm | release | c | +0.0002 | -0.0009–+0.0008 |
| httpsrv.md | release | c | -0.0003 | -0.0054–+0.0017 |
| kvsrv.md | release | c | +0.0013 | -0.0026–+0.0075 |
| kvsrv.md | release | cxx | +0.0033 | -0.0002–+0.0058 |
| httpsrv.sm | debug | rs | -0.0001 | -0.0026–+0.0005 |
| kvsrv.lg | release | rs | +0.0104 | -0.0056–+0.0231 |
| kvsrv.sm | release | c | -0.0001 | -0.0008–+0.0006 |
| httpsrv.sm | release | cxx | +0.0003 | -0.0015–+0.0015 |
| kvsrv.md | release | rs | +0.0004 | -0.0018–+0.0023 |
| kvsrv.sm | release | cxx | -0.0003 | -0.0010–+0.0002 |
| kvsrv.md | debug | rs | -0.0006 | -0.0060–+0.0037 |
| httpsrv.md | debug | cxx | -0.0018 | -0.0045–+0.0009 |
| kvsrv.md | debug | c | +0.0010 | -0.0015–+0.0033 |
| kvsrv.sm | debug | cxx | +0.0003 | -0.0008–+0.0021 |
| httpsrv.lg | release | c | -0.0118 | -0.0282–+0.0135 |
| kvsrv.lg | release | c | +0.0128 | -0.0048–+0.0298 |
| httpsrv.sm | release | rs | +0.0003 | -0.0001–+0.0008 |
| kvsrv.sm | release | rs | +0.0000 | -0.0019–+0.0006 |
| httpsrv.md | release | cxx | +0.0005 | -0.0016–+0.0032 |
| kvsrv.lg | debug | c | -0.0015 | -0.0201–+0.0325 |
| httpsrv.lg | debug | c | -0.0020 | -0.0062–+0.0058 |
| httpsrv.sm | debug | c | -0.0004 | -0.0021–+0.0009 |
| kvsrv.sm | debug | c | +0.0012 | +0.0002–+0.0022 |
| kvsrv.sm | debug | rs | +0.0002 | -0.0005–+0.0010 |
| httpsrv.sm | debug | cxx | -0.0002 | -0.0009–+0.0002 |
| kvsrv.lg | debug | rs | +0.0117 | -0.0070–+0.0282 |
| httpsrv.lg | debug | rs | -0.0054 | -0.0189–+0.0099 |
| kvsrv.lg | debug | cxx | +0.0100 | -0.0199–+0.0286 |
| kvsrv.lg | release | cxx | -0.0071 | -0.0333–+0.0166 |
| httpsrv.md | debug | c | -0.0018 | -0.0069–+0.0019 |
