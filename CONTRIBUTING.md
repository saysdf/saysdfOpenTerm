# Contributing to saysdfTerm

Issues and pull requests are welcome.

## How changes get in

saysdfTerm is developed in a private repository and published here in batches, one commit per
release (`vX.Y.Z`). Because of that, pull requests are not merged on GitHub directly: when a PR
is accepted, the maintainer applies it in the private repository, and it appears in the next
release with credit to you (`Co-authored-by` in the release commit and a line in
[CHANGELOG](saysdf/public/CHANGELOG.md)). The PR is then closed with a link to that release.

## Before you open a PR

- For anything larger than a small fix, open an issue first to agree on the approach.
- Keep upstream WezTerm files changed as little as possible; fork features live in `saysdf/`
  (Lua modules in `saysdf/lua/`, tools in `saysdf/cli/`). Mark each change to an upstream file
  with a `// saysdfTerminal: <why>` comment.
- Requirements are in [openspec/specs/](openspec/specs/); a behaviour change should update them.
- Security problems: do not open an issue or PR, see [SECURITY.md](SECURITY.md).

## Building and testing

See [README.md](README.md#building). Tests run with cargo:

```sh
cargo test -p <crate>          # one crate, e.g. saysdf-lua-bundle or wezterm-gui
cargo test --workspace         # everything (slow)
```

On Windows, `saysdf/tools/check.ps1 -Crate <crate>` does the same with a job limit that keeps
the machine responsive.

There is no CI: please run the tests for the crates you changed before sending a PR and say in
the PR which OS you tested on.

## License

By contributing you agree that your contribution is licensed under the MIT license of this
project ([LICENSE.md](LICENSE.md)).
