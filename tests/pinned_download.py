"""Bounded, resumable public-fixture download; only exact pinned bytes are exposed."""
import hashlib
import re
import urllib.error
import urllib.request


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def download(url, destination, size, sha256, block_bytes=8*1024*1024):
    if size <= 0 or block_bytes <= 0 or not re.fullmatch(r"[0-9a-f]{64}", sha256):
        raise ValueError("Invalid pinned public-fixture contract")
    if destination.exists() and destination.stat().st_size == size and digest(destination) == sha256:
        return
    destination.parent.mkdir(parents=True, exist_ok=True)
    part = destination.with_suffix(destination.suffix + ".part")
    rejected = destination.with_suffix(destination.suffix + ".rejected")
    if part.exists() and part.stat().st_size > size:
        part.replace(rejected)
    failures = 0
    while (offset := part.stat().st_size if part.exists() else 0) < size:
        end = min(offset + block_bytes, size) - 1
        request = urllib.request.Request(url, headers={"Range": f"bytes={offset}-{end}", "Accept-Encoding": "identity"})
        try:
            with urllib.request.urlopen(request, timeout=90) as response:
                if response.status == 206:
                    match = re.fullmatch(r"bytes (\d+)-(\d+)/(\d+)", response.headers.get("Content-Range", ""))
                    if not match:
                        raise ValueError("Missing or invalid public-fixture Content-Range")
                    start, stop, total = map(int, match.groups())
                    if start != offset or not start <= stop <= end or total != size:
                        raise ValueError("Public-fixture range does not match the pinned request")
                    expected = stop-start+1
                elif response.status == 200 and offset == 0:
                    expected = size
                else:
                    raise ValueError("Server ignored the public-fixture resume offset")
                length = response.headers.get("Content-Length")
                if length is not None and int(length) != expected:
                    raise ValueError("Unexpected public-fixture response length")
                received = 0
                with part.open("ab") as output:
                    while data := response.read(min(1024*1024, expected-received+1)):
                        if received+len(data) > expected:
                            raise ValueError("Public-fixture response exceeded its declared range")
                        output.write(data)
                        received += len(data)
                if received != expected:
                    raise OSError("Public-fixture connection ended before its declared length")
        except (urllib.error.URLError, TimeoutError, OSError):
            failures += 1
            if failures >= 3:
                raise RuntimeError(f"Public fixture interrupted after {part.stat().st_size if part.exists() else 0} of {size} bytes; rerun to resume") from None
    actual_size = part.stat().st_size
    actual_sha = digest(part)
    if actual_size != size or actual_sha != sha256:
        part.replace(rejected)
        raise ValueError(f"Public fixture checksum mismatch: {actual_size} bytes, SHA-256 {actual_sha}; rejected bytes retained")
    part.replace(destination)
