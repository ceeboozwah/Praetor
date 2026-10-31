# PraetorBench-1.3 — 20261031-104933 (lab-grade)

| workload | lang | mode | n | median s | 95% CI | rss B | size B | loc |
|---|---|---|---|---|---|---|---|---|
| httpsrv.lg | c | debug | 10 | 0.1877 | 0.1842–0.2110 | 1392640 | 34080 | 35 |
| httpsrv.lg | c | release | 10 | 0.1859 | 0.1791–0.1921 | 1376256 | 34008 | 35 |
| httpsrv.lg | cxx | debug | 10 | 0.1907 | 0.1785–0.2111 | 1409024 | 34088 | 37 |
| httpsrv.lg | cxx | release | 10 | 0.2055 | 0.1870–0.2178 | 1409024 | 34008 | 37 |
| httpsrv.lg | ours | debug | 10 | 0.2014 | 0.1828–0.2241 | 1392640 | 96104 | 39 |
| httpsrv.lg | ours | release | 10 | 0.1853 | 0.1831–0.2163 | 1392640 | 96168 | 39 |
| httpsrv.lg | rs | debug | 10 | 0.1975 | 0.1829–0.2258 | 1589248 | 548496 | 25 |
| httpsrv.lg | rs | release | 10 | 0.2201 | 0.1905–0.2356 | 1589248 | 498152 | 25 |
| httpsrv.md | c | debug | 20 | 0.0735 | 0.0707–0.0752 | 1392640 | 34080 | 35 |
| httpsrv.md | c | release | 20 | 0.0772 | 0.0714–0.0851 | 1376256 | 34008 | 35 |
| httpsrv.md | cxx | debug | 20 | 0.0734 | 0.0708–0.0791 | 1392640 | 34088 | 37 |
| httpsrv.md | cxx | release | 20 | 0.0740 | 0.0703–0.0777 | 1409024 | 34008 | 37 |
| httpsrv.md | ours | debug | 20 | 0.0741 | 0.0723–0.0810 | 1392640 | 96104 | 39 |
| httpsrv.md | ours | release | 20 | 0.0728 | 0.0705–0.0762 | 1392640 | 96168 | 39 |
| httpsrv.md | rs | debug | 20 | 0.0722 | 0.0702–0.0755 | 1605632 | 548496 | 25 |
| httpsrv.md | rs | release | 20 | 0.0719 | 0.0706–0.0764 | 1589248 | 498152 | 25 |
| httpsrv.sm | c | debug | 30 | 0.0224 | 0.0220–0.0227 | 1392640 | 34080 | 35 |
| httpsrv.sm | c | release | 30 | 0.0226 | 0.0221–0.0236 | 1392640 | 34008 | 35 |
| httpsrv.sm | cxx | debug | 30 | 0.0223 | 0.0221–0.0233 | 1409024 | 34088 | 37 |
| httpsrv.sm | cxx | release | 30 | 0.0230 | 0.0222–0.0241 | 1392640 | 34008 | 37 |
| httpsrv.sm | ours | debug | 30 | 0.0225 | 0.0222–0.0232 | 1392640 | 96104 | 39 |
| httpsrv.sm | ours | release | 30 | 0.0229 | 0.0225–0.0245 | 1376256 | 96168 | 39 |
| httpsrv.sm | rs | debug | 30 | 0.0227 | 0.0222–0.0239 | 1605632 | 548496 | 25 |
| httpsrv.sm | rs | release | 30 | 0.0225 | 0.0221–0.0232 | 1572864 | 498152 | 25 |
| kvsrv.lg | c | debug | 10 | 0.1919 | 0.1786–0.2089 | 1376256 | 34448 | 127 |
| kvsrv.lg | c | release | 10 | 0.2109 | 0.1810–0.2335 | 1392640 | 34288 | 127 |
| kvsrv.lg | cxx | debug | 10 | 0.1970 | 0.1835–0.2129 | 1392640 | 34480 | 134 |
| kvsrv.lg | cxx | release | 10 | 0.2040 | 0.1868–0.2200 | 1392640 | 34312 | 134 |
| kvsrv.lg | ours | debug | 10 | 0.2048 | 0.1924–0.2424 | 1458176 | 113408 | 142 |
| kvsrv.lg | ours | release | 10 | 0.2051 | 0.1860–0.2159 | 1409024 | 97272 | 142 |
| kvsrv.lg | rs | debug | 10 | 0.1986 | 0.1886–0.2197 | 1703936 | 559168 | 95 |
| kvsrv.lg | rs | release | 10 | 0.1817 | 0.1783–0.2001 | 1638400 | 515552 | 95 |
| kvsrv.md | c | debug | 20 | 0.0729 | 0.0716–0.0764 | 1392640 | 34448 | 127 |
| kvsrv.md | c | release | 20 | 0.0718 | 0.0701–0.0753 | 1392640 | 34288 | 127 |
| kvsrv.md | cxx | debug | 20 | 0.0716 | 0.0711–0.0758 | 1392640 | 34480 | 134 |
| kvsrv.md | cxx | release | 20 | 0.0723 | 0.0704–0.0758 | 1409024 | 34312 | 134 |
| kvsrv.md | ours | debug | 20 | 0.0732 | 0.0708–0.0746 | 1458176 | 113408 | 142 |
| kvsrv.md | ours | release | 20 | 0.0732 | 0.0722–0.0768 | 1409024 | 97272 | 142 |
| kvsrv.md | rs | debug | 20 | 0.0734 | 0.0716–0.0757 | 1703936 | 559168 | 95 |
| kvsrv.md | rs | release | 20 | 0.0764 | 0.0715–0.0830 | 1654784 | 515552 | 95 |
| kvsrv.sm | c | debug | 30 | 0.0228 | 0.0225–0.0236 | 1376256 | 34448 | 127 |
| kvsrv.sm | c | release | 30 | 0.0235 | 0.0226–0.0247 | 1376256 | 34288 | 127 |
| kvsrv.sm | cxx | debug | 30 | 0.0225 | 0.0223–0.0228 | 1392640 | 34480 | 134 |
| kvsrv.sm | cxx | release | 30 | 0.0223 | 0.0221–0.0232 | 1409024 | 34312 | 134 |
| kvsrv.sm | ours | debug | 30 | 0.0229 | 0.0224–0.0233 | 1458176 | 113408 | 142 |
| kvsrv.sm | ours | release | 30 | 0.0227 | 0.0224–0.0232 | 1425408 | 97272 | 142 |
| kvsrv.sm | rs | debug | 30 | 0.0230 | 0.0227–0.0238 | 1703936 | 559168 | 95 |
| kvsrv.sm | rs | release | 30 | 0.0229 | 0.0222–0.0237 | 1654784 | 515552 | 95 |
| mapbench.lg | c | debug | 10 | 1.8450 | 1.7300–1.9800 | 1612201984 | 33680 | 75 |
| mapbench.lg | c | release | 10 | 1.1250 | 1.0200–1.2750 | 1612201984 | 33608 | 75 |
| mapbench.lg | cxx | debug | 10 | 1.8550 | 1.7400–1.9000 | 1612218368 | 33736 | 76 |
| mapbench.lg | cxx | release | 10 | 1.1550 | 1.0200–1.2300 | 1612218368 | 33624 | 76 |
| mapbench.lg | ours | debug | 10 | 2.5450 | 2.3900–2.6450 | 1612234752 | 74936 | 109 |
| mapbench.lg | ours | release | 10 | 1.1500 | 1.0650–1.3200 | 1612234752 | 74872 | 109 |
| mapbench.lg | rs | debug | 10 | 3.8300 | 3.6100–4.0500 | 1612414976 | 503992 | 62 |
| mapbench.lg | rs | release | 10 | 1.2650 | 1.1300–1.3600 | 1612414976 | 490568 | 62 |
| mapbench.md | c | debug | 20 | 0.5200 | 0.5100–0.5350 | 404226048 | 33680 | 75 |
| mapbench.md | c | release | 20 | 0.2600 | 0.2600–0.2850 | 404226048 | 33608 | 75 |
| mapbench.md | cxx | debug | 20 | 0.5300 | 0.5100–0.5800 | 404242432 | 33736 | 76 |
| mapbench.md | cxx | release | 20 | 0.2600 | 0.2600–0.2800 | 404226048 | 33624 | 76 |
| mapbench.md | ours | debug | 20 | 0.7300 | 0.7100–0.7950 | 404258816 | 74936 | 109 |
| mapbench.md | ours | release | 20 | 0.2800 | 0.2700–0.3000 | 404291584 | 74872 | 109 |
| mapbench.md | rs | debug | 20 | 1.1150 | 1.0950–1.2100 | 404439040 | 503992 | 62 |
| mapbench.md | rs | release | 20 | 0.2700 | 0.2700–0.3000 | 404422656 | 490568 | 62 |
| mapbench.sm | c | debug | 30 | 0.1000 | 0.1000–0.1000 | 102187008 | 33680 | 75 |
| mapbench.sm | c | release | 30 | 0.0500 | 0.0500–0.0500 | 102203392 | 33608 | 75 |
| mapbench.sm | cxx | debug | 30 | 0.1000 | 0.1000–0.1000 | 102219776 | 33736 | 76 |
| mapbench.sm | cxx | release | 30 | 0.0500 | 0.0500–0.0500 | 102203392 | 33624 | 76 |
| mapbench.sm | ours | debug | 30 | 0.1500 | 0.1500–0.1600 | 102252544 | 74936 | 109 |
| mapbench.sm | ours | release | 30 | 0.0500 | 0.0500–0.0500 | 102236160 | 74872 | 109 |
| mapbench.sm | rs | debug | 30 | 0.2300 | 0.2300–0.2450 | 102400000 | 503992 | 62 |
| mapbench.sm | rs | release | 30 | 0.0500 | 0.0500–0.0500 | 102416384 | 490568 | 62 |
| prodcons.lg | c | debug | 10 | 0.3700 | 0.3700–0.3800 | 35012608 | 34256 | 75 |
| prodcons.lg | c | release | 10 | 0.3400 | 0.3300–0.3400 | 22544384 | 34168 | 75 |
| prodcons.lg | cxx | debug | 10 | 0.3800 | 0.3700–0.3900 | 13025280 | 34296 | 76 |
| prodcons.lg | cxx | release | 10 | 0.3400 | 0.3300–0.3500 | 10403840 | 34184 | 76 |
| prodcons.lg | ours | debug | 10 | 0.6600 | 0.6400–0.6900 | 10272768 | 74760 | 42 |
| prodcons.lg | ours | release | 10 | 0.6450 | 0.6300–0.6700 | 79888384 | 74744 | 42 |
| prodcons.lg | rs | debug | 10 | 0.7100 | 0.6950–0.7650 | 4194304 | 693464 | 24 |
| prodcons.lg | rs | release | 10 | 0.8750 | 0.8200–0.8950 | 2588672 | 575880 | 24 |
| prodcons.md | c | debug | 20 | 0.0900 | 0.0900–0.0900 | 4489216 | 34256 | 75 |
| prodcons.md | c | release | 20 | 0.0800 | 0.0800–0.0800 | 13041664 | 34168 | 75 |
| prodcons.md | cxx | debug | 20 | 0.0900 | 0.0900–0.1000 | 6733824 | 34296 | 76 |
| prodcons.md | cxx | release | 20 | 0.0850 | 0.0800–0.0900 | 5062656 | 34184 | 76 |
| prodcons.md | ours | debug | 20 | 0.1600 | 0.1600–0.1650 | 22593536 | 74760 | 42 |
| prodcons.md | ours | release | 20 | 0.1700 | 0.1700–0.1700 | 7110656 | 74744 | 42 |
| prodcons.md | rs | debug | 20 | 0.1750 | 0.1650–0.1800 | 2818048 | 693464 | 24 |
| prodcons.md | rs | release | 20 | 0.2300 | 0.2000–0.2300 | 4096000 | 575880 | 24 |
| prodcons.sm | c | debug | 30 | 0.0200 | 0.0200–0.0200 | 4702208 | 34256 | 75 |
| prodcons.sm | c | release | 30 | 0.0200 | 0.0200–0.0200 | 5455872 | 34168 | 75 |
| prodcons.sm | cxx | debug | 30 | 0.0200 | 0.0200–0.0200 | 5603328 | 34296 | 76 |
| prodcons.sm | cxx | release | 30 | 0.0200 | 0.0200–0.0200 | 3145728 | 34184 | 76 |
| prodcons.sm | ours | debug | 30 | 0.0400 | 0.0400–0.0400 | 3489792 | 74760 | 42 |
| prodcons.sm | ours | release | 30 | 0.0400 | 0.0400–0.0400 | 5406720 | 74744 | 42 |
| prodcons.sm | rs | debug | 30 | 0.0400 | 0.0400–0.0500 | 2129920 | 693464 | 24 |
| prodcons.sm | rs | release | 30 | 0.0500 | 0.0500–0.0500 | 2244608 | 575880 | 24 |
| prodcons_bchan_c1.lg | c | debug | 10 | 27.0350 | 25.8650–28.4000 | 1441792 | 34272 | 91 |
| prodcons_bchan_c1.lg | c | release | 10 | 27.0550 | 26.3600–28.4100 | 1409024 | 34176 | 91 |
| prodcons_bchan_c1.lg | cxx | debug | 10 | 29.7450 | 29.4250–30.8100 | 1441792 | 49680 | 72 |
| prodcons_bchan_c1.lg | cxx | release | 10 | 27.2550 | 26.3050–28.0500 | 1458176 | 36544 | 72 |
| prodcons_bchan_c1.lg | ours | debug | 10 | 28.4500 | 28.0500–30.2200 | 1490944 | 74768 | 58 |
| prodcons_bchan_c1.lg | ours | release | 10 | 29.0950 | 27.0200–30.1600 | 1458176 | 74752 | 58 |
| prodcons_bchan_c1.lg | rs | debug | 10 | 29.8350 | 29.1300–30.6700 | 1785856 | 706912 | 27 |
| prodcons_bchan_c1.lg | rs | release | 10 | 26.2100 | 25.2900–27.1500 | 1720320 | 580224 | 27 |
| prodcons_bchan_c1.md | c | debug | 20 | 7.0550 | 6.7800–7.2500 | 1441792 | 34272 | 91 |
| prodcons_bchan_c1.md | c | release | 20 | 7.1900 | 6.7200–7.4050 | 1441792 | 34176 | 91 |
| prodcons_bchan_c1.md | cxx | debug | 20 | 7.5400 | 7.3600–7.6900 | 1458176 | 49680 | 72 |
| prodcons_bchan_c1.md | cxx | release | 20 | 6.9150 | 6.6100–7.0200 | 1474560 | 36544 | 72 |
| prodcons_bchan_c1.md | ours | debug | 20 | 6.9650 | 6.4450–7.2050 | 1474560 | 74768 | 58 |
| prodcons_bchan_c1.md | ours | release | 20 | 6.8300 | 6.6050–7.2000 | 1474560 | 74752 | 58 |
| prodcons_bchan_c1.md | rs | debug | 20 | 7.7500 | 7.3850–7.8950 | 1802240 | 706912 | 27 |
| prodcons_bchan_c1.md | rs | release | 20 | 6.5650 | 6.1000–6.9200 | 1736704 | 580224 | 27 |
| prodcons_bchan_c1.sm | c | debug | 30 | 1.7200 | 1.6350–1.7700 | 1425408 | 34272 | 91 |
| prodcons_bchan_c1.sm | c | release | 30 | 1.6400 | 1.6100–1.7400 | 1441792 | 34176 | 91 |
| prodcons_bchan_c1.sm | cxx | debug | 30 | 1.8350 | 1.7549–1.9250 | 1458176 | 49680 | 72 |
| prodcons_bchan_c1.sm | cxx | release | 30 | 1.6450 | 1.5999–1.7200 | 1425408 | 36544 | 72 |
| prodcons_bchan_c1.sm | ours | debug | 30 | 1.8000 | 1.7250–1.8900 | 1458176 | 74768 | 58 |
| prodcons_bchan_c1.sm | ours | release | 30 | 1.7000 | 1.6200–1.7800 | 1474560 | 74752 | 58 |
| prodcons_bchan_c1.sm | rs | debug | 30 | 1.7900 | 1.7500–1.8400 | 1802240 | 706912 | 27 |
| prodcons_bchan_c1.sm | rs | release | 30 | 1.5900 | 1.5450–1.6450 | 1736704 | 580224 | 27 |
| prodcons_bchan_c16.lg | c | debug | 10 | 2.2200 | 2.1400–2.2800 | 1409024 | 34272 | 91 |
| prodcons_bchan_c16.lg | c | release | 10 | 2.3150 | 2.2100–2.3500 | 1425408 | 34176 | 91 |
| prodcons_bchan_c16.lg | cxx | debug | 10 | 1.9150 | 1.8650–2.0000 | 1441792 | 49680 | 72 |
| prodcons_bchan_c16.lg | cxx | release | 10 | 2.0050 | 1.8700–2.1100 | 1441792 | 36544 | 72 |
| prodcons_bchan_c16.lg | ours | debug | 10 | 1.9250 | 1.8500–1.9750 | 1458176 | 74768 | 58 |
| prodcons_bchan_c16.lg | ours | release | 10 | 2.0850 | 2.0150–2.1300 | 1474560 | 74752 | 58 |
| prodcons_bchan_c16.lg | rs | debug | 10 | 2.0350 | 1.9900–2.1000 | 1916928 | 706912 | 27 |
| prodcons_bchan_c16.lg | rs | release | 10 | 2.0350 | 2.0100–2.0400 | 1851392 | 580224 | 27 |
| prodcons_bchan_c16.md | c | debug | 20 | 0.5000 | 0.4900–0.5400 | 1409024 | 34272 | 91 |
| prodcons_bchan_c16.md | c | release | 20 | 0.5600 | 0.5400–0.5750 | 1409024 | 34176 | 91 |
| prodcons_bchan_c16.md | cxx | debug | 20 | 0.4900 | 0.4750–0.5200 | 1441792 | 49680 | 72 |
| prodcons_bchan_c16.md | cxx | release | 20 | 0.5000 | 0.4900–0.5200 | 1441792 | 36544 | 72 |
| prodcons_bchan_c16.md | ours | debug | 20 | 0.4900 | 0.4700–0.5050 | 1474560 | 74768 | 58 |
| prodcons_bchan_c16.md | ours | release | 20 | 0.5200 | 0.5100–0.5300 | 1458176 | 74752 | 58 |
| prodcons_bchan_c16.md | rs | debug | 20 | 0.5550 | 0.5450–0.5700 | 1916928 | 706912 | 27 |
| prodcons_bchan_c16.md | rs | release | 20 | 0.5000 | 0.5000–0.5100 | 1851392 | 580224 | 27 |
| prodcons_bchan_c16.sm | c | debug | 30 | 0.1350 | 0.1300–0.1400 | 1425408 | 34272 | 91 |
| prodcons_bchan_c16.sm | c | release | 30 | 0.1300 | 0.1200–0.1300 | 1392640 | 34176 | 91 |
| prodcons_bchan_c16.sm | cxx | debug | 30 | 0.1200 | 0.1200–0.1200 | 1441792 | 49680 | 72 |
| prodcons_bchan_c16.sm | cxx | release | 30 | 0.1200 | 0.1200–0.1300 | 1425408 | 36544 | 72 |
| prodcons_bchan_c16.sm | ours | debug | 30 | 0.1200 | 0.1200–0.1250 | 1441792 | 74768 | 58 |
| prodcons_bchan_c16.sm | ours | release | 30 | 0.1300 | 0.1250–0.1300 | 1458176 | 74752 | 58 |
| prodcons_bchan_c16.sm | rs | debug | 30 | 0.1300 | 0.1300–0.1300 | 1867776 | 706912 | 27 |
| prodcons_bchan_c16.sm | rs | release | 30 | 0.1200 | 0.1200–0.1300 | 1720320 | 580224 | 27 |
| prodcons_bchan_c256.lg | c | debug | 10 | 0.6650 | 0.6600–0.6750 | 1425408 | 34272 | 91 |
| prodcons_bchan_c256.lg | c | release | 10 | 0.6100 | 0.6000–0.6300 | 1458176 | 34176 | 91 |
| prodcons_bchan_c256.lg | cxx | debug | 10 | 0.8900 | 0.8800–0.8950 | 1441792 | 49680 | 72 |
| prodcons_bchan_c256.lg | cxx | release | 10 | 0.6800 | 0.6700–0.6900 | 1441792 | 36544 | 72 |
| prodcons_bchan_c256.lg | ours | debug | 10 | 0.7750 | 0.7650–0.7900 | 1458176 | 74768 | 58 |
| prodcons_bchan_c256.lg | ours | release | 10 | 0.7100 | 0.7050–0.7200 | 1458176 | 74752 | 58 |
| prodcons_bchan_c256.lg | rs | debug | 10 | 1.4750 | 1.4450–1.4800 | 1916928 | 706912 | 27 |
| prodcons_bchan_c256.lg | rs | release | 10 | 1.4100 | 1.3700–1.4500 | 1851392 | 580224 | 27 |
| prodcons_bchan_c256.md | c | debug | 20 | 0.1700 | 0.1650–0.1700 | 1409024 | 34272 | 91 |
| prodcons_bchan_c256.md | c | release | 20 | 0.1500 | 0.1500–0.1500 | 1441792 | 34176 | 91 |
| prodcons_bchan_c256.md | cxx | debug | 20 | 0.2200 | 0.2200–0.2200 | 1425408 | 49680 | 72 |
| prodcons_bchan_c256.md | cxx | release | 20 | 0.1700 | 0.1700–0.1700 | 1425408 | 36544 | 72 |
| prodcons_bchan_c256.md | ours | debug | 20 | 0.1900 | 0.1900–0.1950 | 1441792 | 74768 | 58 |
| prodcons_bchan_c256.md | ours | release | 20 | 0.1800 | 0.1700–0.1800 | 1474560 | 74752 | 58 |
| prodcons_bchan_c256.md | rs | debug | 20 | 0.3600 | 0.3500–0.3600 | 1916928 | 706912 | 27 |
| prodcons_bchan_c256.md | rs | release | 20 | 0.3450 | 0.3350–0.3550 | 1867776 | 580224 | 27 |
| prodcons_bchan_c256.sm | c | debug | 30 | 0.0400 | 0.0400–0.0400 | 1409024 | 34272 | 91 |
| prodcons_bchan_c256.sm | c | release | 30 | 0.0400 | 0.0400–0.0400 | 1425408 | 34176 | 91 |
| prodcons_bchan_c256.sm | cxx | debug | 30 | 0.0500 | 0.0500–0.0500 | 1458176 | 49680 | 72 |
| prodcons_bchan_c256.sm | cxx | release | 30 | 0.0400 | 0.0400–0.0400 | 1441792 | 36544 | 72 |
| prodcons_bchan_c256.sm | ours | debug | 30 | 0.0500 | 0.0500–0.0500 | 1458176 | 74768 | 58 |
| prodcons_bchan_c256.sm | ours | release | 30 | 0.0400 | 0.0400–0.0400 | 1441792 | 74752 | 58 |
| prodcons_bchan_c256.sm | rs | debug | 30 | 0.0900 | 0.0900–0.0900 | 1916928 | 706912 | 27 |
| prodcons_bchan_c256.sm | rs | release | 30 | 0.0800 | 0.0800–0.0900 | 1867776 | 580224 | 27 |
| prodcons_bchan_c4096.lg | c | debug | 10 | 0.4700 | 0.4550–0.4700 | 1523712 | 34272 | 91 |
| prodcons_bchan_c4096.lg | c | release | 10 | 0.4300 | 0.4200–0.4350 | 1540096 | 34176 | 91 |
| prodcons_bchan_c4096.lg | cxx | debug | 10 | 0.6400 | 0.6200–0.6500 | 1556480 | 49680 | 72 |
| prodcons_bchan_c4096.lg | cxx | release | 10 | 0.4650 | 0.4500–0.4800 | 1540096 | 36544 | 72 |
| prodcons_bchan_c4096.lg | ours | debug | 10 | 0.5150 | 0.5050–0.5250 | 1572864 | 74768 | 58 |
| prodcons_bchan_c4096.lg | ours | release | 10 | 0.4800 | 0.4650–0.5050 | 1572864 | 74760 | 58 |
| prodcons_bchan_c4096.lg | rs | debug | 10 | 1.2250 | 1.0800–1.2900 | 2048000 | 706912 | 27 |
| prodcons_bchan_c4096.lg | rs | release | 10 | 1.2400 | 1.1500–1.2700 | 1982464 | 580224 | 27 |
| prodcons_bchan_c4096.md | c | debug | 20 | 0.1100 | 0.1100–0.1100 | 1540096 | 34272 | 91 |
| prodcons_bchan_c4096.md | c | release | 20 | 0.1000 | 0.1000–0.1000 | 1540096 | 34176 | 91 |
| prodcons_bchan_c4096.md | cxx | debug | 20 | 0.1600 | 0.1500–0.1600 | 1540096 | 49680 | 72 |
| prodcons_bchan_c4096.md | cxx | release | 20 | 0.1200 | 0.1200–0.1200 | 1556480 | 36544 | 72 |
| prodcons_bchan_c4096.md | ours | debug | 20 | 0.1300 | 0.1300–0.1300 | 1556480 | 74768 | 58 |
| prodcons_bchan_c4096.md | ours | release | 20 | 0.1200 | 0.1200–0.1200 | 1572864 | 74760 | 58 |
| prodcons_bchan_c4096.md | rs | debug | 20 | 0.2850 | 0.2650–0.3100 | 2015232 | 706912 | 27 |
| prodcons_bchan_c4096.md | rs | release | 20 | 0.2850 | 0.2600–0.2900 | 1982464 | 580224 | 27 |
| prodcons_bchan_c4096.sm | c | debug | 30 | 0.0300 | 0.0300–0.0300 | 1540096 | 34272 | 91 |
| prodcons_bchan_c4096.sm | c | release | 30 | 0.0200 | 0.0200–0.0200 | 1540096 | 34176 | 91 |
| prodcons_bchan_c4096.sm | cxx | debug | 30 | 0.0400 | 0.0400–0.0400 | 1556480 | 49680 | 72 |
| prodcons_bchan_c4096.sm | cxx | release | 30 | 0.0300 | 0.0300–0.0300 | 1556480 | 36544 | 72 |
| prodcons_bchan_c4096.sm | ours | debug | 30 | 0.0300 | 0.0300–0.0300 | 1572864 | 74768 | 58 |
| prodcons_bchan_c4096.sm | ours | release | 30 | 0.0300 | 0.0300–0.0300 | 1572864 | 74760 | 58 |
| prodcons_bchan_c4096.sm | rs | debug | 30 | 0.0700 | 0.0700–0.0800 | 2015232 | 706912 | 27 |
| prodcons_bchan_c4096.sm | rs | release | 30 | 0.0700 | 0.0700–0.0700 | 1933312 | 580224 | 27 |
| psum.lg | c | debug | 10 | 0.0900 | 0.0850–0.1700 | 961560576 | 33840 | 37 |
| psum.lg | c | release | 10 | 0.0850 | 0.0400–0.1400 | 961560576 | 33840 | 37 |
| psum.lg | cxx | debug | 10 | 0.0900 | 0.0900–0.1200 | 961576960 | 34272 | 38 |
| psum.lg | cxx | release | 10 | 0.0400 | 0.0400–0.1000 | 961576960 | 33840 | 38 |
| psum.lg | ours | debug | 10 | 0.1500 | 0.1400–0.1750 | 961576960 | 74928 | 40 |
| psum.lg | ours | release | 10 | 0.1150 | 0.0400–0.1350 | 961593344 | 74920 | 40 |
| psum.lg | rs | debug | 10 | 0.3050 | 0.2900–0.3600 | 961806336 | 586144 | 31 |
| psum.lg | rs | release | 10 | 0.1450 | 0.0400–0.1700 | 961757184 | 524224 | 31 |
| psum.md | c | debug | 20 | 0.0300 | 0.0300–0.0300 | 321552384 | 33840 | 37 |
| psum.md | c | release | 20 | 0.0100 | 0.0100–0.0200 | 321552384 | 33840 | 37 |
| psum.md | cxx | debug | 20 | 0.0300 | 0.0300–0.0400 | 321552384 | 34272 | 38 |
| psum.md | cxx | release | 20 | 0.0100 | 0.0100–0.0200 | 321552384 | 33840 | 38 |
| psum.md | ours | debug | 20 | 0.0500 | 0.0500–0.0550 | 321585152 | 74928 | 40 |
| psum.md | ours | release | 20 | 0.0150 | 0.0100–0.0200 | 321585152 | 74920 | 40 |
| psum.md | rs | debug | 20 | 0.1000 | 0.1000–0.1100 | 321781760 | 586144 | 31 |
| psum.md | rs | release | 20 | 0.0150 | 0.0100–0.0300 | 321765376 | 524224 | 31 |
| psum.sm | c | debug | 30 | 0.0000 | 0.0000–0.0100 | 81559552 | 33840 | 37 |
| psum.sm | c | release | 30 | 0.0000 | 0.0000–0.0000 | 81575936 | 33840 | 37 |
| psum.sm | cxx | debug | 30 | 0.0000 | 0.0000–0.0100 | 81575936 | 34272 | 38 |
| psum.sm | cxx | release | 30 | 0.0000 | 0.0000–0.0000 | 81592320 | 33840 | 38 |
| psum.sm | ours | debug | 30 | 0.0100 | 0.0100–0.0100 | 81592320 | 74928 | 40 |
| psum.sm | ours | release | 30 | 0.0000 | 0.0000–0.0000 | 81592320 | 74920 | 40 |
| psum.sm | rs | debug | 30 | 0.0200 | 0.0200–0.0200 | 81788928 | 586144 | 31 |
| psum.sm | rs | release | 30 | 0.0000 | 0.0000–0.0000 | 81772544 | 524224 | 31 |

