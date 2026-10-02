#!/usr/bin/env python3
"""Print the distinct FIGs in a raw ETI(NI) file and write one sub-channel's bytes.

usage: eti_extract.py out.eti SUBCHID subchannel.bin
"""
import collections
import sys

FRAME = 6144


def main(path, subchid, out_path):
    data = open(path, "rb").read()
    figs = collections.defaultdict(set)
    msc = bytearray()
    for n in range(len(data) // FRAME):
        frame = data[n * FRAME:(n + 1) * FRAME]
        nst = frame[5] & 0x7F
        mid = (frame[6] >> 3) & 3
        pos = 8
        streams = []
        for _ in range(nst):
            stc = frame[pos:pos + 4]
            streams.append((stc[0] >> 2, (((stc[2] & 3) << 8) | stc[3]) * 8))
            pos += 4
        pos += 4  # EOH
        ficl = 128 if mid == 3 else 96
        fic = frame[pos:pos + ficl]
        pos += ficl
        for fib in (fic[i:i + 32] for i in range(0, ficl, 32)):
            q = 0
            while q < 30 and fib[q] != 0xFF:
                header = fib[q]
                length = header & 0x1F
                body = fib[q + 1:q + 1 + length]
                kind = header >> 5
                ext = body[0] & (0x1F if kind == 0 else 0x07)
                if (kind, ext) not in ((0, 0), (0, 10)):  # time-dependent
                    figs[f"{kind}/{ext}"].add((bytes([header]) + body).hex())
                q += 1 + length
        for scid, length in streams:
            if scid == subchid:
                msc += frame[pos:pos + length]
            pos += length
    for key in sorted(figs, key=lambda k: tuple(map(int, k.split("/")))):
        print(f"FIG {key}: {' '.join(sorted(figs[key]))}")
    open(out_path, "wb").write(msc)
    print(f"sub-channel {subchid}: {len(msc)} bytes")


if __name__ == "__main__":
    main(sys.argv[1], int(sys.argv[2]), sys.argv[3])
