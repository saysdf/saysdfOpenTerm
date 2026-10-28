# Security policy

## Supported versions

Only the latest release (the newest `vX.Y.Z` tag) gets security fixes. Fixes are published
in the next release; there are no backports to older versions.

## Reporting a vulnerability

Please report vulnerabilities privately through GitHub:
[Security → Report a vulnerability](https://github.com/saysdf/saysdfOpenTerm/security/advisories/new).
Do not open a public issue for a security problem.

Useful details: the version (`saysdfTerm-cli --version`), the OS, steps or a minimal input that
reproduces the problem (for example, the escape sequence or file that triggers it), and what an
attacker gains.

You will get an answer as soon as the maintainer can look at it. When a fix is released, the
advisory is published with credit to the reporter unless you ask otherwise.

## Scope

In scope: the terminal and its bundled tools in this repository — handling of untrusted terminal
output (escape sequences, OSC, images), the bundled Lua modules, the `saysdf` CLI, secret storage,
SSH and database features, the bootstrap scripts.

Problems that also exist in upstream [WezTerm](https://github.com/wezterm/wezterm) are welcome
here too; please consider reporting them upstream as well.
