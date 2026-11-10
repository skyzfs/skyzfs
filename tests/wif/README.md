# WIF conformance rig (ci.yml `s3-seaweedfs`)

Proves the ambient-credentials path (auth design §5) takes the
**web-identity** link of the AWS default chain, not the env-keys link the
MinIO job exercises: the CI job exports `AWS_WEB_IDENTITY_TOKEN_FILE`,
`AWS_ROLE_ARN`, and `AWS_ENDPOINT_URL_STS` — and no static keys — so the
SDK must call STS `AssumeRoleWithWebIdentity` to get session credentials
before any S3 request can succeed.

Pieces:

- `mint_oidc.py` — throwaway OIDC issuer: RSA keypair, `jwks.json`
  (served over `python3 -m http.server`), and a signed one-hour JWT.
- `iam_config.json` — SeaweedFS `-s3.iam.config`: trusts the throwaway
  issuer via its JWKS, and maps `SkynbdWifRole` (the `AWS_ROLE_ARN` the
  test assumes) to full S3 access.
- The env-gated test `s3_conformance_web_identity` in
  `crates/skynbd-store/src/s3.rs`, which refuses to run if
  `AWS_ACCESS_KEY_ID` is set (that would let the chain short-circuit and
  prove nothing).

SeaweedFS is pinned in `ci.yml`; bump deliberately — the STS/IAM surface
is newer than the S3 core (the SigV4 session-token fix landed in 4.x,
Jan 2026).