## paired ours − {lang} wall diff (median, 95% CI)

| workload | mode | vs | median diff s | 95% CI |
|---|---|---|---|---|
| prodcons_bchan_c256.md | debug | rs | -0.1700 | -0.1700–-0.1550 |
| prodcons_bchan_c1.sm | release | cxx | +0.0250 | -0.0200–+0.1000 |
| prodcons_bchan_c16.lg | debug | c | -0.3250 | -0.3950–-0.2000 |
| prodcons_bchan_c1.lg | debug | c | +1.8800 | +0.1700–+3.2600 |
| prodcons.sm | release | cxx | +0.0200 | +0.0200–+0.0200 |
| prodcons_bchan_c256.lg | debug | c | +0.1100 | +0.1050–+0.1200 |
| prodcons_bchan_c1.lg | release | cxx | +1.6800 | +0.5000–+2.3500 |
| prodcons.sm | debug | cxx | +0.0200 | +0.0200–+0.0200 |
| mapbench.lg | release | c | +0.0450 | -0.0600–+0.1400 |
| prodcons_bchan_c1.lg | release | rs | +2.2350 | +1.6200–+3.6000 |
| prodcons_bchan_c4096.sm | release | cxx | +0.0000 | +0.0000–+0.0000 |
| prodcons_bchan_c16.md | release | rs | +0.0200 | +0.0100–+0.0300 |
| prodcons_bchan_c16.md | debug | rs | -0.0600 | -0.0750–-0.0500 |
| prodcons_bchan_c4096.sm | release | rs | -0.0400 | -0.0400–-0.0400 |
| prodcons_bchan_c4096.lg | release | c | +0.0550 | +0.0400–+0.0800 |
| prodcons_bchan_c4096.md | debug | c | +0.0200 | +0.0200–+0.0200 |
| prodcons.md | debug | rs | -0.0200 | -0.0200–+0.0000 |
| prodcons_bchan_c256.sm | release | cxx | +0.0000 | +0.0000–+0.0000 |
| prodcons_bchan_c16.lg | debug | cxx | +0.0000 | -0.0600–+0.0500 |
| psum.sm | release | c | +0.0000 | +0.0000–+0.0000 |
| prodcons_bchan_c1.sm | release | rs | +0.0850 | +0.0300–+0.1200 |
| prodcons_bchan_c1.md | release | c | -0.0350 | -0.3400–+0.1650 |
| prodcons.md | debug | cxx | +0.0700 | +0.0600–+0.0700 |
| prodcons_bchan_c1.sm | debug | cxx | -0.0450 | -0.0850–+0.0000 |
| prodcons_bchan_c1.md | debug | c | +0.0450 | -0.3600–+0.2500 |
| psum.lg | debug | cxx | +0.0600 | +0.0350–+0.0700 |
| prodcons_bchan_c16.lg | release | cxx | +0.0550 | -0.0600–+0.2200 |
| prodcons_bchan_c4096.sm | debug | c | +0.0000 | +0.0000–+0.0000 |
| prodcons.lg | debug | c | +0.2850 | +0.2600–+0.3100 |
| psum.md | debug | cxx | +0.0200 | +0.0150–+0.0200 |
| prodcons_bchan_c4096.lg | debug | cxx | -0.1200 | -0.1400–-0.1000 |
| prodcons.md | release | rs | -0.0500 | -0.0600–-0.0300 |
| mapbench.md | release | c | +0.0100 | +0.0050–+0.0200 |
| mapbench.lg | release | cxx | +0.0350 | -0.0200–+0.1800 |
| mapbench.sm | release | cxx | +0.0000 | +0.0000–+0.0000 |
| prodcons.sm | debug | rs | +0.0000 | -0.0100–+0.0000 |
| psum.sm | release | cxx | +0.0000 | +0.0000–+0.0000 |
| prodcons_bchan_c16.lg | release | c | -0.1800 | -0.3200–-0.0800 |
| prodcons.lg | debug | rs | -0.0750 | -0.1050–-0.0100 |
| prodcons.lg | release | c | +0.3150 | +0.2950–+0.3300 |
| psum.lg | debug | rs | -0.1600 | -0.2000–-0.1300 |
| prodcons.sm | release | c | +0.0200 | +0.0200–+0.0200 |
| prodcons.lg | release | cxx | +0.3100 | +0.2900–+0.3300 |
| mapbench.sm | release | rs | +0.0000 | +0.0000–+0.0000 |
| prodcons_bchan_c4096.lg | debug | c | +0.0500 | +0.0400–+0.0650 |
| mapbench.md | debug | cxx | +0.2050 | +0.1850–+0.2200 |
| prodcons_bchan_c256.lg | release | cxx | +0.0400 | +0.0200–+0.0400 |
| prodcons_bchan_c16.sm | debug | c | -0.0100 | -0.0200–-0.0100 |
| prodcons_bchan_c16.md | release | c | -0.0500 | -0.0500–-0.0400 |
| prodcons_bchan_c1.sm | debug | c | +0.1050 | +0.0750–+0.1250 |
| prodcons_bchan_c16.sm | release | cxx | +0.0100 | +0.0000–+0.0100 |
| prodcons_bchan_c256.lg | release | rs | -0.7000 | -0.7400–-0.6600 |
| psum.sm | debug | cxx | +0.0100 | +0.0000–+0.0100 |
| prodcons.sm | debug | c | +0.0200 | +0.0200–+0.0200 |
| prodcons_bchan_c1.md | release | cxx | +0.0600 | -0.1000–+0.2150 |
| prodcons_bchan_c16.lg | debug | rs | -0.1400 | -0.1700–-0.1000 |
| mapbench.lg | debug | c | +0.6400 | +0.5400–+0.8200 |
| prodcons_bchan_c256.sm | release | rs | -0.0400 | -0.0500–-0.0400 |
| prodcons_bchan_c256.sm | debug | cxx | +0.0000 | +0.0000–+0.0000 |
| prodcons_bchan_c16.sm | release | c | +0.0000 | +0.0000–+0.0100 |
| psum.md | release | c | +0.0000 | +0.0000–+0.0000 |
| prodcons_bchan_c16.md | debug | cxx | +0.0000 | -0.0250–+0.0100 |
| prodcons_bchan_c16.sm | release | rs | +0.0050 | +0.0000–+0.0100 |
| prodcons_bchan_c1.md | debug | cxx | -0.7700 | -0.9350–-0.3050 |
| psum.lg | release | c | +0.0050 | -0.0100–+0.0700 |
| prodcons_bchan_c16.lg | release | rs | +0.0450 | -0.0400–+0.1100 |
| prodcons_bchan_c4096.md | release | c | +0.0200 | +0.0200–+0.0200 |
| psum.sm | release | rs | +0.0000 | +0.0000–+0.0000 |
| prodcons_bchan_c16.md | release | cxx | +0.0150 | -0.0050–+0.0300 |
| prodcons_bchan_c4096.md | debug | rs | -0.1550 | -0.1800–-0.1350 |
| prodcons_bchan_c16.sm | debug | cxx | +0.0000 | +0.0000–+0.0000 |
| mapbench.sm | debug | rs | -0.0800 | -0.0800–-0.0800 |
| prodcons_bchan_c1.lg | debug | rs | -1.0350 | -1.6350–+0.3500 |
| prodcons_bchan_c4096.md | release | cxx | +0.0000 | +0.0000–+0.0000 |
| prodcons_bchan_c4096.md | release | rs | -0.1700 | -0.1750–-0.1400 |
| psum.md | release | rs | +0.0000 | -0.0050–+0.0050 |
| prodcons_bchan_c256.md | release | rs | -0.1650 | -0.1750–-0.1500 |
| prodcons_bchan_c4096.md | debug | cxx | -0.0250 | -0.0300–-0.0200 |
| psum.lg | release | cxx | +0.0200 | -0.0251–+0.0900 |
| mapbench.md | release | rs | +0.0000 | +0.0000–+0.0150 |
| prodcons.md | release | cxx | +0.0800 | +0.0800–+0.0900 |
| prodcons_bchan_c256.sm | debug | c | +0.0100 | +0.0100–+0.0100 |
| prodcons_bchan_c1.lg | release | c | +1.0050 | -0.1300–+2.3600 |
| prodcons_bchan_c256.sm | debug | rs | -0.0400 | -0.0400–-0.0400 |
| mapbench.lg | debug | rs | -1.2950 | -1.4200–-1.1900 |
| prodcons_bchan_c1.lg | debug | cxx | -0.9650 | -2.1700–+0.8450 |
| mapbench.sm | release | c | +0.0000 | +0.0000–+0.0000 |
| prodcons_bchan_c16.md | debug | c | -0.0300 | -0.0350–-0.0050 |
| mapbench.md | debug | rs | -0.3900 | -0.3950–-0.3800 |
| prodcons_bchan_c1.md | release | rs | +0.4250 | +0.1100–+0.5450 |
| prodcons_bchan_c4096.sm | debug | rs | -0.0400 | -0.0500–-0.0400 |
| prodcons_bchan_c4096.sm | release | c | +0.0100 | +0.0100–+0.0100 |
| psum.md | debug | c | +0.0200 | +0.0200–+0.0200 |
| prodcons.lg | debug | cxx | +0.2800 | +0.2550–+0.3000 |
| prodcons.md | debug | c | +0.0700 | +0.0700–+0.0700 |
| prodcons_bchan_c1.sm | debug | rs | +0.0100 | -0.0250–+0.0400 |
| mapbench.sm | debug | c | +0.0500 | +0.0500–+0.0600 |
| prodcons_bchan_c4096.sm | debug | cxx | -0.0100 | -0.0100–-0.0100 |
| prodcons_bchan_c256.md | debug | cxx | -0.0300 | -0.0350–-0.0200 |
| prodcons_bchan_c4096.lg | release | cxx | +0.0200 | +0.0000–+0.0400 |
| prodcons.lg | release | rs | -0.2200 | -0.2600–-0.1750 |
| mapbench.md | release | cxx | +0.0100 | +0.0100–+0.0200 |
| prodcons_bchan_c256.lg | release | c | +0.1050 | +0.0800–+0.1150 |
| mapbench.lg | release | rs | -0.0050 | -0.2100–+0.1050 |
| psum.sm | debug | c | +0.0100 | +0.0000–+0.0100 |
| mapbench.lg | debug | cxx | +0.6900 | +0.5850–+0.8300 |
| prodcons_bchan_c4096.lg | release | rs | -0.7550 | -0.7850–-0.6600 |
| psum.md | debug | rs | -0.0500 | -0.0600–-0.0500 |
| mapbench.md | debug | c | +0.2200 | +0.1950–+0.2300 |
| prodcons.md | release | c | +0.0900 | +0.0800–+0.0900 |
| prodcons_bchan_c256.md | release | cxx | +0.0100 | +0.0000–+0.0100 |
| psum.lg | release | rs | -0.0150 | -0.0850–+0.0700 |
| prodcons_bchan_c1.sm | release | c | -0.0150 | -0.0350–+0.0250 |
| prodcons_bchan_c256.md | debug | c | +0.0200 | +0.0200–+0.0300 |
| prodcons_bchan_c256.lg | debug | rs | -0.6900 | -0.7100–-0.6700 |
| prodcons_bchan_c256.lg | debug | cxx | -0.1100 | -0.1150–-0.1000 |
| prodcons_bchan_c4096.lg | debug | rs | -0.7050 | -0.7800–-0.5700 |
| psum.md | release | cxx | +0.0000 | +0.0000–+0.0100 |
| psum.sm | debug | rs | -0.0100 | -0.0100–-0.0100 |
| prodcons.sm | release | rs | -0.0100 | -0.0100–-0.0100 |
| prodcons_bchan_c16.sm | debug | rs | -0.0100 | -0.0100–-0.0100 |
| prodcons_bchan_c1.md | debug | rs | -0.9100 | -1.0550–-0.5250 |
| psum.lg | debug | c | +0.0550 | -0.0150–+0.0850 |
| prodcons_bchan_c256.sm | release | c | +0.0000 | +0.0000–+0.0000 |
| mapbench.sm | debug | cxx | +0.0500 | +0.0500–+0.0600 |
| prodcons_bchan_c256.md | release | c | +0.0250 | +0.0200–+0.0300 |
| httpsrv.lg | release | rs | -0.0160 | -0.0454–+0.0014 |
| kvsrv.md | debug | cxx | +0.0000 | -0.0022–+0.0020 |
| httpsrv.lg | release | cxx | -0.0018 | -0.0176–+0.0053 |
| httpsrv.md | release | rs | -0.0007 | -0.0052–+0.0006 |
| httpsrv.md | debug | rs | +0.0019 | +0.0004–+0.0070 |
| httpsrv.lg | debug | cxx | +0.0110 | -0.0120–+0.0252 |
| httpsrv.sm | release | c | +0.0001 | -0.0002–+0.0008 |
| httpsrv.md | release | c | -0.0015 | -0.0040–+0.0007 |
| kvsrv.md | release | c | +0.0017 | +0.0000–+0.0035 |
| kvsrv.md | release | cxx | +0.0009 | -0.0009–+0.0040 |
| httpsrv.sm | debug | rs | +0.0000 | -0.0004–+0.0004 |
| kvsrv.lg | release | rs | +0.0084 | -0.0032–+0.0291 |
| kvsrv.sm | release | c | +0.0001 | -0.0004–+0.0004 |
| httpsrv.sm | release | cxx | +0.0000 | -0.0002–+0.0008 |
| kvsrv.md | release | rs | +0.0011 | -0.0052–+0.0036 |
| kvsrv.sm | release | cxx | +0.0003 | +0.0001–+0.0005 |
| kvsrv.md | debug | rs | -0.0007 | -0.0019–+0.0010 |
| httpsrv.md | debug | cxx | +0.0010 | -0.0013–+0.0037 |
| kvsrv.md | debug | c | -0.0004 | -0.0015–+0.0014 |
| kvsrv.sm | debug | cxx | +0.0000 | -0.0004–+0.0009 |
| httpsrv.lg | release | c | +0.0040 | -0.0042–+0.0198 |
| kvsrv.lg | release | c | -0.0097 | -0.0208–+0.0092 |
| httpsrv.sm | release | rs | +0.0003 | -0.0000–+0.0010 |
| kvsrv.sm | release | rs | -0.0001 | -0.0004–+0.0004 |
| httpsrv.md | release | cxx | +0.0004 | -0.0039–+0.0031 |
| kvsrv.lg | debug | c | +0.0196 | +0.0065–+0.0279 |
| httpsrv.lg | debug | c | +0.0014 | -0.0207–+0.0236 |
| httpsrv.sm | debug | c | +0.0001 | -0.0002–+0.0005 |
| kvsrv.sm | debug | c | -0.0001 | -0.0002–+0.0003 |
| kvsrv.sm | debug | rs | -0.0001 | -0.0004–+0.0002 |
| httpsrv.sm | debug | cxx | +0.0002 | -0.0003–+0.0005 |
| kvsrv.lg | debug | rs | +0.0115 | -0.0178–+0.0316 |
| httpsrv.lg | debug | rs | +0.0049 | -0.0075–+0.0157 |
| kvsrv.lg | debug | cxx | +0.0133 | -0.0076–+0.0265 |
| kvsrv.lg | release | cxx | -0.0066 | -0.0267–+0.0116 |
| httpsrv.md | debug | c | +0.0016 | -0.0001–+0.0040 |

