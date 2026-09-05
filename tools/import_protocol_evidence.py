"""Convert private reverse-engineering evidence into a clean protocol registry.

The generated registry contains functional names and wire schemas only. It does
not copy decompiled code, binaries, game assets, captured player data, or server
responses. Run from the server repository root.
"""

from __future__ import annotations

import json
import re
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
EVIDENCE = ROOT.parent / "analysis"
SOURCE = EVIDENCE / "client-semantics" / "thrift-schema-client-enriched.json"
REQUIREMENTS = EVIDENCE / "full-rpc-audit" / "PROTOCOL_REQUIREMENTS.md"
CALLSITE = EVIDENCE / "full-rpc-audit" / "STATIC_CALLSITE_COVERAGE.md"
OUTPUT = ROOT / "crates" / "protocol" / "schema"
OVERRIDES = OUTPUT / "verified_overrides.json"


def table_calls(markdown: str, heading: str) -> list[str]:
    section = markdown.split(heading, 1)[1].split("## ", 1)[0]
    calls: list[str] = []
    for line in section.splitlines():
        if not line.startswith("|") or "`" not in line:
            continue
        columns = [column.strip() for column in line.strip("|").split("|")]
        if len(columns) < 2 or columns[0] in {"域", "---"}:
            continue
        calls.extend(re.findall(r"`([^`]+)`", columns[1]))
    return sorted(set(calls))


def contained_type(wire_type: str) -> str:
    """Return the generated DTO behind a list/map wire candidate.

    The evidence records collection responses as ``[]Type`` and
    ``map[key]Type``.  Collections are represented by the RPC field itself;
    only the value DTO has a generated struct definition in the type table.
    """

    if wire_type.startswith("[]"):
        return wire_type[2:]
    map_match = re.fullmatch(r"map\[[^]]+](.+)", wire_type)
    if map_match:
        return map_match.group(1)
    return wire_type


def candidate_names(entries: list[dict[str, str]]) -> list[str]:
    return sorted(
        {f"{entry['namespace']}::{contained_type(entry['type'])}" for entry in entries}
    )


def wire_candidates(entries: list[dict[str, str]]) -> list[dict[str, str]]:
    return sorted(
        (
            {"namespace": entry["namespace"], "wire_type": entry["type"]}
            for entry in entries
        ),
        key=lambda entry: (entry["namespace"], entry["wire_type"]),
    )


def main() -> None:
    source = json.loads(SOURCE.read_text(encoding="utf-8"))
    callsite = CALLSITE.read_text(encoding="utf-8")
    requirements = REQUIREMENTS.read_text(encoding="utf-8")

    active = table_calls(callsite, "## 62 个实际发起的 RPC")
    transport = table_calls(callsite, "## 50 个生成传输代码存在、但无玩法调用点的 RPC")
    dto_section = callsite.split("## 15 个 DTO-only", 1)[1].split("## ", 1)[0]
    dto = sorted(set(re.findall(r"`(?:[^/`]+/)?([^`]+)`", dto_section.split("这些契约", 1)[0])))

    classes = {name: "active" for name in active}
    classes.update({name: "transport-only" for name in transport})
    classes.update({name: "dto-only" for name in dto})

    categories: dict[str, str] = {}
    for match in re.finditer(r"### `([^`]+)`(?P<body>.*?)(?=\n### `|\Z)", requirements, re.S):
        path = re.search(r"路径：`?/([^/`]+)/", match.group("body"))
        if path:
            call_name = match.group(1).rsplit("/", 1)[-1]
            categories[call_name] = path.group(1)

    calls = []
    for name, call in source["calls"].items():
        calls.append(
            {
                "name": name,
                "category": categories[name],
                "class": classes[name],
                "request_candidates": candidate_names(call["request_wire_candidates"]),
                "response_candidates": candidate_names(call["wire_candidates"]),
                "request_wires": wire_candidates(call["request_wire_candidates"]),
                "response_wires": wire_candidates(call["wire_candidates"]),
                "has_server_time": bool(call["has_server_time"]),
            }
        )

    types = []
    for qualified_name, fields in source["types"].items():
        namespace, name = qualified_name.split("::", 1)
        types.append({"namespace": namespace, "name": name, "fields": fields})

    assert len(calls) == 127
    assert len(active) == 62
    assert len(transport) == 50
    assert len(dto) == 15
    assert set(classes) == set(source["calls"])
    assert set(categories) == set(source["calls"])

    OUTPUT.mkdir(parents=True, exist_ok=True)
    document = {"schema": 1, "calls": sorted(calls, key=lambda row: row["name"]), "types": types}
    overrides = json.loads(OVERRIDES.read_text(encoding="utf-8"))
    call_index = {call["name"]: call for call in document["calls"]}
    type_index = {
        f"{type_spec['namespace']}::{type_spec['name']}": type_spec
        for type_spec in document["types"]
    }
    for name, replacement in overrides.get("calls", {}).items():
        target = call_index.get(name)
        if target is None:
            raise ValueError(f"verified call override references unknown RPC {name}")
        unknown = set(replacement) - {
            "class",
            "request_candidates",
            "response_candidates",
            "request_wires",
            "response_wires",
            "has_server_time",
        }
        if unknown:
            raise ValueError(f"unsupported override keys for {name}: {sorted(unknown)}")
        target.update(replacement)
    for name, fields in overrides.get("types", {}).items():
        target = type_index.get(name)
        if target is None:
            raise ValueError(f"verified type override references unknown DTO {name}")
        target["fields"] = fields
    (OUTPUT / "protocol.json").write_text(
        json.dumps(document, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )
    generated_classes = {
        class_name: sorted(
            call["name"] for call in document["calls"] if call["class"] == class_name
        )
        for class_name in ("active", "transport-only", "dto-only")
    }
    assert sum(map(len, generated_classes.values())) == 127
    assert len(generated_classes["active"]) == 69
    assert len(generated_classes["transport-only"]) == 45
    assert len(generated_classes["dto-only"]) == 13
    for filename, values in (
        ("active_calls.txt", generated_classes["active"]),
        ("transport_only_calls.txt", generated_classes["transport-only"]),
        ("dto_only_calls.txt", generated_classes["dto-only"]),
    ):
        (OUTPUT / filename).write_text("\n".join(values) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
