# Security policy

Report a vulnerability privately through GitHub's
[private vulnerability reporting](https://github.com/y0sif/lawh/security/advisories/new), not in a
public issue.

lawh treats boards and formulas as untrusted: opening one must never read or write other files,
reach the network or run code outside the Typst sandbox. Typst math compiles in a sandboxed worker
process, whose memory cap holds on Linux only so far. The threat model, with every cap, is in
[docs/architecture.md](docs/architecture.md#threat-model).
