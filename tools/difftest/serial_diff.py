#!/usr/bin/env python3
"""Compare machina serial output against QEMU reference."""
import sys


def load_text(path):
    with open(path, "r", errors="replace") as f:
        return f.read()


def diff_serial(ref_path, machina_path, stage=None):
    ref = load_text(ref_path)
    mach = load_text(machina_path)
    ref_lines = ref.splitlines()
    mach_lines = mach.splitlines()

    stage_markers = {
        "S0": "OpenSBI",
        "S1": "Linux version",
        "S2": "Linux version",
        "S3": "/init",
        "S4": None,
    }

    if stage and stage in stage_markers and stage_markers[stage]:
        marker = stage_markers[stage]
        cut = None
        for i in range(len(ref_lines)):
            if marker in ref_lines[i]:
                cut = i + 1
                break
        if cut:
            ref_lines = ref_lines[:cut]

    matched = 0
    for i in range(min(len(ref_lines), len(mach_lines))):
        if i < len(ref_lines):
            r = ref_lines[i] if i < len(ref_lines) else ""
            m = mach_lines[i] if i < len(mach_lines) else ""
        if r == m:
            matched += 1
        else:
            print(f"DIVERGENCE at line {i+1}:")
            print(f"  REF:      {repr(r)}")
            print(f"  MACHINA:  {repr(m)}")
            start = max(0, i - 3)
            print(f"  Context (ref):")
            for j in range(start, min(i + 3, len(ref_lines))):
                pfx = ">>>" if j == i else "   "
                print(f"    {pfx} {j+1}: {ref_lines[j]}")
            print(f"  Matched {matched}/{len(ref_lines)} lines")
            return False

    if len(mach_lines) < len(ref_lines):
        print(f"SHORT: machina {len(mach_lines)} lines, ref {len(ref_lines)}")
        for j in range(len(mach_lines),
                         min(len(mach_lines) + 5, len(ref_lines))):
            print(f"  Missing {j+1}: {ref_lines[j]}")
        print(f"  Matched {matched}/{len(ref_lines)} lines")
        return False
    if len(mach_lines) > len(ref_lines) and stage:
        print(
            f"EXTRA: machina {len(mach_lines)} lines"
            f" vs ref {len(ref_lines)}"
        )
        print(f"  Matched {matched}/{len(ref_lines)} ref lines")
        for j in range(len(ref_lines), min(len(ref_lines) + 5, len(mach_lines))):
            print(f"  Extra {j+1}: {mach_lines[j]}")
        return True
    print(f"MATCH: {matched}/{len(ref_lines)} lines")
    return True


if __name__ == "__main__":
    if len(sys.argv) < 3:
        print(f"Usage: {sys.argv[0]} <ref.log> <machina.log> [S0|S1|S2|S3|S4]")
        sys.exit(1)
    stage = sys.argv[3] if len(sys.argv) > 3 else None
    ok = diff_serial(sys.argv[1], sys.argv[2], stage)
    sys.exit(0 if ok else 1)
