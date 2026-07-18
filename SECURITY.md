# Security Policy

## Supported versions

konoma is distributed through [crates.io](https://crates.io/crates/konoma) and
[GitHub Releases](https://github.com/LESIM-Co-Ltd/konoma/releases). Security fixes
land on the latest release; please upgrade to the newest version before reporting.

## Reporting a vulnerability

Please report security issues **privately** — do not open a public issue for them.

Use GitHub's private vulnerability reporting:
[**Report a vulnerability**](https://github.com/LESIM-Co-Ltd/konoma/security/advisories/new).
This opens a private advisory visible only to the maintainers.

Please include, as far as you can:

- the konoma version (`konoma` shows it in the crate metadata / release tag),
- your macOS and terminal (e.g. Ghostty) versions,
- a minimal reproduction (a file or directory layout that triggers it), and
- the impact you observed.

We aim to acknowledge a report within a few business days and to keep you updated
as we work on a fix.

## Scope notes

konoma previews arbitrary local files and delegates some previews to optional
external tools (`git`, `poppler`, `ffmpeg`, an editor). Relevant classes of issue
include: a crafted file that crashes the app instead of degrading to the safe
`[can not preview]` fallback, path handling that escapes the intended directory,
or an argument passed to a delegated command in a way that could be abused. Reports
in these areas are especially welcome.
