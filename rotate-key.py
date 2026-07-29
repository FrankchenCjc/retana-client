#!/usr/bin/env python3
"""Generate new NaCl keypair and save as NEXT key.
Bridge detects this file, encrypts the new pubkey with current key,
sends key_rot to all clients, then swaps next→current."""
from nacl.public import PrivateKey
from nacl.encoding import RawEncoder
import os

next_path = os.path.expanduser("~/.retana/bridge_nacl_next.key")

sk = PrivateKey.generate()
seed = bytes(sk)
pubkey_hex = bytes(sk.public_key).hex()

os.makedirs(os.path.dirname(next_path), exist_ok=True)
with open(next_path, "w") as f:
    f.write(seed.hex())

print(pubkey_hex)
