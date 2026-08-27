#!/usr/bin/env python3
"""THROWAWAY (spike #679). Attribute a core wasm module's code-section bytes to
crates, using the function-name subsection of the `name` custom section.

Usage: python3 spike679_attribute.py <core-module.wasm>
"""
import sys
from collections import defaultdict


def leb(buf, i):
    """Unsigned LEB128 -> (value, next_index)."""
    result = 0
    shift = 0
    while True:
        b = buf[i]
        i += 1
        result |= (b & 0x7F) << shift
        if not (b & 0x80):
            return result, i
        shift += 7


def sections(buf):
    i = 8  # magic + version
    n = len(buf)
    while i < n:
        sid = buf[i]
        i += 1
        size, i = leb(buf, i)
        yield sid, i, size
        i += size


def parse(path):
    buf = open(path, "rb").read()

    imported_funcs = 0
    code_bodies = []      # (func_index, body_bytes)
    names = {}            # func_index -> name

    for sid, off, size in sections(buf):
        body = memoryview(buf)[off:off + size]

        if sid == 2:  # import section — count imported funcs (they shift indices)
            i = 0
            count, i = leb(body, i)
            for _ in range(count):
                mlen, i = leb(body, i); i += mlen
                nlen, i = leb(body, i); i += nlen
                kind = body[i]; i += 1
                if kind == 0x00:
                    _, i = leb(body, i)
                    imported_funcs += 1
                elif kind == 0x01:      # table
                    i += 1
                    flags = body[i - 1]
                    lim, i = leb(body, i)
                    if flags & 0x01:
                        _, i = leb(body, i)
                elif kind == 0x02:      # memory
                    flags = body[i]; i += 1
                    _, i = leb(body, i)
                    if flags & 0x01:
                        _, i = leb(body, i)
                elif kind == 0x03:      # global
                    i += 2
                else:
                    raise SystemExit(f"unhandled import kind {kind}")

        elif sid == 10:  # code section
            i = 0
            count, i = leb(body, i)
            for k in range(count):
                blen, i = leb(body, i)
                code_bodies.append((imported_funcs + k, blen))
                i += blen

        elif sid == 0:  # custom
            i = 0
            nlen, i = leb(body, i)
            cname = bytes(body[i:i + nlen]).decode()
            i += nlen
            if cname == "name":
                while i < len(body):
                    sub = body[i]; i += 1
                    sublen, i = leb(body, i)
                    end = i + sublen
                    if sub == 1:  # function names
                        j = i
                        cnt, j = leb(body, j)
                        for _ in range(cnt):
                            idx, j = leb(body, j)
                            ln, j = leb(body, j)
                            names[idx] = bytes(body[j:j + ln]).decode(errors="replace")
                            j += ln
                    i = end

    return code_bodies, names


import re

# Rust v0 mangling encodes a crate root as `Cs<disambiguator>_<len><name>`.
# The FIRST such element in a symbol is the crate that DEFINES the function
# (later ones are generic arguments), so attributing to the first occurrence
# gives "which crate's code is this", not "which crate is mentioned".
_CRATE_RE = re.compile(r"Cs[0-9a-zA-Z]+_(\d+)")


def defining_crate(name):
    m = _CRATE_RE.search(name)
    if not m:
        # Legacy `_ZN<len><crate>...` mangling fallback.
        m2 = re.match(r"_ZN(\d+)", name)
        if m2:
            ln = int(m2.group(1))
            start = m2.end()
            return name[start:start + ln]
        return "unmangled/other"
    ln = int(m.group(1))
    start = m.end()
    return name[start:start + ln] or "unmangled/other"


def bucket(name):
    return defining_crate(name)


def main():
    path = sys.argv[1]
    bodies, names = parse(path)
    total = sum(b for _, b in bodies)

    by = defaultdict(int)
    cnt = defaultdict(int)
    for idx, blen in bodies:
        b = bucket(names.get(idx, ""))
        by[b] += blen
        cnt[b] += 1

    print(f"core module: {path}")
    print(f"code section total: {total} B ({total/1024:.1f} KiB) across {len(bodies)} functions\n")
    print("| origin | funcs | code B | KiB | % of code |")
    print("|---|--:|--:|--:|--:|")
    for k in sorted(by, key=lambda k: -by[k]):
        print(f"| {k} | {cnt[k]} | {by[k]} | {by[k]/1024:.1f} | {100*by[k]/total:.1f}% |")

    # The specific monomorphisations the `complete` boundary forces.
    print("\n-- functions whose name mentions CompletionRequest/CompletionResponse --")
    hit = 0
    hitc = 0
    for idx, blen in bodies:
        n = names.get(idx, "")
        if "CompletionRequest" in n or "CompletionResponse" in n:
            hit += blen
            hitc += 1
    print(f"{hitc} functions, {hit} B ({hit/1024:.1f} KiB, {100*hit/total:.2f}% of code)")


main()
