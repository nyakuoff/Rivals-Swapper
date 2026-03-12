"""
UAsset Name-Map Patcher.

Performs safe, targeted renaming of FName entries inside .uasset files
by modifying the Name Map region, FolderName header, and export-table
FName Number fields.  All other data (imports, property blobs, bulk
data) is left untouched.

How it works
────────────
UAsset files store all symbolic names (class names, object names, path
segments, etc.) in a flat string table called the "Name Map".  Every
reference in the rest of the file is an integer *index* into this table.
By rewriting just the strings in the table we can rename objects without
corrupting type metadata, property offsets, or any other structural data.

Requirements for safe patching:
  • The replacement string MUST be the same byte-length as the original.
    (Our skin IDs are always 7-digit ASCII, so this is guaranteed.)

Binary layout refresher (little-endian):
  Header … NameCount (int32) … NameOffset (int32) …
  At NameOffset:
    For each of NameCount entries:
      FString: int32 length  (positive → ASCII+null, negative → UTF-16)
               byte[abs(length)] string data  (includes null terminator)
      uint32  hashes          (lower 16 = Strihash, upper 16 = StrCrc32)
"""

from __future__ import annotations

import struct
from pathlib import Path
from typing import Optional

# ──────────────────────────────────────────────────────────────────────
# CRC tables – ported from UAssetAPI/CRCGenerator.cs
# ──────────────────────────────────────────────────────────────────────

# Strihash_DEPRECATED CRC table (256 entries, same as UE source)
_CRC_TABLE_DEPRECATED: list[int] = [
    0x00000000, 0x04C11DB7, 0x09823B6E, 0x0D4326D9, 0x130476DC, 0x17C56B6B, 0x1A864DB2, 0x1E475005,
    0x2608EDB8, 0x22C9F00F, 0x2F8AD6D6, 0x2B4BCB61, 0x350C9B64, 0x31CD86D3, 0x3C8EA00A, 0x384FBDBD,
    0x4C11DB70, 0x48D0C6C7, 0x4593E01E, 0x4152FDA9, 0x5F15ADAC, 0x5BD4B01B, 0x569796C2, 0x52568B75,
    0x6A1936C8, 0x6ED82B7F, 0x639B0DA6, 0x675A1011, 0x791D4014, 0x7DDC5DA3, 0x709F7B7A, 0x745E66CD,
    0x9823B6E0, 0x9CE2AB57, 0x91A18D8E, 0x95609039, 0x8B27C03C, 0x8FE6DD8B, 0x82A5FB52, 0x8664E6E5,
    0xBE2B5B58, 0xBAEA46EF, 0xB7A96036, 0xB3687D81, 0xAD2F2D84, 0xA9EE3033, 0xA4AD16EA, 0xA06C0B5D,
    0xD4326D90, 0xD0F37027, 0xDDB056FE, 0xD9714B49, 0xC7361B4C, 0xC3F706FB, 0xCEB42022, 0xCA753D95,
    0xF23A8028, 0xF6FB9D9F, 0xFBB8BB46, 0xFF79A6F1, 0xE13EF6F4, 0xE5FFEB43, 0xE8BCCD9A, 0xEC7DD02D,
    0x34867077, 0x30476DC0, 0x3D044B19, 0x39C556AE, 0x278206AB, 0x23431B1C, 0x2E003DC5, 0x2AC12072,
    0x128E9DCF, 0x164F8078, 0x1B0CA6A1, 0x1FCDBB16, 0x018AEB13, 0x054BF6A4, 0x0808D07D, 0x0CC9CDCA,
    0x7897AB07, 0x7C56B6B0, 0x71159069, 0x75D48DDE, 0x6B93DDDB, 0x6F52C06C, 0x6211E6B5, 0x66D0FB02,
    0x5E9F46BF, 0x5A5E5B08, 0x571D7DD1, 0x53DC6066, 0x4D9B3063, 0x495A2DD4, 0x44190B0D, 0x40D816BA,
    0xACA5C697, 0xA864DB20, 0xA527FDF9, 0xA1E6E04E, 0xBFA1B04B, 0xBB60ADFC, 0xB6238B25, 0xB2E29692,
    0x8AAD2B2F, 0x8E6C3698, 0x832F1041, 0x87EE0DF6, 0x99A95DF3, 0x9D684044, 0x902B669D, 0x94EA7B2A,
    0xE0B41DE7, 0xE4750050, 0xE9362689, 0xEDF73B3E, 0xF3B06B3B, 0xF771768C, 0xFA325055, 0xFEF34DE2,
    0xC6BCF05F, 0xC27DEDE8, 0xCF3ECB31, 0xCBFFD686, 0xD5B88683, 0xD1799B34, 0xDC3ABDED, 0xD8FBA05A,
    0x690CE0EE, 0x6DCDFD59, 0x608EDB80, 0x644FC637, 0x7A089632, 0x7EC98B85, 0x738AAD5C, 0x774BB0EB,
    0x4F040D56, 0x4BC510E1, 0x46863638, 0x42472B8F, 0x5C007B8A, 0x58C1663D, 0x558240E4, 0x51435D53,
    0x251D3B9E, 0x21DC2629, 0x2C9F00F0, 0x285E1D47, 0x36194D42, 0x32D850F5, 0x3F9B762C, 0x3B5A6B9B,
    0x0315D626, 0x07D4CB91, 0x0A97ED48, 0x0E56F0FF, 0x1011A0FA, 0x14D0BD4D, 0x19939B94, 0x1D528623,
    0xF12F560E, 0xF5EE4BB9, 0xF8AD6D60, 0xFC6C70D7, 0xE22B20D2, 0xE6EA3D65, 0xEBA91BBC, 0xEF68060B,
    0xD727BBB6, 0xD3E6A601, 0xDEA580D8, 0xDA649D6F, 0xC423CD6A, 0xC0E2D0DD, 0xCDA1F604, 0xC960EBB3,
    0xBD3E8D7E, 0xB9FF90C9, 0xB4BCB610, 0xB07DABA7, 0xAE3AFBA2, 0xAAFBE615, 0xA7B8C0CC, 0xA379DD7B,
    0x9B3660C6, 0x9FF77D71, 0x92B45BA8, 0x9675461F, 0x8832161A, 0x8CF30BAD, 0x81B02D74, 0x857130C3,
    0x5D8A9099, 0x594B8D2E, 0x5408ABF7, 0x50C9B640, 0x4E8EE645, 0x4A4FFBF2, 0x470CDD2B, 0x43CDC09C,
    0x7B827D21, 0x7F436096, 0x7200464F, 0x76C15BF8, 0x68860BFD, 0x6C47164A, 0x61043093, 0x65C52D24,
    0x119B4BE9, 0x155A565E, 0x18197087, 0x1CD86D30, 0x029F3D35, 0x065E2082, 0x0B1D065B, 0x0FDC1BEC,
    0x3793A651, 0x3352BBE6, 0x3E119D3F, 0x3AD08088, 0x2497D08D, 0x2056CD3A, 0x2D15EBE3, 0x29D4F654,
    0xC5A92679, 0xC1683BCE, 0xCC2B1D17, 0xC8EA00A0, 0xD6AD50A5, 0xD26C4D12, 0xDF2F6BCB, 0xDBEE767C,
    0xE3A1CBC1, 0xE760D676, 0xEA23F0AF, 0xEEE2ED18, 0xF0A5BD1D, 0xF464A0AA, 0xF9278673, 0xFDE69BC4,
    0x89B8FD09, 0x8D79E0BE, 0x803AC667, 0x84FBDBD0, 0x9ABC8BD5, 0x9E7D9662, 0x933EB0BB, 0x97FFAD0C,
    0xAFB010B1, 0xAB710D06, 0xA6322BDF, 0xA2F33668, 0xBCB4666D, 0xB8757BDA, 0xB5365D03, 0xB1F740B4,
]

