#!/usr/bin/env python3
"""Bounded Linux DRM fdinfo memory parsing for native soak evidence."""

from __future__ import annotations

from pathlib import Path
import re
import statistics
from typing import Any, Iterable


DRM_MEMORY_INTERFACE = "linux-drm-fdinfo-v1"
MAX_DRM_PROCESSES = 128
MAX_DRM_FDS_PER_PROCESS = 512
MAX_DRM_FDINFOS = 2_048
MAX_DRM_FDINFO_BYTES = 64 * 1024
MAX_DRM_CLIENTS = 64
MAX_DRM_REGIONS = 32
MAX_DRM_LABEL_CHARACTERS = 64
MAX_DRM_FIELD_BYTES = 1 << 50
MAX_DRM_AGGREGATE_BYTES = 1 << 60
UNAVAILABLE_REASONS = {
    "no-drm-client-fd",
    "no-standardized-memory-fields",
    "missing-drm-client-id",
}

REGION_NAME = re.compile(r"[A-Za-z0-9][A-Za-z0-9_.-]{0,63}")
MEMORY_KEY = re.compile(rf"drm-(total|resident|memory)-({REGION_NAME.pattern})")
PCI_DEVICE = re.compile(r"[0-9A-Fa-f]{4}:[0-9A-Fa-f]{2}:[0-9A-Fa-f]{2}\.[0-7]")


class DrmMemoryError(ValueError):
    """Standardized DRM usage data violates the evidence contract."""


def _bounded_label(value: object, field: str) -> str:
    if (
        not isinstance(value, str)
        or not value
        or len(value) > MAX_DRM_LABEL_CHARACTERS
        or any(not character.isprintable() or character in "\r\n" for character in value)
    ):
        raise DrmMemoryError(f"{field} is invalid")
    return value


def _uint(value: object, field: str, *, maximum: int = (1 << 64) - 1) -> int:
    if (
        isinstance(value, bool)
        or not isinstance(value, int)
        or value < 0
        or value > maximum
    ):
        raise DrmMemoryError(f"{field} is not a bounded unsigned integer")
    return value


def parse_memory_value(value: str) -> int:
    fields = value.split()
    if len(fields) not in {1, 2} or not fields[0].isascii() or not fields[0].isdigit():
        raise DrmMemoryError(f"invalid DRM memory value {value!r}")
    multiplier = {None: 1, "KiB": 1024, "MiB": 1024 * 1024}.get(
        fields[1] if len(fields) == 2 else None
    )
    if multiplier is None:
        raise DrmMemoryError(f"invalid DRM memory unit in {value!r}")
    amount = int(fields[0])
    if amount > MAX_DRM_FIELD_BYTES // multiplier:
        raise DrmMemoryError("DRM memory value exceeds its byte ceiling")
    return amount * multiplier


def parse_drm_fdinfo(text: str) -> dict[str, Any] | None:
    values: dict[str, str] = {}
    for raw_line in text.splitlines():
        key, separator, raw_value = raw_line.partition(":")
        if not key.startswith("drm-"):
            continue
        if not separator:
            raise DrmMemoryError(f"malformed standardized DRM field {raw_line!r}")
        if key in values:
            raise DrmMemoryError(f"duplicate standardized DRM field {key!r}")
        values[key] = raw_value.strip()

    if not values:
        return None
    driver = values.get("drm-driver")
    if driver is None:
        raise DrmMemoryError("DRM fdinfo omits drm-driver")
    driver = _bounded_label(driver, "drm-driver")

    device = values.get("drm-pdev")
    if device is not None and PCI_DEVICE.fullmatch(device) is None:
        raise DrmMemoryError("drm-pdev is invalid")
    raw_client_id = values.get("drm-client-id")
    client_id = None
    if raw_client_id is not None:
        if not raw_client_id.isascii() or not raw_client_id.isdigit():
            raise DrmMemoryError("drm-client-id is invalid")
        client_id = _uint(int(raw_client_id), "drm-client-id")

    totals: dict[str, int] = {}
    residents: dict[str, int] = {}
    resident_aliases: dict[str, int] = {}
    for key, raw_value in values.items():
        # drm-total-cycles-<engine> is an utilization counter, not the
        # similarly-prefixed drm-total-<memory-region> field.
        if key.startswith("drm-total-cycles-"):
            continue
        match = MEMORY_KEY.fullmatch(key)
        if match is None:
            if key.startswith(("drm-total-", "drm-resident-", "drm-memory-")):
                raise DrmMemoryError(f"invalid standardized DRM memory key {key!r}")
            continue
        category, region = match.groups()
        parsed = parse_memory_value(raw_value)
        destination = {
            "total": totals,
            "resident": residents,
            "memory": resident_aliases,
        }[category]
        destination[region] = parsed
    return {
        "driver": driver,
        "device": device,
        "client_id": client_id,
        "totals": totals,
        "residents": residents,
        "resident_aliases": resident_aliases,
    }


