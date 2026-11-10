#!/usr/bin/env python3
"""Mint a throwaway OIDC issuer for the WIF CI job (ci.yml s3-seaweedfs).

Generates an RSA keypair, writes the public half as <outdir>/jwks.json
(served to SeaweedFS as the provider's jwksUri), and signs a short-lived
RS256 JWT into <outdir>/token (the AWS_WEB_IDENTITY_TOKEN_FILE). The
private key never touches disk — this issuer exists for one CI job.

Usage: mint_oidc.py <outdir> [issuer] [audience]
Defaults match tests/wif/iam_config.json.
"""

import base64
import json
import sys
import time
from pathlib import Path

from cryptography.hazmat.primitives import hashes
from cryptography.hazmat.primitives.asymmetric import padding, rsa


def b64url(b: bytes) -> str:
    return base64.urlsafe_b64encode(b).rstrip(b"=").decode()


def b64url_json(obj) -> str:
    return b64url(json.dumps(obj, separators=(",", ":")).encode())


def b64url_uint(n: int) -> str:
    return b64url(n.to_bytes((n.bit_length() + 7) // 8, "big"))


def main() -> None:
    out = Path(sys.argv[1])
    out.mkdir(parents=True, exist_ok=True)
    issuer = sys.argv[2] if len(sys.argv) > 2 else "http://127.0.0.1:8481"
    audience = sys.argv[3] if len(sys.argv) > 3 else "skynbd-wif"

    key = rsa.generate_private_key(public_exponent=65537, key_size=2048)
    pub = key.public_key().public_numbers()
    kid = "skynbd-wif-test"

    jwks = {
        "keys": [
            {
                "kty": "RSA",
                "use": "sig",
                "alg": "RS256",
                "kid": kid,
                "n": b64url_uint(pub.n),
                "e": b64url_uint(pub.e),
            }
        ]
    }
    (out / "jwks.json").write_text(json.dumps(jwks))

    now = int(time.time())
    header = {"alg": "RS256", "typ": "JWT", "kid": kid}
    claims = {
        "iss": issuer,
        "aud": audience,
        "sub": "system:serviceaccount:ci:skynbd",
        "iat": now,
        "exp": now + 3600,
    }
    signing_input = f"{b64url_json(header)}.{b64url_json(claims)}".encode()
    sig = key.sign(signing_input, padding.PKCS1v15(), hashes.SHA256())
    (out / "token").write_text(f"{signing_input.decode()}.{b64url(sig)}")
    print(f"wrote {out}/jwks.json and {out}/token (iss={issuer}, aud={audience})")


if __name__ == "__main__":
    main()