# StrCrc32 table (CRCTablesSB8[0], 256 entries)
_CRC_TABLES_SB8_0: list[int] = [
    0x00000000, 0x77073096, 0xEE0E612C, 0x990951BA, 0x076DC419, 0x706AF48F, 0xE963A535, 0x9E6495A3,
    0x0EDB8832, 0x79DCB8A4, 0xE0D5E91B, 0x97D2D988, 0x09B64C2B, 0x7EB17CBD, 0xE7B82D09, 0x90BF1D3F,
    0x1DB71064, 0x6AB020F2, 0xF3B97148, 0x84BE41DE, 0x1ADAD47D, 0x6DDDE4EB, 0xF4D4B551, 0x83D385C7,
    0x136C9856, 0x646BA8C0, 0xFD62F97A, 0x8A65C9EC, 0x14015C4F, 0x63066CD9, 0xFA0F3D63, 0x8D080DF5,
    0x3B6E20C8, 0x4C69105E, 0xD56041E4, 0xA2677172, 0x3C03E4D1, 0x4B04D447, 0xD20D85FD, 0xA50AB56B,
    0x35B5A8FA, 0x42B2986C, 0xDBBBC9D6, 0xACBCF940, 0x32D86CE3, 0x45DF5C75, 0xDCD60DCF, 0xABD13D59,
    0x26D930AC, 0x51DE003A, 0xC8D75180, 0xBFD06116, 0x21B4F4B5, 0x56B3C423, 0xCFBA9599, 0xB8BDA50F,
    0x2802B89E, 0x5F058808, 0xC60CD9B2, 0xB10BE924, 0x2F6F7C87, 0x58684C11, 0xC1611DAB, 0xB6662D3D,
    0x76DC4190, 0x01DB7106, 0x98D220BC, 0xEFD5102A, 0x71B18589, 0x06B6B51F, 0x9FBFE4A5, 0xE8B8D433,
    0x7807C9A2, 0x0F00F934, 0x9609A88E, 0xE10E9818, 0x7F6A0DBB, 0x086D3D2D, 0x91646C97, 0xE6635C01,
    0x6B6B51F4, 0x1C6C6162, 0x856530D8, 0xF262004E, 0x6C0695ED, 0x1B01A57B, 0x8208F4C1, 0xF50FC457,
    0x65B0D9C6, 0x12B7E950, 0x8BBEB8EA, 0xFCB9887C, 0x62DD1DDF, 0x15DA2D49, 0x8CD37CF3, 0xFBD44C65,
    0x4DB26158, 0x3AB551CE, 0xA3BC0074, 0xD4BB30E2, 0x4ADFA541, 0x3DD895D7, 0xA4D1C46D, 0xD3D6F4FB,
    0x4369E96A, 0x346ED9FC, 0xAD678846, 0xDA60B8D0, 0x44042D73, 0x33031DE5, 0xAA0A4C5F, 0xDD0D7AC9,
    0x5005713C, 0x270241AA, 0xBE0B1010, 0xC90C2086, 0x5768B525, 0x206F85B3, 0xB966D409, 0xCE61E49F,
    0x5EDEF90E, 0x29D9C998, 0xB0D09822, 0xC7D7A8B4, 0x59B33D17, 0x2EB40D81, 0xB7BD5C3B, 0xC0BA6CAD,
    0xEDB88320, 0x9ABFB3B6, 0x03B6E20C, 0x74B1D29A, 0xEAD54739, 0x9DD277AF, 0x04DB2615, 0x73DC1683,
    0xE3630B12, 0x94643B84, 0x0D6D6A3E, 0x7A6A5AA8, 0xE40ECF0B, 0x9309FF9D, 0x0A00AE27, 0x7D079EB1,
    0xF00F9344, 0x8708A3D2, 0x1E01F268, 0x6906C2FE, 0xF762575D, 0x806567CB, 0x196C3671, 0x6E6B06E7,
    0xFED41B76, 0x89D32BE0, 0x10DA7A5A, 0x67DD4ACC, 0xF9B9DF6F, 0x8EBEEFF9, 0x17B7BE43, 0x60B08ED5,
    0xD6D6A3E8, 0xA1D1937E, 0x38D8C2C4, 0x4FDFF252, 0xD1BB67F1, 0xA6BC5767, 0x3FB506DD, 0x48B2364B,
    0xD80D2BDA, 0xAF0A1B4C, 0x36034AF6, 0x41047A60, 0xDF60EFC3, 0xA8670955, 0x316E58EF, 0x4669BE79,
    0xCB61B38C, 0xBC66831A, 0x256FD2A0, 0x5268E236, 0xCC0C7795, 0xBB0B4703, 0x220216B9, 0x5505262F,
    0xC5BA3BBE, 0xB2BD0B28, 0x2BB45A92, 0x5CB36A04, 0xC2D7FFA7, 0xB5D0CF31, 0x2CD99E8B, 0x5BDEAE1D,
    0x9B64C2B0, 0xEC63F226, 0x756AA39C, 0x026D930A, 0x9C0906A9, 0xEB0E363F, 0x72076785, 0x05005713,
    0x95BF4A82, 0xE2B87A14, 0x7BB12BAE, 0x0CB61B38, 0x92D28E9B, 0xE5D5BE0D, 0x7CDCEFB7, 0x0BDBDF21,
    0x86D3D2D4, 0xF1D4E242, 0x68DDB3F8, 0x1FDA836E, 0x81BE16CD, 0xF6B9265B, 0x6FB077E1, 0x18B74777,
    0x88085AE6, 0xFF0F6B70, 0x66063BCA, 0x11010B5C, 0x8F659EFF, 0xF862AE69, 0x616BFFD3, 0x166CCF45,
    0xA00AE278, 0xD70DD2EE, 0x4E048354, 0x3903B3C2, 0xA7672661, 0xD06016F7, 0x4969474D, 0x3E6E77DB,
    0xAED16A4A, 0xD9D65ADC, 0x40DF0B66, 0x37D83BF0, 0xA9BCAE53, 0xDEBBBEC5, 0x47B2CF7F, 0x30B5FFE9,
    0xBDBDF21C, 0xCABAC28A, 0x53B39330, 0x24B4A3A6, 0xBAD03605, 0xCDD70693, 0x54DE5729, 0x23D967BF,
    0xB3667A2E, 0xC4614AB8, 0x5D681B02, 0x2A6F2B94, 0xB40BBE37, 0xC30C8EA1, 0x5A05DF1B, 0x2D02EF8D,
]


