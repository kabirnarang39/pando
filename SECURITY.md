# Security Policy

## Supported versions

| Version | Supported |
|---|---|
| 1.x | ✅ |

## Reporting a vulnerability

Please **do not** open a public issue. Use GitHub's
[private vulnerability reporting](https://github.com/kabirnarang39/sapwood/security/advisories/new).
You will receive an acknowledgement within 72 hours and a fix timeline after triage.

## Security model (v1)

* **Authentication**: bearer tokens (`SAPWOOD_TOKENS`, `principal:token` pairs), compared in
  constant time. Run with TLS (`--tls-cert/--tls-key`) whenever traffic leaves the host.
  Without `SAPWOOD_TOKENS` the server allows anonymous access and logs a warning.
* **SSO**: OpenID Connect bearer tokens (`--oidc-issuer/--oidc-audience`). Keys come from the
  issuer's JWKS and rotate automatically; only asymmetric algorithms matching the key are
  accepted (no `alg: none`, no HMAC key confusion); `iss`, `aud` and `exp` are required. The
  principal is a configurable claim and works with the access rules below. Static tokens and
  OIDC can be enabled together.
* **Authorization**: per-directory write permissions and per-ref permissions
  (`--acl rules.json`). Read access is repository-wide in v1.
* **Integrity**: every object is hash-verified when accepted by a server, when received
  by a tier from upstream, and when materialised by a client.
* **Client filesystem safety**: paths are validated (`..`, `.sap`, separators, NUL) and no
  write follows a symlink, so a malicious tree cannot write outside the working copy.
* **Resource limits**: per-object size cap (256 MiB; larger files are chunked) with bounded decompression; bounded
  ids per request, history scan, find and blame work per request.
