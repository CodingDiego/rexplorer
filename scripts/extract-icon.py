"""Copies the first icon group of a Windows binary into an .ico file, reading the PE resource tree directly.

    python -I scripts/extract-icon.py [source.exe] [dest.ico]

Defaults: Explorer's icon (C:\\Windows\\explorer.exe) -> crates/app/app.ico.
"""
import struct
import sys

RT_ICON, RT_GROUP_ICON = 3, 14


def sections(pe):
    pe_off = struct.unpack_from("<I", pe, 0x3C)[0]
    count, opt_size = struct.unpack_from("<HH", pe, pe_off + 6)[0], struct.unpack_from("<H", pe, pe_off + 20)[0]
    magic = struct.unpack_from("<H", pe, pe_off + 24)[0]
    dd = pe_off + 24 + (112 if magic == 0x20B else 96) + 8 * 2  # data directory 2 = resources
    rsrc_rva = struct.unpack_from("<I", pe, dd)[0]
    secs = [struct.unpack_from("<8sIIII", pe, pe_off + 24 + opt_size + 40 * i) for i in range(count)]
    return rsrc_rva, secs


def rva_to_off(rva, secs):
    for _, vsize, vaddr, rsize, raw in secs:
        if vaddr <= rva < vaddr + max(vsize, rsize):
            return rva - vaddr + raw
    sys.exit(f"RVA {rva:#x} is in no section")


def dir_entries(pe, base, off):
    n_named, n_id = struct.unpack_from("<HH", pe, base + off + 12)
    out = []
    for i in range(n_named + n_id):
        name, data = struct.unpack_from("<II", pe, base + off + 16 + 8 * i)
        out.append((name & 0x7FFFFFFF if name >> 31 == 0 else None, data & 0x7FFFFFFF, bool(data >> 31)))
    return out


def resource(pe, base, secs, rtype, name):
    """Bytes of the first language of resource (rtype, name)."""
    for t, off, is_dir in dir_entries(pe, base, 0):
        if t != rtype or not is_dir:
            continue
        for n, off2, is_dir2 in dir_entries(pe, base, off):
            if n != name or not is_dir2:
                continue
            _, leaf, _ = dir_entries(pe, base, off2)[0]
            rva, size = struct.unpack_from("<II", pe, base + leaf)
            o = rva_to_off(rva, secs)
            return pe[o : o + size]
    return None


def first_name(pe, base, rtype):
    for t, off, is_dir in dir_entries(pe, base, 0):
        if t == rtype and is_dir:
            return dir_entries(pe, base, off)[0][0]
    sys.exit(f"no resources of type {rtype}")


def main(src, dst):
    pe = open(src, "rb").read()
    rsrc_rva, secs = sections(pe)
    base = rva_to_off(rsrc_rva, secs)
    group_id = first_name(pe, base, RT_GROUP_ICON)
    grp = resource(pe, base, secs, RT_GROUP_ICON, group_id)
    count = struct.unpack_from("<HHH", grp, 0)[2]
    entries = [struct.unpack_from("<BBBBHHIH", grp, 6 + 14 * i) for i in range(count)]
    images = [resource(pe, base, secs, RT_ICON, e[7]) for e in entries]
    out = bytearray(struct.pack("<HHH", 0, 1, count))
    offset = 6 + 16 * count
    for e, img in zip(entries, images):
        out += struct.pack("<BBBBHHII", e[0], e[1], e[2], e[3], e[4], e[5], len(img), offset)
        offset += len(img)
    for img in images:
        out += img
    with open(dst, "wb") as f:
        f.write(out)
    print(f"{dst}: group {group_id}, {count} images, sizes {[e[0] or 256 for e in entries]}, {len(out)} bytes")


if __name__ == "__main__":
    main(sys.argv[1] if len(sys.argv) > 1 else r"C:\Windows\explorer.exe", sys.argv[2] if len(sys.argv) > 2 else "crates/app/app.ico")