def _to_upper(ch: int) -> int:
    """UE-style uppercase for a single code-point (ASCII subset)."""
    if 0x61 <= ch <= 0x7A:  # a-z
        return ch - 0x20
    return ch


def _strihash_deprecated(text: str) -> int:
    """
    Port of UAssetAPI CRCGenerator.Strihash_DEPRECATED
    operating on UTF-8 encoded bytes of the uppercased string.
    """
    h: int = 0
    for ch in text:
        upper_ch = chr(_to_upper(ord(ch)))
        for b in upper_ch.encode("utf-8"):
            h = ((h >> 8) & 0x00FFFFFF) ^ _CRC_TABLE_DEPRECATED[(h ^ b) & 0xFF]
    return h & 0xFFFFFFFF


def _strcrc32(text: str) -> int:
    """
    Port of UAssetAPI CRCGenerator.StrCrc32
    operating on original (case-preserved) characters.
    Each char is processed as a 4-byte little-endian value, but since
    ASCII chars are < 256, only the low byte contributes.
    """
    crc: int = 0xFFFFFFFF
    for ch in text:
        c = ord(ch)
        crc = ((crc >> 8) & 0x00FFFFFF) ^ _CRC_TABLES_SB8_0[(crc ^ c) & 0xFF]
        c >>= 8
        crc = ((crc >> 8) & 0x00FFFFFF) ^ _CRC_TABLES_SB8_0[(crc ^ c) & 0xFF]
        c >>= 8
        crc = ((crc >> 8) & 0x00FFFFFF) ^ _CRC_TABLES_SB8_0[(crc ^ c) & 0xFF]
        c >>= 8
        crc = ((crc >> 8) & 0x00FFFFFF) ^ _CRC_TABLES_SB8_0[(crc ^ c) & 0xFF]
    return (~crc) & 0xFFFFFFFF


