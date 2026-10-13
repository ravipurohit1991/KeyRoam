# Security policy

## Pre-release warning

KeyRoam has **not yet been independently security-reviewed**. Until the
review in milestone 8 is complete:

- Store only **synthetic** (made-up) credentials in KeyRoam.
- Do not rely on KeyRoam as your only copy of any secret.

## Reporting a vulnerability

Please report suspected vulnerabilities privately through GitHub's
"Report a vulnerability" (Security → Advisories) on this repository. Do not open
a public issue. Include steps to reproduce and the affected version or commit.

## Design

The cryptographic design, threat model and known limitations are documented in
[docs/security.md](docs/security.md).
