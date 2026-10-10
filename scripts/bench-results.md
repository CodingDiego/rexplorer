Machine: 12th Gen Intel(R) Core(TM) i5-12450H, 12 logical cores, 32 GB RAM, Windows 10.0.26200; Explorer 10.0.26100.8875 (WinBuild.160101.0800); rexplorer.exe built 2026-10-10 00:55 (1636 KB); run 2026-10-10, 7 runs per cell after 1 warm-up, median (min-max).

| Metric | rexplorer | Windows Explorer | rexplorer / Explorer |
|---|---|---|---|
| First window visible, 200-file folder (ms) | 153 (141-168) | 337 (316-782) | 0.45x |
| Open a 200-file folder (ms) | 226 (214-240) | 899 (866-954) | 0.25x |
| Open a 1,000-file folder (ms) | 243 (214-266) | 873 (858-927) | 0.28x |
| Open a 20,000-file folder (ms) | 421 (419-452) | 925 (783-947) | 0.45x |
| Navigate 200 -> 5000 files in the open window (ms) | 65 (64-70) | 166 (160-179) | 0.39x |
| Private working set after opening 200 files (MB) | 5.3 (5.1-5.3) | 100.8 (100.3-101.4) | 0.05x |
| Private working set after opening 1,000 files (MB) | 5.4 (5.4-5.5) | 100.8 (100.4-101.1) | 0.05x |
| Private working set after opening 20,000 files (MB) | 8.3 (7.9-8.6) | 105.9 (105.4-106.5) | 0.08x |

Explorer windows hosted by a new explorer.exe process (memory = that whole process): 21 of 21 measured opens; the rest are the growth of an existing process.