def generate_name_hash(name: str) -> int:
    """
    Compute the 4-byte name-map hash for a given string.

    Layout: lower 16 bits = Strihash_DEPRECATED (non-case-preserving)
            upper 16 bits = StrCrc32            (case-preserving)
    """
    alg1 = _strihash_deprecated(name)
    alg2 = _strcrc32(name)
    return (alg1 & 0xFFFF) | ((alg2 & 0xFFFF) << 16)


# ──────────────────────────────────────────────────────────────────────
# UAsset header helpers
# ──────────────────────────────────────────────────────────────────────

# Fixed-offset header field positions (standard UE4/5 cooked asset)
# The magic is 0x9E2A83C1, then version info follows.
# We only need NameCount and NameOffset which are at fixed offsets
# in the "package file summary".

_UASSET_MAGIC = 0x9E2A83C1


def _read_i32(data: bytes, offset: int) -> int:
    return struct.unpack_from("<i", data, offset)[0]


def _read_u32(data: bytes, offset: int) -> int:
    return struct.unpack_from("<I", data, offset)[0]


def _write_u32(data: bytearray, offset: int, value: int) -> None:
    struct.pack_into("<I", data, offset, value & 0xFFFFFFFF)


def _find_name_map_header(data: bytes) -> tuple[int, int]:
    """
    Parse the UAsset header to locate the Name Map.

    Returns (name_count, name_offset).
    Raises ValueError if the file is not a valid UAsset.

    The header layout varies slightly across UE versions, but NameCount
    and NameOffset always appear at the same position relative to fixed
    fields.  We scan for them by reading the known header structure.
    """
    return _parse_header(data)[:2]


def _parse_header(data: bytes) -> tuple[int, int, int, int]:
    """
    Parse the UAsset header to locate:
      1. Name Map (name_count, name_offset)
      2. FolderName FString (folder_name_offset, folder_name_length)

    Returns (name_count, name_offset, folder_string_offset, folder_string_length).
      - folder_string_offset: byte offset of the FString length field
      - folder_string_length: value of the FString length (positive=ASCII, negative=UTF-16)

    Raises ValueError if the file is not a valid UAsset.
    """
    full = _parse_header_full(data)
    return full[:4]


