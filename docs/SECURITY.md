# Security policy

## Supported versions

lanpull is pre-1.0; only the latest revision is supported. There are no
backports.

## Reporting a vulnerability

Please do not open a public issue for a security problem. Report it privately
through GitHub's advisory flow:

<https://github.com/Star-Barsuk/lanpull/security/advisories/new>

Include:

- a description of the vulnerability and its impact;
- steps to reproduce, or a proof of concept;
- the affected version and the relevant configuration.

You will get an acknowledgement, and a fix or mitigation as soon as practical.

## Scope and assumptions

lanpull is designed for a trusted local network with a trusted operator. The
"Security model" section of [`README.md`](../README.md#security-model) describes
what it does and does not protect; reports that only restate those documented
limits (for example, an attacker who already controls a client, or who steals a
registered IP during the arm window) are out of scope.
