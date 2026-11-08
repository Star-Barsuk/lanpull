# Contributing to lanpull

Thanks for your interest. This is a small, focused project: please keep changes
minimal and in scope.

## Before you start

- Open an issue to discuss a non-trivial change before writing code.
- Keep the public surface documented: update `README.md` and
  [the reference](REFERENCE.md) when behavior, flags, configuration keys, or
  error messages change.

## Quality gate

Every change must pass the operator's private quality gate before it is merged.
The gate is not part of this repository.

```bash
make setup             # pinned Rust toolchain
make test-all          # Rust and Python test suites
scripts/gate-e2e.sh    # loopback end-to-end round
```

The Rust toolchain and the Python development tools are pinned to known-good
versions. Bump them deliberately, never inline, and re-run the private gate.
The toolchain also requires `gitleaks` and `shellcheck`; install both before
claiming a green gate.

## Review checklist

- The behavior change has a test that fails without it.
- Authentication, path validation, TLS, and file writes have a negative test.
- `README.md` and `docs/REFERENCE.md` are updated if a flag, a key, or an error
  message changed.
- The private gate is green.
- No secret, home path, real hostname, or personal data was added.
- A decision is recorded when a tradeoff was made.

## Style

- All code, comments, documentation, and commit messages are in English.
- Follow the conventions already present in the surrounding code.
- Do not commit machine-local secrets: `/etc/lanpull/lanpull.conf`,
  `lanpull.clients`, `lanpull.access.json`, `auth`, or `server.key`.

## License

By contributing, you agree that your changes are licensed under the MIT License
(see [`LICENSE`](../LICENSE)).
