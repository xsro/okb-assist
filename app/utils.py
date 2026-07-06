import hashlib


def calculate_file_hash(file_path: str) -> str:
    """Calculate SHA256 hash of a file.

    Uses a 4096-byte buffer for memory efficiency.
    This is the canonical hash function shared by server and client scripts.
    """
    sha256_hash = hashlib.sha256()
    with open(file_path, "rb") as f:
        for byte_block in iter(lambda: f.read(4096), b""):
            sha256_hash.update(byte_block)
    return sha256_hash.hexdigest()