def _parse_header_full(data: bytes) -> tuple[int, int, int, int, int, int]:
    """
    Parse the UAsset header to locate:
      1. Name Map  (name_count, name_offset)
      2. FolderName FString (folder_name_offset, folder_name_length)
      3. Export Table (export_count, export_offset)

    Returns (name_count, name_offset, folder_string_offset,
             folder_string_length, export_count, export_offset).

    Raises ValueError if the file is not a valid UAsset.
    """
    magic = _read_u32(data, 0)
    if magic != _UASSET_MAGIC:
        raise ValueError(f"Not a UAsset file (magic=0x{magic:08X})")

    legacy_ver = _read_i32(data, 4)

    if legacy_ver >= 0:
        raise ValueError(f"Legacy package format (ver={legacy_ver}) not supported")

    offset = 0x08  # skip magic + LegacyFileVersion

    # LegacyUE3Version
    offset += 4

    # FileVersionUE4
    offset += 4

    # FileVersionLicenseeUE4
    offset += 4

    # UE5: if LegacyFileVersion <= -8, there's extra UE5 version info
    if legacy_ver <= -8:
        # FileVersionUE5
        offset += 4

    # Custom versions container
    num_custom_versions = _read_i32(data, offset)
    offset += 4
    if num_custom_versions < 0 or num_custom_versions > 1000:
        raise ValueError(f"Suspicious custom version count: {num_custom_versions}")
    offset += num_custom_versions * 20  # 16 byte GUID + 4 byte version

    # TotalHeaderSize (int32)
    offset += 4

    # FolderName (FString): int32 length + chars
    folder_string_offset = offset
    folder_name_len = _read_i32(data, offset)
    offset += 4
    if folder_name_len < 0:
        # UTF-16
        offset += (-folder_name_len) * 2
    elif folder_name_len > 0:
        offset += folder_name_len
    # else: 0 = null string, no data

    # PackageFlags (uint32)
    offset += 4

    # NameCount (int32)
    name_count = _read_i32(data, offset)
    offset += 4

    # NameOffset (int32)
    name_offset = _read_i32(data, offset)
    offset += 4

    if name_count <= 0 or name_count > 100000:
        raise ValueError(f"Suspicious NameCount: {name_count}")
    if name_offset <= 0 or name_offset > len(data):
        raise ValueError(f"NameOffset {name_offset} out of range")

    # SoftObjectPathsCount (int32) + SoftObjectPathsOffset (int32)
    offset += 4 + 4

    # GatherableTextDataCount (int32) + GatherableTextDataOffset (int32)
    offset += 4 + 4

    # ExportCount (int32)
    export_count = _read_i32(data, offset)
    offset += 4

    # ExportOffset (int32)
    export_offset = _read_i32(data, offset)

    return (name_count, name_offset, folder_string_offset,
            folder_name_len, export_count, export_offset)


def _parse_name_map(
    data: bytes,
    name_count: int,
    name_offset: int,
) -> list[tuple[str, int, int, int]]:
    """
    Parse the name map entries.

    Returns list of (name_string, entry_start_offset, string_data_offset, entry_end_offset).
      - entry_start_offset: byte offset of the int32 length field
      - string_data_offset: byte offset of the first string byte
      - entry_end_offset: byte offset past the 4-byte hash
    """
    entries: list[tuple[str, int, int, int]] = []
    pos = name_offset

    for _ in range(name_count):
        entry_start = pos
        length = _read_i32(data, pos)
        pos += 4

        string_data_start = pos

        if length < 0:
            # UTF-16 encoded
            byte_count = (-length) * 2
            raw = data[pos : pos + byte_count]
            name_str = raw[:-2].decode("utf-16-le", errors="replace")  # strip null
            pos += byte_count
        elif length > 0:
            raw = data[pos : pos + length]
            name_str = raw[:-1].decode("utf-8", errors="replace")  # strip null
            pos += length
        else:
            name_str = ""

        # Hash (uint32) — may or may not be present; for UE4.23+ it is.
        # We assume it is present (UE5 cooked assets always have it).
        # The hash follows immediately.
        hash_offset = pos
        pos += 4  # skip hash

        entry_end = pos
        entries.append((name_str, entry_start, string_data_start, entry_end))

    return entries


# ──────────────────────────────────────────────────────────────────────
# Public API
# ──────────────────────────────────────────────────────────────────────

def patch_name_map(
    uasset_path: Path,
    replacements: dict[str, str],
    output_path: Optional[Path] = None,
) -> list[str]:
    """
    Patch name-map entries in a .uasset file.

    For each (old_substring, new_substring) in *replacements*,
    any name-map entry containing *old_substring* will have it
    replaced with *new_substring*.  The hash is recalculated.

    BOTH substrings must be the same byte-length (ASCII).

    Args:
        uasset_path:   Path to the source .uasset file.
        replacements:  Mapping of old_substring → new_substring.
        output_path:   Where to write the patched file (default: overwrite in-place).

    Returns:
        List of name-map entries that were modified.

    Raises:
        ValueError if the file is not a valid UAsset or lengths mismatch.
    """
    if output_path is None:
        output_path = uasset_path

    # Validate replacement lengths
    for old, new in replacements.items():
        if len(old.encode("utf-8")) != len(new.encode("utf-8")):
            raise ValueError(
                f"Replacement length mismatch: {old!r} ({len(old.encode('utf-8'))}B) "
                f"→ {new!r} ({len(new.encode('utf-8'))}B)"
            )

    raw = uasset_path.read_bytes()
    buf = bytearray(raw)

    name_count, name_offset = _find_name_map_header(raw)
    entries = _parse_name_map(raw, name_count, name_offset)

    modified: list[str] = []

    for name_str, entry_start, string_data_start, entry_end in entries:
        new_name = name_str
        changed = False
        for old_sub, new_sub in replacements.items():
            if old_sub in new_name:
                new_name = new_name.replace(old_sub, new_sub)
                changed = True

        if not changed:
            continue

        # Read the original length field to determine encoding
        length = _read_i32(raw, entry_start)

        if length < 0:
            # UTF-16
            new_bytes = (new_name + "\x00").encode("utf-16-le")
        elif length > 0:
            # UTF-8 / ASCII
            new_bytes = (new_name + "\x00").encode("utf-8")
        else:
            continue  # empty string, nothing to patch

        expected_len = abs(length) * (2 if length < 0 else 1)
        if len(new_bytes) != expected_len:
            raise ValueError(
                f"Patched name {new_name!r} has different byte length "
                f"({len(new_bytes)}) than original {name_str!r} ({expected_len})"
            )

        # Write patched string bytes
        buf[string_data_start : string_data_start + len(new_bytes)] = new_bytes

        # Preserve the original hash value.
        # Cooked UE5 assets store all-zero hashes in the name map
        # because lookups use indices, not hashes.  Writing a
        # recalculated hash here would corrupt the asset.
        # (The hash field is left untouched.)

        modified.append(f"{name_str} → {new_name}")

    output_path.write_bytes(bytes(buf))
    return modified