## invalid cells (excluded above)

| workload | lang | mode | reason | n |
|---|---|---|---|---|
| fcopy.lg | c | debug | run-failed | 10 |
| fcopy.lg | c | release | run-failed | 10 |
| fcopy.lg | cxx | debug | run-failed | 10 |
| fcopy.lg | cxx | release | run-failed | 10 |
| fcopy.lg | ours | debug | run-failed | 10 |
| fcopy.lg | ours | release | run-failed | 10 |
| fcopy.lg | rs | debug | run-failed | 10 |
| fcopy.lg | rs | release | run-failed | 10 |
| fcopy.md | c | debug | run-failed | 20 |
| fcopy.md | c | release | run-failed | 20 |
| fcopy.md | cxx | debug | run-failed | 20 |
| fcopy.md | cxx | release | run-failed | 20 |
| fcopy.md | ours | debug | run-failed | 20 |
| fcopy.md | ours | release | run-failed | 20 |
| fcopy.md | rs | debug | run-failed | 20 |
| fcopy.md | rs | release | run-failed | 20 |
| fcopy.sm | c | debug | run-failed | 30 |
| fcopy.sm | c | release | run-failed | 30 |
| fcopy.sm | cxx | debug | run-failed | 30 |
| fcopy.sm | cxx | release | run-failed | 30 |
| fcopy.sm | ours | debug | run-failed | 30 |
| fcopy.sm | ours | release | run-failed | 30 |
| fcopy.sm | rs | debug | run-failed | 30 |
| fcopy.sm | rs | release | run-failed | 30 |
| jsonparse.lg | c | debug | run-failed | 10 |
| jsonparse.lg | c | release | run-failed | 10 |
| jsonparse.lg | cxx | debug | run-failed | 10 |
| jsonparse.lg | cxx | release | run-failed | 10 |
| jsonparse.lg | ours | debug | run-failed | 10 |
| jsonparse.lg | ours | release | run-failed | 10 |
| jsonparse.lg | rs | debug | run-failed | 10 |
| jsonparse.lg | rs | release | run-failed | 10 |
| jsonparse.md | c | debug | run-failed | 20 |
| jsonparse.md | c | release | run-failed | 20 |
| jsonparse.md | cxx | debug | run-failed | 20 |
| jsonparse.md | cxx | release | run-failed | 20 |
| jsonparse.md | ours | debug | run-failed | 20 |
| jsonparse.md | ours | release | run-failed | 20 |
| jsonparse.md | rs | debug | run-failed | 20 |
| jsonparse.md | rs | release | run-failed | 20 |
| jsonparse.sm | c | debug | run-failed | 30 |
| jsonparse.sm | c | release | run-failed | 30 |
| jsonparse.sm | cxx | debug | run-failed | 30 |
| jsonparse.sm | cxx | release | run-failed | 30 |
| jsonparse.sm | ours | debug | run-failed | 30 |
| jsonparse.sm | ours | release | run-failed | 30 |
| jsonparse.sm | rs | debug | run-failed | 30 |
| jsonparse.sm | rs | release | run-failed | 30 |
| wordfreq.lg | c | debug | run-failed | 10 |
| wordfreq.lg | c | release | run-failed | 10 |
| wordfreq.lg | cxx | debug | run-failed | 10 |
| wordfreq.lg | cxx | release | run-failed | 10 |
| wordfreq.lg | ours | debug | run-failed | 10 |
| wordfreq.lg | ours | release | run-failed | 10 |
| wordfreq.lg | rs | debug | run-failed | 10 |
| wordfreq.lg | rs | release | run-failed | 10 |
| wordfreq.md | c | debug | run-failed | 20 |
| wordfreq.md | c | release | run-failed | 20 |
| wordfreq.md | cxx | debug | run-failed | 20 |
| wordfreq.md | cxx | release | run-failed | 20 |
| wordfreq.md | ours | debug | run-failed | 20 |
| wordfreq.md | ours | release | run-failed | 20 |
| wordfreq.md | rs | debug | run-failed | 20 |
| wordfreq.md | rs | release | run-failed | 20 |
| wordfreq.sm | c | debug | run-failed | 30 |
| wordfreq.sm | c | release | run-failed | 30 |
| wordfreq.sm | cxx | debug | run-failed | 30 |
| wordfreq.sm | cxx | release | run-failed | 30 |
| wordfreq.sm | ours | debug | run-failed | 30 |
| wordfreq.sm | ours | release | run-failed | 30 |
| wordfreq.sm | rs | debug | run-failed | 30 |
| wordfreq.sm | rs | release | run-failed | 30 |
