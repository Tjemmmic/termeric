import datetime
import gzip
import hashlib
from pathlib import Path
import tempfile
import unittest
from unittest import mock

from scripts import build_apt_repository
from scripts.build_apt_repository import (
    Package,
    RepositoryError,
    build_repository,
    control_field,
    packages_index,
    release_text,
    require_one_release,
    secret_key_fingerprint,
    sources_text,
)


DATE = datetime.datetime(2026, 11, 22, 23, 9, 5, tzinfo=datetime.timezone.utc)
CONTROL = "Package: termeric\nVersion: {version}\nArchitecture: {architecture}\nDescription: A terminal\n Themed around roots."
KEY = "-----BEGIN PGP PUBLIC KEY BLOCK-----\n\nmDMEabc\n=abcd\n-----END PGP PUBLIC KEY BLOCK-----\n"


def package(directory: Path, architecture: str = "amd64", version: str = "1.2.0-1") -> Package:
    path = directory / f"termeric_{version}_{architecture}.deb"
    path.write_bytes(f"deb for {architecture}".encode())
    return Package(path, version, architecture, CONTROL.format(version=version, architecture=architecture))


class AptRepositoryTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.directory = Path(temporary.name)

    def test_index_adds_the_download_to_the_control_fields(self):
        amd64 = package(self.directory)
        digest = hashlib.sha256(b"deb for amd64").hexdigest()
        self.assertEqual(
            packages_index(amd64),
            f"{amd64.control}\n"
            "Filename: pool/main/t/termeric/termeric_1.2.0-1_amd64.deb\n"
            f"Size: 13\nSHA256: {digest}\n",
        )

    def test_control_field_ignores_continuation_lines(self):
        control = "Description: x\n Version: 9\nVersion: 1.2.0-1\n"
        self.assertEqual(control_field(control, "Version"), "1.2.0-1")
        with self.assertRaisesRegex(RepositoryError, "no Architecture field"):
            control_field(control, "Architecture")

    def test_release_lists_each_index_with_its_hash_and_size(self):
        release = release_text(["arm64", "amd64"], {"main/binary-amd64/Packages": b"index"}, DATE)
        self.assertIn("Date: Sun, 22 Nov 2026 23:09:05 GMT\n", release)
        self.assertIn("Architectures: amd64 arm64\n", release)
        self.assertIn("Acquire-By-Hash: yes\n", release)
        digest = hashlib.sha256(b"index").hexdigest()
        self.assertTrue(release.endswith(f"SHA256:\n {digest} 5 main/binary-amd64/Packages\n"))

    def test_source_carries_the_key_as_a_folded_field(self):
        source = sources_text("https://example.org/apt", KEY)
        self.assertIn("URIs: https://example.org/apt\nSuites: stable\nComponents: main\n", source)
        self.assertIn("Signed-By:\n -----BEGIN PGP PUBLIC KEY BLOCK-----\n .\n mDMEabc\n", source)
        self.assertNotIn("\n\n", source)

    def test_packages_must_be_one_release(self):
        amd64 = package(self.directory)
        cases = [
            ([], "no packages"),
            ([amd64, package(self.directory, "arm64", "1.3.0-1")], "different versions: 1.2.0-1, 1.3.0-1"),
            ([amd64, amd64], "more than one package for an architecture"),
        ]
        for packages, message in cases:
            with self.subTest(message=message), self.assertRaisesRegex(RepositoryError, message):
                require_one_release(packages)

    def test_signing_key_must_be_the_only_one(self):
        key = "sec:u:255:22:AAAA:1:::u:::scESC::::::23::0:\nfpr:::::::::{0}:\nuid:u::::1::::T::::::::::0:\n"
        subkey = "ssb:u:255:18:BBBB:1::::::e::::::23:\nfpr:::::::::SUBKEY:\n"
        self.assertEqual(secret_key_fingerprint(key.format("PRIMARY") + subkey), "PRIMARY")
        for colons, count in [("", 0), (key.format("ONE") + key.format("TWO"), 2)]:
            with self.subTest(count=count), self.assertRaisesRegex(RepositoryError, f"found {count}"):
                secret_key_fingerprint(colons)

    def test_repository_holds_every_index_it_lists(self):
        out = self.directory / "apt"
        packages = [package(self.directory), package(self.directory, "arm64")]
        with mock.patch.object(build_apt_repository, "run", return_value=KEY) as run:
            build_repository(packages, "https://example.org/apt", out, "FINGERPRINT", DATE)
        distribution = out / "dists/stable"
        listed = (distribution / "Release").read_text().split("SHA256:\n")[1].splitlines()
        self.assertEqual(len(listed), 4)
        for line in listed:
            digest, size, path = line.split()
            contents = (distribution / path).read_bytes()
            self.assertEqual((hashlib.sha256(contents).hexdigest(), len(contents)), (digest, int(size)))
            self.assertEqual(((distribution / path).parent / "by-hash/SHA256" / digest).read_bytes(), contents)
        amd64 = distribution / "main/binary-amd64"
        self.assertEqual(gzip.decompress((amd64 / "Packages.gz").read_bytes()), (amd64 / "Packages").read_bytes())
        self.assertTrue((out / "pool/main/t/termeric/termeric_1.2.0-1_arm64.deb").is_file())
        self.assertTrue((out / "termeric.sources").is_file())
        self.assertEqual(
            [call.args[0][-1] for call in run.call_args_list],
            [str(distribution / "Release"), str(distribution / "Release"), "FINGERPRINT"],
        )

    def test_existing_output_is_left_alone(self):
        with self.assertRaisesRegex(RepositoryError, "already exists"):
            build_repository([package(self.directory)], "https://example.org", self.directory, "F", DATE)


if __name__ == "__main__":
    unittest.main()
