#!/usr/bin/env python3
import array
import hashlib
from pathlib import Path
import struct
import sys
import urllib.request
import zlib

REVISION = "6fd20aad4a7905b13f25389c68368a942a2b68cb"
BLOBS = (
    "57793e37677c3b319b7df15e13ffc98d9296bfc5",
    "a94d3f70ac2056645bec91ea253bf82a0bf2f004",
    "239b93d32081a9fa1688205dee330c6fd4c0e403",
)


def convert(raw):
    if raw[:1] != b"\x02":
        raise ValueError("expected FSoD WMAP v2")
    decoder = zlib.decompressobj()
    data = decoder.decompress(raw[1:], 16 * 1024 * 1024 + 1)
    if len(data) > 16 * 1024 * 1024 or not decoder.eof or decoder.unused_data:
        raise ValueError("invalid or oversized compressed Realm")
    offset = 0

    def number(fmt):
        nonlocal offset
        value = struct.unpack_from(fmt, data, offset)[0]
        offset += struct.calcsize(fmt)
        return value

    def text():
        nonlocal offset
        length = 0
        for shift in range(0, 35, 7):
            byte = number("<B")
            length |= (byte & 127) << shift
            if byte < 128:
                break
        else:
            raise ValueError("map string length overflow")
        if offset + length > len(data):
            raise ValueError("truncated map string")
        value = data[offset:offset + length].decode("utf-8")
        offset += length
        return value

    count = number("<h")
    if count <= 0:
        raise ValueError("empty map dictionary")
    dictionary = []
    for _ in range(count):
        ground = number("<h")
        if not 0 <= ground <= 255:
            raise ValueError("ground does not fit the selected client")
        dictionary.append((ground, text(), text(), number("<B"), number("<B")))
    if (number("<i"), number("<i")) != (2048, 2048):
        raise ValueError("expected a full 2048-square Realm")
    payload = data[offset:]
    if len(payload) != 2048 * 2048 * 3:
        raise ValueError("truncated or trailing Realm cells")
    cells = bytearray(2048 * 2048 * 2)
    cells[0::2], cells[1::2] = payload[0::3], payload[1::3]
    indices = array.array("H", cells)
    if sys.byteorder != "little":
        indices.byteswap()
    if max(indices) >= count:
        raise ValueError("invalid map dictionary index")
    from realm_roads import round_roads
    cells = round_roads(cells, dictionary, 2048, 2048)
    return zlib.compress(data[:offset] + cells, 9)


def main():
    destination = Path(__file__).resolve().parents[1] / "data"
    for index, blob in enumerate(BLOBS, 1):
        url = f"https://raw.githubusercontent.com/ossimc82/fabiano-swagger-of-doom/{REVISION}/wServer/realm/worlds/maps/world{index}.wmap"
        with urllib.request.urlopen(url, timeout=60) as response:
            raw = response.read(2 * 1024 * 1024 + 1)
        actual = hashlib.sha1(f"blob {len(raw)}\0".encode() + raw).hexdigest()
        if actual != blob:
            raise ValueError(f"world{index}: source blob mismatch: {actual}")
        converted = convert(raw)
        path = destination / f"realm-fsod-{index}.wmap"
        path.write_bytes(converted)
        print(path.name, hashlib.sha256(converted).hexdigest())


if __name__ == "__main__":
    main()
