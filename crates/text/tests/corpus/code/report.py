#!/usr/bin/env python3
"""Summarise a CSV of orders: totals per customer."""

import csv
import sys
from collections import defaultdict


def totals(path):
    out = defaultdict(float)
    with open(path, newline="", encoding="utf-8") as f:
        for row in csv.DictReader(f):
            out[row["customer"]] += float(row["amount"])
    return dict(out)


if __name__ == "__main__":
    for name, total in sorted(totals(sys.argv[1]).items()):
        print(f"{name:<20} {total:>10.2f} €")
