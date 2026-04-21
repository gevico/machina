#!/usr/bin/env python3
"""Compare two serial log files and report differences.

Usage: serial_diff.py <reference.log> <actual.log>

Outputs matching/diverging sections with line numbers.
Exit code 0 if logs match, 1 if they diverge.
"""

import sys


def load_lines(path):
    with open(path, "r", errors="replace") as f:
        return [line.rstrip("\n\r") for line in f]


def main():
    if len(sys.argv) != 3:
        print(f"Usage: {sys.argv[0]} <reference> <actual>")
        sys.exit(2)

    ref_lines = load_lines(sys.argv[1])
    act_lines = load_lines(sys.argv[2])

    min_len = min(len(ref_lines), len(act_lines))
    first_diff = None

    for i in range(min_len):
        if ref_lines[i] != act_lines[i]:
            first_diff = i
            break

    if first_diff is None and len(ref_lines) == len(act_lines):
        print(f"MATCH: {len(ref_lines)} lines identical")
        sys.exit(0)

    if first_diff is None:
        first_diff = min_len

    print(f"MATCH: lines 1-{first_diff} identical")

    if first_diff < min_len:
        print(f"DIVERGE at line {first_diff + 1}:")
        print(f"  ref: {ref_lines[first_diff]!r}")
        print(f"  act: {act_lines[first_diff]!r}")
    elif len(act_lines) < len(ref_lines):
        print(
            f"SHORT: actual has {len(act_lines)} lines, "
            f"reference has {len(ref_lines)} lines"
        )
    else:
        print(
            f"EXTRA: actual has {len(act_lines)} lines, "
            f"reference has {len(ref_lines)} lines"
        )

    sys.exit(1)


if __name__ == "__main__":
    main()
