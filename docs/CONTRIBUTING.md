# Contributing to lanpull

Thanks for your interest. This is a small, focused project: please keep changes
minimal and in scope.

## Before you start

- Read [the engineering standard](ENGINEERING_STANDARD.md). It is the process
  contract for this repository.
- Open an issue to discuss a non-trivial change before writing code.
- Keep the public surface documented: update `README.md` and
  [the reference](REFERENCE.md) when behavior, flags, configuration keys, or
  error messages change.

## Quality gate

Every change must pass the gate before it is merged:

```bash
make setup             # pinned Rust toolchain and gate tools
make ci-full           # secret scan, docs check, both sides, loopback e2e
```

`make ci` is the fast inner loop (both sides, no end-to-end round and no secret
scan); it is not sufficient on its own. `make hooks` installs an optional
`pre-push` hook with the fast gates and the secret scan; it is a safety net,
not the gate.

The Rust toolchain, the gate tools, and the Python development tools are pinned
to known-good versions. Bump them deliberately, never inline, and re-run
`make ci-full`.

## Review checklist

- The behavior change has a test that fails without it.
- Authentication, path validation, TLS, and file writes have a negative test.
- `README.md` and `docs/REFERENCE.md` are updated if a flag, a key, or an error
  message changed.
- `make ci-full` is green.
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