def _unavailable(reason: str) -> dict[str, Any]:
    if reason not in UNAVAILABLE_REASONS:
        raise DrmMemoryError(f"unknown unavailable reason {reason!r}")
    return {
        "status": "unavailable",
        "reason": reason,
        "clients": [],
        "client_count": 0,
        "total_bytes": None,
        "resident_bytes": None,
    }


def _merge_max(destination: dict[str, int], source: dict[str, int]) -> None:
    for key, value in source.items():
        destination[key] = max(destination.get(key, 0), value)


def _sum_known(values: Iterable[int | None]) -> int | None:
    present = [value for value in values if value is not None]
    if not present:
        return None
    total = sum(present)
    if total > MAX_DRM_AGGREGATE_BYTES:
        raise DrmMemoryError("aggregate DRM memory exceeds its byte ceiling")
    return total


def aggregate_drm_fdinfos(texts: Iterable[str]) -> dict[str, Any]:
    saw_drm = False
    saw_memory = False
    missing_identity = False
    merged: dict[tuple[str, int], dict[str, Any]] = {}
    for text in texts:
        parsed = parse_drm_fdinfo(text)
        if parsed is None:
            continue
        saw_drm = True
        has_memory = any(
            parsed[field] for field in ("totals", "residents", "resident_aliases")
        )
        if not has_memory:
            continue
        saw_memory = True
        if parsed["client_id"] is None:
            missing_identity = True
            continue
        identity = (parsed["device"] or "<global>", parsed["client_id"])
        if parsed["device"] is None:
            if any(
                known_client_id == parsed["client_id"] and known_device != "<global>"
                for known_device, known_client_id in merged
            ):
                raise DrmMemoryError(
                    "DRM client identity mixes global and device-local scope"
                )
        elif ("<global>", parsed["client_id"]) in merged:
            raise DrmMemoryError(
                "DRM client identity mixes global and device-local scope"
            )
        current = merged.get(identity)
        if current is None:
            if len(merged) >= MAX_DRM_CLIENTS:
                raise DrmMemoryError("DRM client count exceeds its ceiling")
            current = {
                "driver": parsed["driver"],
                "device": parsed["device"],
                "client_id": parsed["client_id"],
                "totals": {},
                "residents": {},
                "resident_aliases": {},
            }
            merged[identity] = current
        elif current["driver"] != parsed["driver"]:
            raise DrmMemoryError("duplicate DRM client identity changes driver")
        _merge_max(current["totals"], parsed["totals"])
        _merge_max(current["residents"], parsed["residents"])
        _merge_max(current["resident_aliases"], parsed["resident_aliases"])

    if missing_identity:
        return _unavailable("missing-drm-client-id")
    if not saw_memory:
        return _unavailable(
            "no-standardized-memory-fields" if saw_drm else "no-drm-client-fd"
        )

    clients = []
    observed_regions: set[str] = set()
    for identity in sorted(merged):
        source = merged[identity]
        regions = sorted(
            set(source["totals"])
            | set(source["residents"])
            | set(source["resident_aliases"])
        )
        observed_regions.update(regions)
        if len(observed_regions) > MAX_DRM_REGIONS:
            raise DrmMemoryError("DRM region count exceeds its ceiling")
        rendered_regions = {}
        for region in regions:
            rendered_regions[region] = {
                "total_bytes": source["totals"].get(region),
                "resident_bytes": source["residents"].get(
                    region, source["resident_aliases"].get(region)
                ),
            }
        clients.append(
            {
                "driver": source["driver"],
                "device": source["device"],
                "client_id": source["client_id"],
                "regions": rendered_regions,
            }
        )

    total_bytes = _sum_known(
        region["total_bytes"]
        for client in clients
        for region in client["regions"].values()
    )
    resident_bytes = _sum_known(
        region["resident_bytes"]
        for client in clients
        for region in client["regions"].values()
    )
    result = {
        "status": "measured",
        "reason": None,
        "clients": clients,
        "client_count": len(clients),
        "total_bytes": total_bytes,
        "resident_bytes": resident_bytes,
    }
    validate_drm_memory_snapshot(result)
    return result


