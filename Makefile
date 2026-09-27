# lanpull root Makefile.
# Thin forwarder: the server and the client each have their own Makefile.
# Server targets are forwarded so the documented `make <target>` commands keep
# working from the repository root.

SHELL := /bin/sh

SERVER_TARGETS := deps build install up down restart status rescan \
                  add-client remove-client list-clients passwd arm disarm \
                  report cert client-bundle logs clean config distclean \
                  uninstall wipe \
                  fmt fmt-check lint doc test deny audit audit-bin geiger security

CLIENT_TARGETS := client-lint client-typecheck client-test client-audit

.PHONY: help ci $(SERVER_TARGETS) $(CLIENT_TARGETS)

help: ## Show available targets
	@printf 'Server targets (forwarded to server/):\n'
	@printf '  %s\n' '$(SERVER_TARGETS)'
	@printf '\nClient targets (forwarded to client/):\n'
	@printf '  client-lint client-typecheck client-test client-audit\n'
	@printf '\nOther:\n'
	@printf '  ci   Run every quality and security gate on both sides\n'

$(SERVER_TARGETS):
	@$(MAKE) --no-print-directory -C server $@

client-lint:
	@$(MAKE) --no-print-directory -C client lint

client-typecheck:
	@$(MAKE) --no-print-directory -C client typecheck

client-test:
	@$(MAKE) --no-print-directory -C client test

client-audit:
	@$(MAKE) --no-print-directory -C client audit

ci: ## Run every quality and security gate on both sides
	@$(MAKE) --no-print-directory -C server ci
	@$(MAKE) --no-print-directory -C client ci
