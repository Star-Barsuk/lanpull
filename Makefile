# lanpull root Makefile.
#
# Thin forwarder. Server targets are reachable as-is, client targets are
# prefixed with `client-` (for example `make client-lint`), and `ci`/`setup`
# run both sides. The target list is not duplicated here: `help` asks each
# side, so the surface cannot drift.

SHELL := /bin/sh

.PHONY: help setup ci

help: ## Show available targets
	@printf 'Server targets:\n'
	@$(MAKE) --no-print-directory -C server help
	@printf '\nClient targets (make client-<target>):\n'
	@$(MAKE) --no-print-directory -C client help
	@printf '\nOther:\n'
	@printf '  %-16s %s\n' ci "Run every quality and security gate on both sides"
	@printf '  %-16s %s\n' setup "Install both toolchains"

setup: ## Install both toolchains
	@$(MAKE) --no-print-directory -C server setup
	@$(MAKE) --no-print-directory -C client setup

ci: ## Run every quality and security gate on both sides
	@$(MAKE) --no-print-directory -C server ci
	@$(MAKE) --no-print-directory -C client ci

client-%:
	@$(MAKE) --no-print-directory -C client $(patsubst client-%,%,$@)

%:
	@$(MAKE) --no-print-directory -C server $@
