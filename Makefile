# lanpull root Makefile.
#
# Thin forwarder. Server targets are reachable as-is, client targets are
# prefixed with `client-` (for example `make client-lint`), and `ci`/`setup`
# run both sides. The target list is not duplicated here: `help` asks each
# side, so the surface cannot drift.

SHELL := /bin/sh

.PHONY: help doctor setup check test-all ci ci-full secrets docs-check check-scripts release release-check publish publish-draft hooks hooks-uninstall

help: ## Show available targets
	@printf 'Server targets:\n'
	@$(MAKE) --no-print-directory -C server help
	@printf '\nClient targets (make client-<target>):\n'
	@$(MAKE) --no-print-directory -C client help
	@printf '\nOther:\n'
	@printf '  %-16s %s\n' doctor "Print toolchain and optional tool status"
	@printf '  %-16s %s\n' check "Fast static checks on both sides (no tests)"
	@printf '  %-16s %s\n' test-all "Run the Rust and Python test suites"
	@printf '  %-16s %s\n' ci "Fast loop: static checks and both test suites"
	@printf '  %-16s %s\n' ci-full "The gate: ci, notices, audit-bin, e2e, secrets, docs"
	@printf '  %-16s %s\n' secrets "Scan the working tree for credentials (gitleaks or fallback)"
	@printf '  %-16s %s\n' docs-check "Check links, the public/private boundary, and the version"
	@printf '  %-16s %s\n' check-scripts "Parse and lint the repository shell scripts"
	@printf '  %-16s %s\n' release "Build the release archive (does not tag)"
	@printf '  %-16s %s\n' release-check "Rehearse the release layout from the working tree"
	@printf '  %-16s %s\n' publish "Publish the release to GitHub (needs gh auth and a tag)"
	@printf '  %-16s %s\n' publish-draft "Publish the release as a draft for review"
	@printf '  %-16s %s\n' hooks "Install the pre-push hook (fast gates and secret scan)"
	@printf '  %-16s %s\n' hooks-uninstall "Remove the pre-push hook"
	@printf '  %-16s %s\n' setup "Install both toolchains"

doctor: ## Print toolchain and optional tool status
	@$(MAKE) --no-print-directory -C server doctor
	@printf '\n'
	@$(MAKE) --no-print-directory -C client doctor

setup: ## Install both toolchains
	@$(MAKE) --no-print-directory -C server setup
	@$(MAKE) --no-print-directory -C client setup

check: check-scripts ## Run the fast static checks on both sides
	@$(MAKE) --no-print-directory -C server check-tools check-scripts check-core fmt-check machete lint
	@$(MAKE) --no-print-directory -C client lint typecheck

test-all: ## Run the Rust and Python test suites
	@$(MAKE) --no-print-directory -C server test
	@$(MAKE) --no-print-directory -C client test

ci: check-scripts ## Run every quality and security gate on both sides
	@$(MAKE) --no-print-directory -C server ci
	@$(MAKE) --no-print-directory -C client ci

# The single gate named in the project rules: cheap checks first, then the full suite,
# then the release-binary audit and the loopback end-to-end round. The full gate
# requires the real secret and shell linters; the fast loop keeps the fallbacks.
ci-full: export REQUIRE_GITLEAKS := 1
ci-full: export REQUIRE_SHELLCHECK := 1
ci-full: secrets docs-check ci
	@$(MAKE) --no-print-directory -C server licenses-check audit-bin e2e

secrets: ## Scan the working tree for credentials
	@scripts/secret-scan.sh

docs-check: ## Check links, the public/private boundary, and the version
	@scripts/docs-check.sh

check-scripts: ## Parse and lint the repository shell scripts
	@scripts/check-scripts.sh

release: ## Build the release archive (does not tag)
	@scripts/release.sh

release-check: ## Rehearse the release layout from the working tree
	@scripts/release.sh --check

publish: ## Publish the release to GitHub (needs gh auth)
	@scripts/publish.sh

publish-draft: ## Publish the release as a draft for review
	@scripts/publish.sh --draft

hooks: ## Install the pre-push hook
	@hook=$$(git rev-parse --git-path hooks/pre-push) || exit 1; \
	install -Dm755 scripts/git-hooks/pre-push "$$hook"; \
	printf 'installed %s\n' "$$hook"

hooks-uninstall: ## Remove the pre-push hook
	@hook=$$(git rev-parse --git-path hooks/pre-push) || exit 1; \
	rm -f "$$hook"; \
	printf 'removed %s\n' "$$hook"

client-%:
	@$(MAKE) --no-print-directory -C client $(patsubst client-%,%,$@)

%:
	@$(MAKE) --no-print-directory -C server $@
