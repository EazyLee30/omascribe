#!/usr/bin/env python3
"""Sample a process tree without collecting command lines or meeting content."""

import json, pathlib, sys, time

root = int(sys.argv[1])
out = pathlib.Path(sys.argv[2])
started = time.monotonic()
with out.open("a") as stream:
    while pathlib.Path(f"/proc/{root}").exists():
        stats = {}
        for proc in pathlib.Path("/proc").glob("[0-9]*"):
            try:
                fields = dict(
                    line.split(":", 1)
                    for line in (proc / "status").read_text().splitlines()
                    if ":" in line
                )
                stats[int(proc.name)] = (int(fields["PPid"]), proc)
            except (OSError, ValueError, KeyError):
                pass
        selected = {root}
        while True:
            more = {pid for pid, (parent, _) in stats.items() if parent in selected}
            if more <= selected:
                break
            selected |= more
        pss = 0
        for pid in selected:
            try:
                pss += int(
                    next(
                        l
                        for l in pathlib.Path(f"/proc/{pid}/smaps_rollup")
                        .read_text()
                        .splitlines()
                        if l.startswith("Pss:")
                    ).split()[1]
                )
            except (OSError, StopIteration):
                pass
        memory = dict(
            line.split(":", 1)
            for line in pathlib.Path("/proc/meminfo").read_text().splitlines()
        )
        temps = []
        for temp in pathlib.Path("/sys/class/thermal").glob("thermal_zone*/temp"):
            try:
                temps.append(int(temp.read_text()) / 1000)
            except (OSError, ValueError):
                pass
        stream.write(
            json.dumps(
                dict(
                    elapsed_s=round(time.monotonic() - started, 1),
                    tree_pss_mib=round(pss / 1024, 1),
                    mem_available_mib=int(memory["MemAvailable"].split()[0]) // 1024,
                    max_thermal_c=max(temps, default=None),
                )
            )
            + "\n"
        )
        stream.flush()
        time.sleep(10)
