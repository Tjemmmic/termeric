from __future__ import annotations

import copy
from pathlib import Path
from tempfile import TemporaryDirectory
import unittest
from unittest import mock

from scripts.drm_memory import (
    MAX_DRM_FIELD_BYTES,
    DrmMemoryError,
    aggregate_drm_fdinfos,
    evaluate_drm_memory,
    parse_drm_fdinfo,
    parse_memory_value,
    read_process_drm_memory,
    summarize_drm_memory,
    validate_drm_memory_snapshot,
)


def fdinfo(
    client_id: int | None,
    *memory: str,
    driver: str = "amdgpu",
    device: str | None = "0000:03:00.0",
) -> str:
    lines = ["pos:\t0", f"drm-driver:\t{driver}"]
    if device is not None:
        lines.append(f"drm-pdev:\t{device}")
    if client_id is not None:
        lines.append(f"drm-client-id:\t{client_id}")
    lines.extend(memory)
    return "\n".join(lines) + "\n"


def measured_sample(index: int, resident_mib: int, total_mib: int) -> dict:
    return {
        "elapsed_seconds": float(index),
        "gpu_memory": aggregate_drm_fdinfos(
            [
                fdinfo(
                    7,
                    f"drm-total-vram: {total_mib} MiB",
                    f"drm-resident-vram: {resident_mib} MiB",
                )
            ]
        ),
    }


