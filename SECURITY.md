# Security policy

## Reporting a vulnerability

Report it privately through a
[GitHub Security Advisory](https://github.com/y0sif/lawh/security/advisories/new), not a public issue.
Expect a first reply within 48 hours.

## Scope

lawh is a local desktop app with no accounts and no server. The risks come from opening files
someone else made.

**In scope:**

- A `.lawh` file that makes lawh read or write files, reach the network, or run code outside the
  Typst sandbox when you open it.
- Math blocks: Typst code inside a board must not escape the sandbox (no file system, no package
  downloads, bundled fonts only).
- Parser bugs that crash lawh or corrupt a board on save.
- A file that hangs or exhausts memory when opened (denial of service).

**Out of scope:**

- Issues in dependencies that lawh doesn't expose. Report those upstream.
- The OS warning on first launch: lawh releases are deliberately unsigned.