def patch_childbp_uasset(
    uasset_path: Path,
    source_skin_id: str,
    target_skin_id: str,
    output_path: Optional[Path] = None,
) -> list[str]:
    """
    Patch a skin's ChildBP .uasset so it registers at the default
    skin's IoStore path, while preserving all VFX / weapon / mesh
    asset references pointing at their original source paths.

    A ChildBP's name map contains both:
      • Identity entries (class name, self-path, Default__ prefix) that
        must be rewritten so the asset loads at the default skin path.
      • Asset references (Niagara particle systems, VFX meshes, weapons)
        that MUST stay pointing at the source skin's paths because those
        assets already exist in the game's base paks.

    What gets patched:
      • FolderName FString in the package header
      • Name-map entries containing ``_ChildBP`` or ``_ShowBP`` or
        ``_LikeBP`` or ``_ShowAnimBP`` — these are the blueprint's own
        identity strings

    What is LEFT ALONE:
      • VFX references   (/VFX/Particles/…, NS_…, SM_Bundle_…)
      • Weapon references (/…/Weapons/…, SK_WP_…)
      • Mesh references   (/…/Meshes/SK_…)
      • All other entries
    """
    if output_path is None:
        output_path = uasset_path

    if len(source_skin_id.encode("utf-8")) != len(target_skin_id.encode("utf-8")):
        raise ValueError(
            f"Replacement length mismatch: {source_skin_id!r} → {target_skin_id!r}"
        )

    raw = uasset_path.read_bytes()
    buf = bytearray(raw)

    (name_count, name_offset, folder_str_offset, folder_str_len,
     export_count, export_offset) = _parse_header_full(raw)

    modified: list[str] = []

    # ── 1. Patch FolderName ─────────────────────────────────────────
    if folder_str_len != 0:
        data_start = folder_str_offset + 4
        if folder_str_len < 0:
            byte_count = (-folder_str_len) * 2
            folder_str = raw[data_start : data_start + byte_count - 2].decode(
                "utf-16-le", errors="replace"
            )
        else:
            byte_count = folder_str_len
            folder_str = raw[data_start : data_start + byte_count - 1].decode(
                "utf-8", errors="replace"
            )

        if source_skin_id in folder_str:
            new_folder = folder_str.replace(source_skin_id, target_skin_id)
            if folder_str_len < 0:
                new_bytes = (new_folder + "\x00").encode("utf-16-le")
            else:
                new_bytes = (new_folder + "\x00").encode("utf-8")
            if len(new_bytes) != byte_count:
                raise ValueError(
                    f"FolderName patch size mismatch: {len(new_bytes)} vs {byte_count}"
                )
            buf[data_start : data_start + byte_count] = new_bytes
            modified.append(f"FolderName: {folder_str} → {new_folder}")

    # ── 2. Patch ONLY blueprint identity entries in name map ────────
    #
    # Blueprint identity entries are those containing the blueprint
    # type suffix (e.g. _ChildBP, _ShowBP, _LikeBP, _ShowAnimBP).
    # The source skin's ChildBP file has the asset name as part of
    # its filename, e.g. "1055500_ChildBP".  All class references
    # also embed this pattern: "1055500_ChildBP_C",
    # "Default__1055500_ChildBP_C", and the full self-reference path.
    #
    # Crucially, VFX/weapon/mesh references do NOT contain "_ChildBP"
    # so this filter cleanly separates identity from asset references.

    bp_suffixes = ("_ChildBP", "_ShowBP", "_LikeBP", "_ShowAnimBP")

    entries = _parse_name_map(raw, name_count, name_offset)

    for name_str, entry_start, string_data_start, entry_end in entries:
        if source_skin_id not in name_str:
            continue

        # Only patch if the entry contains a blueprint identity suffix
        if not any(suffix in name_str for suffix in bp_suffixes):
            continue

        new_name = name_str.replace(source_skin_id, target_skin_id)

        length = _read_i32(raw, entry_start)

        if length < 0:
            new_bytes = (new_name + "\x00").encode("utf-16-le")
        elif length > 0:
            new_bytes = (new_name + "\x00").encode("utf-8")
        else:
            continue

        expected_len = abs(length) * (2 if length < 0 else 1)
        if len(new_bytes) != expected_len:
            raise ValueError(
                f"Patched name {new_name!r} has different byte length "
                f"({len(new_bytes)}) than original {name_str!r} ({expected_len})"
            )

        buf[string_data_start : string_data_start + len(new_bytes)] = new_bytes
        modified.append(f"{name_str} → {new_name}")

    output_path.write_bytes(bytes(buf))
    return modified


