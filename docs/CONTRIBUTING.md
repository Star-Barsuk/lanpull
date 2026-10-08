# Contributing to lanpull

Thanks for your interest. This is a small, focused project — please keep changes
minimal and in scope.

## Before you start

- Open an issue to discuss a non-trivial change before writing code.
- Keep the public surface documented: update `README.md` and
  `docs/REFERENCE.md` when you change behavior, flags, or configuration keys.

## Quality gate

Every change must pass the quality gate before it is merged. Run the full gate,
or one side only:

```bash
make ci                # server + client
make -C server ci      # shell scripts, rustfmt, Clippy, rustdoc, tests, machete, deny, audit, geiger
make -C client ci      # ruff check/format, mypy --strict, pytest, pip-audit
```

`make setup` and `make -C client setup` install the pinned toolchains and gate
tools. The Rust toolchain and the gate tools are pinned to known-good versions;
bump them deliberately, never inline, and re-run `make ci`.

## Style

- All code, comments, documentation, and commit messages are in English.
- Follow the conventions already present in the surrounding code.
- Do not commit machine-local secrets: `/etc/lanpull/lanpull.conf`,
  `lanpull.clients`, `lanpull.access.json`, `auth`, or `server.key`.

## License

By contributing, you agree that your changes are licensed under the MIT License
(see [`LICENSE`](../LICENSE)).
