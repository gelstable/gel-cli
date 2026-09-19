import hashlib
import tempfile
import unittest
from pathlib import Path

from gel_release import digests


class DigestFileTests(unittest.TestCase):
    def test_digest_matches_hashlib(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "payload.bin"
            payload = b"gel" * 1000
            path.write_bytes(payload)

            result = digests.digest_file(path)

            self.assertEqual(result.size, len(payload))
            self.assertEqual(result.sha256, hashlib.sha256(payload).hexdigest())
            self.assertEqual(
                result.blake2b512,
                hashlib.blake2b(payload, digest_size=64).hexdigest(),
            )
            self.assertEqual(len(result.sha256), 64)
            self.assertEqual(len(result.blake2b512), 128)
            self.assertEqual(result.sha256, result.sha256.lower())


class SumsFileTests(unittest.TestCase):
    def _populate(self, root: Path) -> None:
        (root / "b.txt").write_bytes(b"beta")
        (root / "a.txt").write_bytes(b"alpha")
        (root / "SHA256SUMS").write_bytes(b"placeholder")

    def test_sums_are_sorted_and_skip_listed_names(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self._populate(root)
            out = root / "SHA256SUMS"

            digests.write_sums(root, "sha256", out, skip={"SHA256SUMS"})

            lines = out.read_bytes().decode().splitlines()
            self.assertEqual([line.split("  ")[1] for line in lines], ["a.txt", "b.txt"])
            self.assertTrue(out.read_bytes().endswith(b"\n"))
            self.assertNotIn(b"\r", out.read_bytes())
            self.assertEqual(lines[0].split("  ")[0], hashlib.sha256(b"alpha").hexdigest())

    def test_blake2b_sums_use_128_hex_characters(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self._populate(root)
            out = root / "BLAKE2B512SUMS"

            digests.write_sums(root, "blake2b512", out, skip={"SHA256SUMS"})

            first = out.read_bytes().decode().splitlines()[0]
            self.assertEqual(len(first.split("  ")[0]), 128)

    def test_verify_round_trip(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self._populate(root)
            out = root / "SHA256SUMS"
            digests.write_sums(root, "sha256", out, skip={"SHA256SUMS"})

            digests.verify_sums(root, out)

            (root / "a.txt").write_bytes(b"tampered")
            with self.assertRaises(digests.DigestMismatch):
                digests.verify_sums(root, out)


if __name__ == "__main__":
    unittest.main()
