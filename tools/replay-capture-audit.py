#!/usr/bin/env python3
"""Replay private raw Thrift captures against a local GGFM server.

The capture archive stays outside the repository.  This tool rewrites only the
request identity to the fresh audit slot, then compares result codes and wire
trees while deliberately ignoring player-specific scalar values and list
lengths.
"""

from __future__ import annotations

import argparse
import base64
import bz2
import json
import re
import struct
import urllib.parse
import urllib.request
import zipfile
from pathlib import Path


def read_value(data: bytes, offset: int, kind: int):
    if kind == 2:
        return bool(data[offset]), offset + 1
    if kind == 3:
        return struct.unpack_from(">b", data, offset)[0], offset + 1
    if kind == 4:
        return struct.unpack_from(">d", data, offset)[0], offset + 8
    if kind == 6:
        return struct.unpack_from(">h", data, offset)[0], offset + 2
    if kind == 8:
        return struct.unpack_from(">i", data, offset)[0], offset + 4
    if kind == 10:
        return struct.unpack_from(">q", data, offset)[0], offset + 8
    if kind == 11:
        size = struct.unpack_from(">i", data, offset)[0]
        offset += 4
        return data[offset : offset + size], offset + size
    if kind == 12:
        return read_struct(data, offset)
    if kind in (14, 15):
        item_kind = data[offset]
        size = struct.unpack_from(">i", data, offset + 1)[0]
        offset += 5
        values = []
        for _ in range(size):
            value, offset = read_value(data, offset, item_kind)
            values.append(value)
        return (item_kind, values), offset
    if kind == 13:
        key_kind, value_kind = data[offset], data[offset + 1]
        size = struct.unpack_from(">i", data, offset + 2)[0]
        offset += 6
        values = []
        for _ in range(size):
            key, offset = read_value(data, offset, key_kind)
            value, offset = read_value(data, offset, value_kind)
            values.append((key, value))
        return (key_kind, value_kind, values), offset
    raise ValueError(f"unsupported Thrift type {kind} at {offset}")


def read_struct(data: bytes, offset: int = 0):
    fields = []
    while True:
        kind = data[offset]
        offset += 1
        if kind == 0:
            return fields, offset
        field_id = struct.unpack_from(">h", data, offset)[0]
        value, offset = read_value(data, offset + 2, kind)
        fields.append([field_id, kind, value])


def write_value(kind: int, value) -> bytes:
    if kind == 2:
        return bytes([int(value)])
    if kind == 3:
        return struct.pack(">b", value)
    if kind == 4:
        return struct.pack(">d", value)
    if kind == 6:
        return struct.pack(">h", value)
    if kind == 8:
        return struct.pack(">i", value)
    if kind == 10:
        return struct.pack(">q", value)
    if kind == 11:
        raw = value if isinstance(value, bytes) else value.encode("utf-8")
        return struct.pack(">i", len(raw)) + raw
    if kind == 12:
        return write_struct(value)
    if kind in (14, 15):
        item_kind, values = value
        return bytes([item_kind]) + struct.pack(">i", len(values)) + b"".join(
            write_value(item_kind, item) for item in values
        )
    if kind == 13:
        key_kind, value_kind, entries = value
        return (
            bytes([key_kind, value_kind])
            + struct.pack(">i", len(entries))
            + b"".join(
                write_value(key_kind, key) + write_value(value_kind, item)
                for key, item in entries
            )
        )
    raise ValueError(f"unsupported Thrift type {kind}")


def write_struct(fields) -> bytes:
    return b"".join(
        bytes([kind]) + struct.pack(">h", field_id) + write_value(kind, value)
        for field_id, kind, value in fields
    ) + b"\0"


def field(fields, field_id: int):
    return next((entry for entry in fields if entry[0] == field_id), None)


def scalar_text(entry) -> str:
    value = entry[2]
    return value.decode("utf-8", errors="replace") if isinstance(value, bytes) else str(value)