def read_process_drm_memory(proc_root: Path, pids: Iterable[int]) -> dict[str, Any]:
    unique_pids = sorted(set(pids))
    if any(
        isinstance(pid, bool) or not isinstance(pid, int) or pid <= 0
        for pid in unique_pids
    ):
        raise DrmMemoryError("DRM process list contains an invalid PID")
    if len(unique_pids) > MAX_DRM_PROCESSES:
        raise DrmMemoryError("DRM process count exceeds its ceiling")
    texts: list[str] = []
    for pid in unique_pids:
        directory = proc_root / str(pid) / "fdinfo"
        try:
            entries = sorted(directory.iterdir(), key=lambda path: int(path.name))
        except FileNotFoundError:
            continue
        if len(entries) > MAX_DRM_FDS_PER_PROCESS:
            raise DrmMemoryError("DRM fdinfo count exceeds its per-process ceiling")
        for entry in entries:
            if len(texts) >= MAX_DRM_FDINFOS:
                raise DrmMemoryError("DRM fdinfo total count exceeds its ceiling")
            try:
                with entry.open("rb") as source:
                    data = source.read(MAX_DRM_FDINFO_BYTES + 1)
            except FileNotFoundError:
                continue
            if len(data) > MAX_DRM_FDINFO_BYTES:
                raise DrmMemoryError("DRM fdinfo exceeds its byte ceiling")
            try:
                texts.append(data.decode("ascii"))
            except UnicodeDecodeError as error:
                raise DrmMemoryError("DRM fdinfo is not ASCII") from error
    return aggregate_drm_fdinfos(texts)


def validate_drm_memory_snapshot(value: object) -> dict[str, Any]:
    if not isinstance(value, dict) or set(value) != {
        "status",
        "reason",
        "clients",
        "client_count",
        "total_bytes",
        "resident_bytes",
    }:
        raise DrmMemoryError("DRM memory snapshot has an invalid shape")
    if value["status"] == "unavailable":
        if (
            value["reason"] not in UNAVAILABLE_REASONS
            or value["clients"] != []
            or value["client_count"] != 0
            or value["total_bytes"] is not None
            or value["resident_bytes"] is not None
        ):
            raise DrmMemoryError("unavailable DRM memory snapshot is inconsistent")
        return value
    if value["status"] != "measured" or value["reason"] is not None:
        raise DrmMemoryError("DRM memory disposition is invalid")
    clients = value["clients"]
    if not isinstance(clients, list) or not clients or len(clients) > MAX_DRM_CLIENTS:
        raise DrmMemoryError("measured DRM clients are invalid")
    client_count = _uint(
        value["client_count"], "DRM client count", maximum=MAX_DRM_CLIENTS
    )
    if client_count != len(clients):
        raise DrmMemoryError("DRM client count does not match the client array")

    aggregate_total = value["total_bytes"]
    aggregate_resident = value["resident_bytes"]
    if aggregate_total is not None:
        aggregate_total = _uint(
            aggregate_total,
            "aggregate DRM total bytes",
            maximum=MAX_DRM_AGGREGATE_BYTES,
        )
    if aggregate_resident is not None:
        aggregate_resident = _uint(
            aggregate_resident,
            "aggregate DRM resident bytes",
            maximum=MAX_DRM_AGGREGATE_BYTES,
        )

    identities = []
    totals: list[int | None] = []
    residents: list[int | None] = []
    all_regions: set[str] = set()
    for client in clients:
        if not isinstance(client, dict) or set(client) != {
            "driver",
            "device",
            "client_id",
            "regions",
        }:
            raise DrmMemoryError("DRM client has an invalid shape")
        _bounded_label(client["driver"], "DRM driver")
        device = client["device"]
        if device is not None and (
            not isinstance(device, str) or PCI_DEVICE.fullmatch(device) is None
        ):
            raise DrmMemoryError("DRM client device is invalid")
        client_id = _uint(client["client_id"], "DRM client id")
        identity = (device or "<global>", client_id)
        identities.append(identity)
        regions = client["regions"]
        if (
            not isinstance(regions, dict)
            or not regions
            or list(regions) != sorted(regions)
        ):
            raise DrmMemoryError("DRM client regions are invalid")
        for region, measurements in regions.items():
            if not isinstance(region, str) or REGION_NAME.fullmatch(region) is None:
                raise DrmMemoryError("DRM region is invalid")
            all_regions.add(region)
            if len(all_regions) > MAX_DRM_REGIONS:
                raise DrmMemoryError("DRM region count exceeds its ceiling")
            if not isinstance(measurements, dict) or set(measurements) != {
                "total_bytes",
                "resident_bytes",
            }:
                raise DrmMemoryError("DRM region measurements have an invalid shape")
            total = measurements["total_bytes"]
            resident = measurements["resident_bytes"]
            if total is not None:
                total = _uint(total, "DRM total bytes", maximum=MAX_DRM_FIELD_BYTES)
            if resident is not None:
                resident = _uint(
                    resident, "DRM resident bytes", maximum=MAX_DRM_FIELD_BYTES
                )
            if total is None and resident is None:
                raise DrmMemoryError("DRM region has no memory measurement")
            totals.append(total)
            residents.append(resident)
    if identities != sorted(set(identities)):
        raise DrmMemoryError("DRM client identities are not unique and sorted")
    expected_total = _sum_known(totals)
    expected_resident = _sum_known(residents)
    if aggregate_total != expected_total:
        raise DrmMemoryError("aggregate DRM total bytes do not match clients")
    if aggregate_resident != expected_resident:
        raise DrmMemoryError("aggregate DRM resident bytes do not match clients")
    if expected_total is None and expected_resident is None:
        raise DrmMemoryError("measured DRM snapshot contains no memory")
    return value