class DrmMemoryTests(unittest.TestCase):
    def test_standard_units_are_exact_and_other_units_fail_closed(self) -> None:
        self.assertEqual(parse_memory_value("7"), 7)
        self.assertEqual(parse_memory_value("7 KiB"), 7 * 1024)
        self.assertEqual(parse_memory_value("7 MiB"), 7 * 1024 * 1024)
        for value in ["-1", "1.5 MiB", "1 KB", "1 GiB", "1 MiB trailing", ""]:
            with self.subTest(value=value), self.assertRaises(DrmMemoryError):
                parse_memory_value(value)
        with self.assertRaises(DrmMemoryError):
            parse_memory_value(f"{MAX_DRM_FIELD_BYTES + 1}")

    def test_parser_rejects_malformed_standardized_fields(self) -> None:
        invalid = [
            "drm-driver amdgpu\n",
            "drm-driver: amdgpu\ndrm-driver: i915\n",
            "drm-client-id: 7\ndrm-total-vram: 1 MiB\n",
            "drm-driver: amdgpu\ndrm-client-id: -1\n",
            "drm-driver: amdgpu\ndrm-pdev: card0\n",
            "drm-driver: amdgpu\ndrm-client-id: 7\ndrm-total-: 1 MiB\n",
        ]
        for value in invalid:
            with self.subTest(value=value), self.assertRaises(DrmMemoryError):
                parse_drm_fdinfo(value)
        self.assertIsNone(parse_drm_fdinfo("pos: 0\nflags: 0100002\n"))

    def test_duplicate_clients_and_deprecated_alias_do_not_inflate_totals(self) -> None:
        first = fdinfo(
            7,
            "drm-total-vram: 4 MiB",
            "drm-resident-vram: 3 MiB",
            "drm-memory-vram: 9 MiB",
            "drm-total-memory: 512 KiB",
        )
        duplicate = fdinfo(
            7,
            "drm-total-vram: 5 MiB",
            "drm-resident-vram: 2 MiB",
            "drm-memory-memory: 256 KiB",
        )
        second = fdinfo(
            8,
            "drm-total-vram: 2 MiB",
            "drm-memory-vram: 1 MiB",
        )
        snapshot = aggregate_drm_fdinfos([first, duplicate, first, second])
        self.assertEqual(snapshot["client_count"], 2)
        self.assertEqual(snapshot["total_bytes"], int(7.5 * 1024 * 1024))
        self.assertEqual(snapshot["resident_bytes"], int(4.25 * 1024 * 1024))
        first_regions = snapshot["clients"][0]["regions"]
        self.assertEqual(first_regions["vram"]["resident_bytes"], 3 * 1024 * 1024)
        validate_drm_memory_snapshot(snapshot)

    def test_total_cycle_counters_are_not_misclassified_as_memory(self) -> None:
        snapshot = aggregate_drm_fdinfos(
            [
                fdinfo(
                    7,
                    "drm-total-cycles-render: 987654321",
                    "drm-cycles-render: 123456789",
                )
            ]
        )
        self.assertEqual(snapshot["status"], "unavailable")
        self.assertEqual(snapshot["reason"], "no-standardized-memory-fields")

    def test_unavailable_dispositions_are_explicit(self) -> None:
        self.assertEqual(
            aggregate_drm_fdinfos([])["reason"], "no-drm-client-fd"
        )
        self.assertEqual(
            aggregate_drm_fdinfos([fdinfo(7)])["reason"],
            "no-standardized-memory-fields",
        )
        self.assertEqual(
            aggregate_drm_fdinfos(
                [fdinfo(None, "drm-resident-vram: 1 MiB")]
            )["reason"],
            "missing-drm-client-id",
        )

    def test_global_and_device_local_client_identity_cannot_be_mixed(self) -> None:
        global_client = fdinfo(
            7, "drm-resident-memory: 1 MiB", device=None
        )
        device_client = fdinfo(7, "drm-resident-vram: 1 MiB")
        for values in ([global_client, device_client], [device_client, global_client]):
            with self.subTest(values=values), self.assertRaisesRegex(
                DrmMemoryError, "mixes global and device-local"
            ):
                aggregate_drm_fdinfos(values)

    def test_filesystem_reader_is_process_bounded_and_skips_exited_fds(self) -> None:
        with TemporaryDirectory() as directory:
            root = Path(directory)
            for pid in [20, 21]:
                (root / str(pid) / "fdinfo").mkdir(parents=True)
            (root / "20/fdinfo/3").write_text(
                fdinfo(7, "drm-resident-vram: 3 MiB"), encoding="ascii"
            )
            (root / "20/fdinfo/4").write_text("pos: 0\n", encoding="ascii")
            (root / "21/fdinfo/5").write_text(
                fdinfo(7, "drm-resident-vram: 3 MiB"), encoding="ascii"
            )
            snapshot = read_process_drm_memory(root, [21, 20, 21, 22])
            self.assertEqual(snapshot["client_count"], 1)
            self.assertEqual(snapshot["resident_bytes"], 3 * 1024 * 1024)

            with mock.patch("scripts.drm_memory.MAX_DRM_FDS_PER_PROCESS", 1):
                with self.assertRaisesRegex(DrmMemoryError, "per-process ceiling"):
                    read_process_drm_memory(root, [20])

    def test_snapshot_validation_reconstructs_aggregates_and_identity(self) -> None:
        snapshot = aggregate_drm_fdinfos(
            [fdinfo(7, "drm-total-vram: 4 MiB", "drm-resident-vram: 3 MiB")]
        )
        for mutation in [
            lambda value: value.update({"resident_bytes": 1}),
            lambda value: value.update({"client_count": 2}),
            lambda value: value.update({"client_count": True}),
            lambda value: value.update({"total_bytes": True}),
            lambda value: value["clients"][0]["regions"].update(
                {"bad region": {"total_bytes": 1, "resident_bytes": 1}}
            ),
        ]:
            changed = copy.deepcopy(snapshot)
            mutation(changed)
            with self.assertRaises(DrmMemoryError):
                validate_drm_memory_snapshot(changed)

    def test_summary_and_gates_detect_coverage_peak_and_growth_independently(self) -> None:
        samples = [measured_sample(i, 100 + i * 10, 200 + i * 10) for i in range(8)]
        summary = summarize_drm_memory(samples)
        self.assertEqual(summary["measured_samples"], 8)
        self.assertEqual(summary["peak_resident_bytes"], 170 * 1024 * 1024)
        self.assertEqual(summary["resident_growth_bytes"], 60 * 1024 * 1024)
        checks = {
            check["name"]: check
            for check in evaluate_drm_memory(
                samples,
                required=True,
                max_resident_bytes=160 * 1024 * 1024,
                max_resident_growth_bytes=50 * 1024 * 1024,
            )
        }
        self.assertFalse(
            checks["maximum process-attributed DRM resident memory"]["passed"]
        )
        self.assertFalse(
            checks["first-to-last DRM resident memory growth"]["passed"]
        )
        self.assertTrue(checks["required DRM memory sample coverage"]["passed"])

        samples[3]["gpu_memory"] = aggregate_drm_fdinfos([])
        checks = {
            check["name"]: check
            for check in evaluate_drm_memory(
                samples,
                required=False,
                max_resident_bytes=1024 * 1024 * 1024,
                max_resident_growth_bytes=256 * 1024 * 1024,
            )
        }
        self.assertFalse(checks["DRM memory disposition consistency"]["passed"])
        self.assertFalse(checks["DRM resident-memory field consistency"]["passed"])


if __name__ == "__main__":
    unittest.main()