def wire_shape(kind: int, value):
    if kind == 12:
        return tuple((field_id, nested_kind, wire_shape(nested_kind, nested)) for field_id, nested_kind, nested in value)
    if kind in (14, 15):
        element_kind, values = value
        samples = {wire_shape(element_kind, item) for item in values}
        return (element_kind, tuple(sorted(samples, key=repr)))
    if kind == 13:
        key_kind, value_kind, entries = value
        key_shapes = {wire_shape(key_kind, key) for key, _ in entries}
        value_shapes = {wire_shape(value_kind, item) for _, item in entries}
        return (
            key_kind,
            value_kind,
            tuple(sorted(key_shapes, key=repr)),
            tuple(sorted(value_shapes, key=repr)),
        )
    return None


def response_summary(raw: bytes):
    fields, end = read_struct(raw)
    if end != len(raw):
        raise ValueError(f"response has {len(raw) - end} trailing bytes")
    result = field(fields, 1)
    code = None
    if result and result[1] == 12:
        result_code = field(result[2], 1)
        code = result_code[2] if result_code else None
    data = field(fields, 5) or field(fields, 4)
    return code, None if data is None else (data[1], wire_shape(data[1], data[2])), data


def shape_differences(expected, actual, path: str = "Data", limit: int = 8):
    output = []

    def visit(left_kind, left, right_kind, right, current):
        if len(output) >= limit:
            return
        if left_kind != right_kind:
            output.append(f"{current}: type {left_kind} != {right_kind}")
            return
        if left_kind == 12:
            left_fields = {row[0]: row for row in left}
            right_fields = {row[0]: row for row in right}
            if left_fields.keys() != right_fields.keys():
                output.append(
                    f"{current}: fields {sorted(left_fields)} != {sorted(right_fields)}"
                )
            for field_id in left_fields.keys() & right_fields.keys():
                a, b = left_fields[field_id], right_fields[field_id]
                visit(a[1], a[2], b[1], b[2], f"{current}.{field_id}")
        elif left_kind in (14, 15):
            left_element, left_values = left
            right_element, right_values = right
            if left_element != right_element:
                output.append(f"{current}: element type {left_element} != {right_element}")
            # Empty versus populated collections are player-state differences,
            # not an incompatible wire shape. Compare samples only when both
            # captures provide one.
            if left_values and right_values:
                visit(
                    left_element,
                    left_values[0],
                    right_element,
                    right_values[0],
                    f"{current}[]",
                )
        elif left_kind == 13:
            left_key, left_value, left_entries = left
            right_key, right_value, right_entries = right
            if (left_key, left_value) != (right_key, right_value):
                output.append(
                    f"{current}: map types {(left_key, left_value)} != {(right_key, right_value)}"
                )
            if left_entries and right_entries:
                visit(
                    left_key,
                    left_entries[0][0],
                    right_key,
                    right_entries[0][0],
                    f"{current}[key]",
                )
                visit(
                    left_value,
                    left_entries[0][1],
                    right_value,
                    right_entries[0][1],
                    f"{current}[value]",
                )

    if expected is None or actual is None:
        if expected is not actual:
            output.append(f"{path}: presence mismatch")
    else:
        visit(expected[1], expected[2], actual[1], actual[2], path)
    return output


def encode_transport(raw: bytes) -> str:
    return base64.b64encode(bz2.compress(raw)).decode("ascii")


def decode_transport(encoded: bytes) -> bytes:
    return bz2.decompress(base64.b64decode(encoded.strip().replace(b" ", b"+")))


def post(endpoint: str, category: str, route_call: str, locale: str, raw: bytes, capability: str, sequence: int) -> bytes:
    form_call = "Request" if route_call == "Request" else route_call
    body = urllib.parse.urlencode(
        {"call": form_call, "tapsonic_data": encode_transport(raw)}
    ).encode("ascii")
    request = urllib.request.Request(
        f"{endpoint.rstrip('/')}/{category}/{route_call}/{locale}/",
        data=body,
        headers={
            "Content-Type": "application/x-www-form-urlencoded",
            "X-GGFM-Session": capability,
            "X-GGFM-Request-Seq": str(sequence),
            "X-GGFM-Device-Time": "1788360100",
            "X-GGFM-UTC-Offset": "0",
        },
    )
    with urllib.request.urlopen(request, timeout=30) as response:
        return decode_transport(response.read())