def _edge_growth(values: list[int]) -> float:
    width = max(1, len(values) // 4)
    return statistics.median(values[-width:]) - statistics.median(values[:width])


def summarize_drm_memory(samples: list[dict[str, Any]]) -> dict[str, Any]:
    snapshots = [
        validate_drm_memory_snapshot(sample.get("gpu_memory")) for sample in samples
    ]
    measured = [snapshot for snapshot in snapshots if snapshot["status"] == "measured"]
    total_values = [
        snapshot["total_bytes"]
        for snapshot in measured
        if snapshot["total_bytes"] is not None
    ]
    resident_values = [
        snapshot["resident_bytes"]
        for snapshot in measured
        if snapshot["resident_bytes"] is not None
    ]
    reasons = {
        reason: sum(snapshot["reason"] == reason for snapshot in snapshots)
        for reason in sorted(UNAVAILABLE_REASONS)
        if any(snapshot["reason"] == reason for snapshot in snapshots)
    }
    return {
        "measured_samples": len(measured),
        "unavailable_samples": len(samples) - len(measured),
        "unavailable_reasons": reasons,
        "total_samples": len(total_values),
        "resident_samples": len(resident_values),
        "peak_total_bytes": max(total_values) if total_values else None,
        "peak_resident_bytes": max(resident_values) if resident_values else None,
        "total_growth_bytes": (
            _edge_growth(total_values)
            if len(total_values) == len(samples) and samples
            else None
        ),
        "resident_growth_bytes": (
            _edge_growth(resident_values)
            if len(resident_values) == len(samples) and samples
            else None
        ),
    }


def evaluate_drm_memory(
    samples: list[dict[str, Any]],
    *,
    required: bool,
    max_resident_bytes: int,
    max_resident_growth_bytes: int,
) -> list[dict[str, Any]]:
    if type(required) is not bool:
        raise DrmMemoryError("required DRM memory coverage flag is not a boolean")
    max_resident_bytes = _uint(
        max_resident_bytes,
        "maximum DRM resident bytes",
        maximum=MAX_DRM_AGGREGATE_BYTES,
    )
    max_resident_growth_bytes = _uint(
        max_resident_growth_bytes,
        "maximum DRM resident growth bytes",
        maximum=MAX_DRM_AGGREGATE_BYTES,
    )
    summary = summarize_drm_memory(samples)
    count = len(samples)
    measured = summary["measured_samples"]
    total_samples = summary["total_samples"]
    resident_samples = summary["resident_samples"]
    peak = summary["peak_resident_bytes"]
    growth = summary["resident_growth_bytes"]
    return [
        {
            "name": "DRM memory disposition consistency",
            "actual": measured,
            "allowed": [0, count],
            "passed": measured in {0, count},
        },
        {
            "name": "required DRM memory sample coverage",
            "actual": measured,
            "min": count if required else 0,
            "passed": not required or (count > 0 and measured == count),
        },
        {
            "name": "DRM total-memory field consistency",
            "actual": total_samples,
            "allowed": [0, count],
            "passed": total_samples in {0, count},
        },
        {
            "name": "DRM resident-memory field consistency",
            "actual": resident_samples,
            "allowed": [0, count],
            "passed": resident_samples in {0, count},
        },
        {
            "name": "maximum process-attributed DRM resident memory",
            "actual": peak,
            "max": max_resident_bytes,
            "passed": peak is None or peak <= max_resident_bytes,
        },
        {
            "name": "first-to-last DRM resident memory growth",
            "actual": growth,
            "max": max_resident_growth_bytes,
            "passed": growth is None or growth <= max_resident_growth_bytes,
        },
    ]