def _is_self_reference_path(name: str, source_skin_id: str) -> bool:
    """
    Determine whether a name-map entry is a "self-reference" that must
    be patched, vs an external import that should be left alone.

    We ONLY patch:
      • The asset's own path that goes through a /Meshes/, /Materials/,
        or /Textures/ directory under the skin's folder
        e.g. /Game/Marvel/Characters/1055/1055500/Meshes/SK_1055
        e.g. /Game/Marvel/Characters/1035/1035101/Materials/MI_1035101_Body
        e.g. /Game/Marvel/Characters/1035/1035101/Textures/T_1035101_Body_D
      • VFX self-reference paths — the asset's own path under /VFX/
        e.g. /Game/Marvel/VFX/Particles/Characters/1055/1055100/105531/NS_105531_Release_01
      • Export object names that embed the skin ID and start with a
        known asset prefix (SK_, SM_, MI_, M_, T_)
        e.g. SK_1055_1055500_Lobby, MI_1035101_Body, T_1035101_Body_D

    We do NOT patch:
      • Material import paths that reference a DIFFERENT skin's assets
        (only relevant when a mesh file imports MI_ from the source skin;
         but for material/texture files we DO patch their own self-ref)
      • Any other import paths (Physics, Skeleton, Blueprints)

    For VFX files the only name-map entries referencing the source skin
    are (1) the FolderName (handled separately) and (2) the self-reference
    full path.  Material/texture *imports* inside VFX files don't embed
    the source skin ID in their path — they point to shared assets — so
    they are never matched here.
    """
    if source_skin_id not in name:
        return False

    # Full paths containing the skin ID in known asset directories
    # are self-references for this package
    if "/" in name:
        for marker in ("/Meshes/", "/Materials/", "/Textures/",
                        "/Weapons/", "/VFX/"):
            if marker in name:
                return True
        # Also match folder-level paths like .../1035101/Materials
        # (the folder itself without a trailing filename)
        if name.endswith(("/Meshes", "/Materials", "/Textures")):
            return True

    # Short export names (no slash) that embed the skin ID
    # — SK_ covers character meshes & weapon meshes (SK_WP_*)
    # — SM_ covers VFX static meshes and weapon static meshes
    # — MI_ / M_ covers material instances
    # — T_ covers textures
    if "/" not in name:
        if name.startswith(("SK_", "SM_", "MI_", "M_", "T_")):
            return True
        # VFX particle short names
        if name.startswith(("NS_", "NE_")):
            return True

    return False


