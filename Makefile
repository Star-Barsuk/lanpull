# lanpull root Makefile.
#
# Thin forwarder. Server targets are reachable as-is, client targets are
# prefixed with `client-` (for example `make client-lint`), and `ci`/`setup`
# run both sides. The target list is not duplicated here: `help` asks each
# side, so the surface cannot drift.

SHELL := /bin/sh

.PHONY: help setup ci ci-full secrets docs-check check-scripts hooks hooks-uninstall

help: ## Show available targets
	@printf 'Server targets:\n'
	@$(MAKE) --no-print-directory -C server help
	@printf '\nClient targets (make client-<target>):\n'
	@$(MAKE) --no-print-directory -C client help
	@printf '\nOther:\n'
	@printf '  %-16s %s\n' ci "Fast inner loop: both sides, no e2e or secret scan"
	@printf '  %-16s %s\n' ci-full "The gate: ci, loopback e2e, and the secret scan"
	@printf '  %-16s %s\n' secrets "Scan tracked files for credentials (gitleaks or fallback)"
	@printf '  %-16s %s\n' docs-check "Check links, the public/private boundary, and the version"
	@printf '  %-16s %s\n' check-scripts "Parse and lint the repository shell scripts"
	@printf '  %-16s %s\n' hooks "Install the pre-push hook (fast gates and secret scan)"
	@printf '  %-16s %s\n' hooks-uninstall "Remove the pre-push hook"
	@printf '  %-16s %s\n' setup "Install both toolchains"

setup: ## Install both toolchains
	@$(MAKE) --no-print-directory -C server setup
	@$(MAKE) --no-print-directory -C client setup

ci: check-scripts ## Run every quality and security gate on both sides
	@$(MAKE) --no-print-directory -C server ci
	@$(MAKE) --no-print-directory -C client ci

# The single gate named in the project rules: cheap checks first, then the full suite,
# then the loopback end-to-end round.
ci-full: secrets docs-check ci
	@$(MAKE) --no-print-directory -C server e2e

secrets: ## Scan tracked files for credentials
	@scripts/secret-scan.sh

docs-check: ## Check links, the public/private boundary, and the version
	@scripts/docs-check.sh

check-scripts: ## Parse and lint the repository shell scripts
	@scripts/check-scripts.sh

hooks: ## Install the pre-push hook
	@install -Dm755 scripts/git-hooks/pre-push .git/hooks/pre-push
	@printf 'installed .git/hooks/pre-push\n'

hooks-uninstall: ## Remove the pre-push hook
	@rm -f .git/hooks/pre-push
	@printf 'removed .git/hooks/pre-push\n'

client-%:
	@$(MAKE) --no-print-directory -C client $(patsubst client-%,%,$@)

%:
	@$(MAKE) --no-print-directory -C server $@