def schema_indexes(path: Path):
    document = json.loads(path.read_text(encoding="utf-8"))
    calls = {call["name"]: call for call in document["calls"]}
    types = {(row["namespace"], row["name"]): row for row in document["types"]}
    return calls, types


def rewrite_identity(raw: bytes, call: str, calls, types, usn: int, user_id: str) -> bytes:
    outer, end = read_struct(raw)
    if end != len(raw):
        raise ValueError("request has trailing bytes")
    args = field(outer, 2)
    if not args or args[1] != 12:
        return raw
    request_wire = calls[call]["request_wires"][0]
    type_spec = types[(request_wire["namespace"], request_wire["wire_type"])]
    names = {row["id"]: row["name"] for row in type_spec["fields"]}
    for entry in args[2]:
        if names.get(entry[0]) == "U_seq" and entry[1] in (6, 8, 10):
            entry[2] = usn
        elif names.get(entry[0]) == "U_id" and entry[1] == 11:
            entry[2] = user_id.encode("utf-8")
    return write_struct(outer)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("capture_zip", type=Path)
    parser.add_argument("endpoint")
    parser.add_argument("capability")
    parser.add_argument(
        "--schema",
        type=Path,
        default=Path(__file__).parents[1] / "crates/protocol/schema/protocol.json",
    )
    args = parser.parse_args()
    calls, types = schema_indexes(args.schema)
    name_pattern = re.compile(r"([^_/]+)_([^_/]+)_([^_/]+)_(\d+)_request\.bin$")

    with zipfile.ZipFile(args.capture_zip) as archive:
        entries = []
        for name in archive.namelist():
            match = name_pattern.search(name)
            if match and "/real_traffic/" in name:
                entries.append((name, *match.groups()))

        login = next(
            row for row in entries if row[2] == "userLogin" and field(read_struct(archive.read(row[0]))[0], 2)[2][0][2] == 0
        )
        login_raw = post(args.endpoint, login[1], login[2], login[3], archive.read(login[0]), args.capability, 1)
        login_fields, _ = read_struct(login_raw)
        login_data = field(login_fields, 5)[2]
        user = field(login_data, 1)[2]
        usn = int(field(user, 1)[2])
        user_id = scalar_text(field(user, 2))

        mismatches = []
        successful = 0
        for sequence, (name, category, route_call, locale, index) in enumerate(entries, 2):
            wire_call = "init" if route_call == "Request" else route_call
            if wire_call == "userDel":
                continue
            request_raw = rewrite_identity(archive.read(name), wire_call, calls, types, usn, user_id)
            actual_raw = post(
                args.endpoint,
                category,
                route_call,
                locale,
                request_raw,
                args.capability,
                sequence,
            )
            response_name = name.replace("_request.bin", "_response.bin")
            expected_raw = archive.read(response_name)
            expected_code, expected_shape, expected_data = response_summary(expected_raw)
            actual_code, actual_shape, actual_data = response_summary(actual_raw)
            label = f"{category}/{wire_call}#{index}"
            if actual_code == 0:
                successful += 1
            shape_diff = shape_differences(expected_data, actual_data)
            if actual_code != expected_code or shape_diff:
                mismatches.append(
                    {
                        "rpc": label,
                        "captured_code": expected_code,
                        "actual_code": actual_code,
                        "shape_differences": shape_diff,
                    }
                )

    print(json.dumps({"requests": len(entries) - 1, "successful": successful, "mismatches": mismatches}, indent=2))
    return 1 if mismatches else 0


if __name__ == "__main__":
    raise SystemExit(main())
