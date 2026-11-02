# holt die shim aus der signierten liste: erst die signatur prüfen, dann die datei
import hashlib
import json
import os
import sys
import urllib.parse
import urllib.request

from cryptography.exceptions import InvalidSignature
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PublicKey

HOST = "mods.badasswizard.boats"
INDEX_URL = f"https://{HOST}/index.json"
SIG_URL = f"https://{HOST}/index.sig"
OUT = os.path.join("src-tauri", "resources", "version.dll")
MAX_BYTES = 50 * 1024 * 1024


def fetch(url):
    # liest höchstens MAX_BYTES + 1, damit eine zu große datei auffällt
    with urllib.request.urlopen(url, timeout=60) as resp:
        return resp.read(MAX_BYTES + 1)


def check_host(url):
    parts = urllib.parse.urlsplit(url)
    if parts.scheme != "https" or parts.hostname != HOST:
        sys.exit(f"download url is not from the mod server: {url}")


def main():
    index_bytes = fetch(INDEX_URL)
    sig = bytes.fromhex(fetch(SIG_URL).decode("ascii").strip())

    key = Ed25519PublicKey.from_public_bytes(bytes.fromhex(os.environ["INDEX_PUBLIC_KEY_HEX"]))
    try:
        key.verify(sig, index_bytes)
    except InvalidSignature:
        sys.exit("mod list signature does not match, not building")

    shim = json.loads(index_bytes).get("shim")
    if not shim:
        sys.exit("signed mod list has no shim entry")
    check_host(shim["download_url"])

    data = fetch(shim["download_url"])
    if len(data) != shim["size"]:
        sys.exit("shim size does not match the signed mod list")
    if hashlib.sha256(data).hexdigest() != shim["sha256"].lower():
        sys.exit("shim checksum does not match the signed mod list")

    os.makedirs(os.path.dirname(OUT), exist_ok=True)
    with open(OUT, "wb") as f:
        f.write(data)
    print(f"verified shim, {len(data)} bytes")


if __name__ == "__main__":
    main()