def patch_skin_id_in_uasset(
    uasset_path: Path,
    source_skin_id: str,
    target_skin_id: str,
    output_path: Optional[Path] = None,
) -> list[str]:
    """
    Patch the FolderName header, selective name-map entries, and export-
    table FName Numbers in a .uasset file so the asset registers at the
    *target* skin's IoStore path, while keeping material/texture import
    references pointing at the *source* skin's original assets.

    What gets patched:
      • FolderName FString in the package header (drives IoStore PackageId)
      • Self-reference paths under /Meshes/ in the name map
      • VFX self-reference paths under /VFX/ in the name map
      • Export object names that embed the skin ID (e.g. SK_1055_1055500_Lobby)
      • FName Number fields in the export table whose Number equals
        int(source_skin_id)+1  (UE5 FName instancing — the SkeletalMesh
        export uses base name "SK_XXXX" with Number=skin_id+1 so that
        the displayed name becomes "SK_XXXX_<skin_id>"; this also
        drives the PublicExportHash that the game uses for lookup)

    What is LEFT ALONE:
      • Material import paths  (/…/Materials/MI_1055500_Body)
      • Material short names   (MI_1055500_Body)
      • Any other import paths (Physics, Skeleton, Blueprints)

    This way the mesh keeps referencing its own materials & textures
    (which exist in the game's base paks for the source skin) instead
    of trying to load materials that belong to the default skin.
    """
    if output_path is None:
        output_path = uasset_path

    # Validate replacement lengths
    if len(source_skin_id.encode("utf-8")) != len(target_skin_id.encode("utf-8")):
        raise ValueError(
            f"Replacement length mismatch: {source_skin_id!r} → {target_skin_id!r}"
        )

    raw = uasset_path.read_bytes()
    buf = bytearray(raw)

    # Parse full header to get name map, FolderName, AND export table location
    (name_count, name_offset, folder_str_offset, folder_str_len,
     export_count, export_offset) = _parse_header_full(raw)

    modified: list[str] = []

    # ── 1. Patch FolderName FString in header ───────────────────────
    if folder_str_len != 0:
        # Read current FolderName string
        data_start = folder_str_offset + 4  # past the int32 length
        if folder_str_len < 0:
            byte_count = (-folder_str_len) * 2
            folder_str = raw[data_start : data_start + byte_count - 2].decode(
                "utf-16-le", errors="replace"
            )
        else:
            byte_count = folder_str_len
            folder_str = raw[data_start : data_start + byte_count - 1].decode(
                "utf-8", errors="replace"
            )

        if source_skin_id in folder_str:
            new_folder = folder_str.replace(source_skin_id, target_skin_id)
            if folder_str_len < 0:
                new_bytes = (new_folder + "\x00").encode("utf-16-le")
            else:
                new_bytes = (new_folder + "\x00").encode("utf-8")
            if len(new_bytes) != byte_count:
                raise ValueError(
                    f"FolderName patch size mismatch: {len(new_bytes)} vs {byte_count}"
                )
            buf[data_start : data_start + byte_count] = new_bytes
            modified.append(f"FolderName: {folder_str} → {new_folder}")

    # ── 2. Patch ONLY self-reference Name Map entries ───────────────
    entries = _parse_name_map(raw, name_count, name_offset)

    for name_str, entry_start, string_data_start, entry_end in entries:
        if source_skin_id not in name_str:
            continue

        # Only patch self-reference paths, not material imports
        if not _is_self_reference_path(name_str, source_skin_id):
            continue

        new_name = name_str.replace(source_skin_id, target_skin_id)

        length = _read_i32(raw, entry_start)

        if length < 0:
            new_bytes = (new_name + "\x00").encode("utf-16-le")
        elif length > 0:
            new_bytes = (new_name + "\x00").encode("utf-8")
        else:
            continue

        expected_len = abs(length) * (2 if length < 0 else 1)
        if len(new_bytes) != expected_len:
            raise ValueError(
                f"Patched name {new_name!r} has different byte length "
                f"({len(new_bytes)}) than original {name_str!r} ({expected_len})"
            )

        buf[string_data_start : string_data_start + len(new_bytes)] = new_bytes
        modified.append(f"{name_str} → {new_name}")

    # ── 3. Patch FName Number fields in the Export Table ────────────
    #
    # UE5 FName = (NameIndex: int32, Number: int32).  When Number > 0
    # the engine displays the name as  BaseName_{Number-1}.
    #
    # Main meshes use this:  base "SK_1055" + Number=1055501
    #   → displayed as "SK_1055_1055500"
    # Lobby meshes use full string "SK_1055_1055500_Lobby" + Number=0.
    #
    # UAssetTool computes PublicExportHash = CityHash64(displayed_name).
    # If Number is wrong the hash won't match what the game expects and
    # the export will be invisible.
    #
    # Each export entry is 96 bytes.  The FName ObjectName sits at
    # offset +0x10 (NameIndex) and +0x14 (Number) within the entry.

    _EXPORT_ENTRY_SIZE = 96
    _FNAME_NAME_IDX_OFF = 0x10
    _FNAME_NUMBER_OFF = 0x14

    try:
        source_num = int(source_skin_id) + 1  # e.g. 1055500 → 1055501
        target_num = int(target_skin_id) + 1  # e.g. 1055001 → 1055002
    except ValueError:
        source_num = target_num = None  # non-numeric IDs → skip

    if source_num is not None and source_num != target_num:
        for i in range(export_count):
            entry_base = export_offset + i * _EXPORT_ENTRY_SIZE
            number_off = entry_base + _FNAME_NUMBER_OFF
            name_idx_off = entry_base + _FNAME_NAME_IDX_OFF

            cur_number = _read_i32(raw, number_off)
            if cur_number != source_num:
                continue

            # Validate: the NameIndex should point to a name starting
            # with "SK_" to avoid false positives.
            name_idx = _read_i32(raw, name_idx_off)
            if 0 <= name_idx < len(entries):
                base_name = entries[name_idx][0]
                if not base_name.startswith("SK_"):
                    continue

            # Replace Number
            struct.pack_into("<i", buf, number_off, target_num)
            display_old = f"{entries[name_idx][0]}_{source_num - 1}"
            display_new = f"{entries[name_idx][0]}_{target_num - 1}"
            modified.append(
                f"Export[{i}] FName Number: {display_old} → {display_new}"
            )

    output_path.write_bytes(bytes(buf))
    return modified
