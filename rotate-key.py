#!/usr/bin/env python3
"""Rotate bridge NaCl key daily. Saves new seed to ~/.retana/bridge_nacl.key
and prints the new public key. Bridge picks up new key on next restart."""
from nacl.public import PrivateKey
from nacl.encoding import RawEncoder
import os, sys

key_path = os.path.expanduser("~/.retana/bridge_nacl.key")

sk = PrivateKey.generate()
seed = bytes(sk)
pubkey_hex = bytes(sk.public_key).hex()

os.makedirs(os.path.dirname(key_path), exist_ok=True)
with open(key_path, "w") as f:
    f.write(seed.hex())

print(f"pubkey: {pubkey_hex}", file=sys.stderr)
print(pubkey_hex)
