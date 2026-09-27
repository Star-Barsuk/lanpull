# lanpull server Makefile.
# Lifecycle for the Rust server and the Rust quality gates.
# Run from this directory (`make -C server ...`) or through the root forwarder.
#
# The lifecycle targets need the `server/` crate and a built `lanpull` binary;
# they become functional in Phase 1.

SHELL := /bin/sh

LANPULL_CONF ?= ../config/lanpull.conf
LANPULL_CONF_ABS := $(abspath $(LANPULL_CONF))
-include $(LANPULL_CONF)

BIN_DIR    ?= /usr/local/bin
LANPULL    ?= $(BIN_DIR)/lanpull
UNIT       ?= /etc/systemd/system/lanpull.service
LANPULL_USER ?= $(shell id -un)
LANPULL_CLIENTS ?= ../config/lanpull.clients
CARGO      ?= cargo
TARGET     ?= x86_64-unknown-linux-musl
CLIENT_DIR ?= ../client
STATE_DIR  ?= /var/lib/lanpull
STATE_DIR_ABS := $(abspath $(STATE_DIR))
TOOLS      ?= cargo-audit cargo-deny cargo-geiger cargo-auditable

# Destructive targets (distclean, uninstall, wipe) refuse to run unless the
# operator passes CONFIRM=1. The SHARE=1 flag additionally removes $SHARE_DIR.
CONFIRM ?=
SHARE   ?=

.PHONY: help deps build install up down restart status rescan add-client remove-client \
        list-clients passwd arm disarm report cert client-bundle logs clean config \
        distclean uninstall wipe confirm \
        fmt fmt-check lint doc test deny audit audit-bin geiger security ci

help: ## Show available targets
	@grep -E '^[a-zA-Z_-]+:.*?## ' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "  %-14s %s\n", $$1, $$2}'

deps: ## Install the Rust target and the quality-gate tools
	rustup target add $(TARGET)
	$(CARGO) install --locked $(TOOLS)

build: ## Build the static server binary with embedded dependency metadata
	$(CARGO) auditable build --release --locked --target $(TARGET)

install: build ## Install the binary, the systemd unit, and stage the client bundle
	install -Dm755 target/$(TARGET)/release/lanpull $(LANPULL)
	install -Dm644 ../systemd/lanpull.service $(UNIT)
	sed -i -e 's|__LANPULL_USER__|$(LANPULL_USER)|g' \
	       -e 's|__LANPULL_CONF__|$(LANPULL_CONF_ABS)|g' \
	       -e 's|__LANPULL_STATE_DIR__|$(STATE_DIR_ABS)|g' $(UNIT)
	$(MAKE) client-bundle

up: ## Start the service (no autostart)
	systemctl start lanpull.service

down: ## Stop the service
	systemctl stop lanpull.service

restart: ## Restart the service
	systemctl restart lanpull.service

status: ## Warn on a stale manifest and show armed clients
	$(LANPULL) status --config $(LANPULL_CONF)

rescan: ## Regenerate the manifest
	$(LANPULL) manifest --config $(LANPULL_CONF)

add-client: ## Create an account: make add-client NAME=<client-name> [IP=<client-ip>] OUTPUT=<output-dir>
	$(LANPULL) add-client $(NAME) $(if $(IP),--ip $(IP),) --output $(OUTPUT) --config $(LANPULL_CONF)

remove-client: ## Revoke an account: make remove-client NAME=<client-name>
	$(LANPULL) remove-client $(NAME) --config $(LANPULL_CONF)

list-clients: ## List accounts
	$(LANPULL) list-clients --config $(LANPULL_CONF)

passwd: ## Rotate a password: make passwd NAME=<client-name>
	$(LANPULL) passwd $(NAME) --config $(LANPULL_CONF)

arm: ## Arm an account: make arm NAME=<client-name> TTL=15m
	$(LANPULL) arm $(NAME) --ttl $(TTL) --config $(LANPULL_CONF)

disarm: ## Clear an arm window: make disarm NAME=<client-name>
	$(LANPULL) disarm $(NAME) --config $(LANPULL_CONF)

report: ## Show who pulled what
	$(LANPULL) report --config $(LANPULL_CONF)

cert: ## Generate the self-signed TLS certificate
	$(LANPULL) cert --config $(LANPULL_CONF)

client-bundle: ## Stage client/ into $STATE_DIR/client
	install -Dm755 $(CLIENT_DIR)/pull.py $(STATE_DIR)/client/pull.py
	install -Dm644 $(CLIENT_DIR)/VERSION $(STATE_DIR)/client/VERSION

logs: ## Follow the service log
	journalctl -u lanpull.service -f

clean: ## Remove build artifacts and caches (server target and client venv)
	$(CARGO) clean
	$(MAKE) -C $(CLIENT_DIR) clean

confirm: ## Refuse a destructive target unless CONFIRM=1 is given
	@if [ "$(CONFIRM)" != "1" ]; then \
		echo "lanpull: this target deletes configuration, credentials, and/or state."; \
		echo "         re-run with CONFIRM=1 to proceed (SHARE=1 also removes SHARE_DIR)."; \
		exit 1; \
	fi

config: ## Generate config/lanpull.conf for this machine
	scripts/init-config.sh --config $(LANPULL_CONF)

distclean: confirm ## Remove build artifacts, generated config, and accounts (CONFIRM=1)
	$(MAKE) clean
	rm -f $(LANPULL_CONF) $(LANPULL_CLIENTS)
	find .. -path ../.git -prune -o -path ../target -prune \
		-o -path ../server/target -prune -o -path ../client/.venv -prune \
		-o -type f \( -name '.lanpull.lock' -o -name 'state.json' \
		-o -name '*.part' -o -name '.manifest.tmp' \) \
		-print -exec rm -f {} +

uninstall: confirm ## Remove the installed binary, unit, and state (CONFIRM=1, SHARE=1)
	@[ -n "$(STATE_DIR)" ] && [ "$(STATE_DIR)" != "/" ] \
		|| { echo "lanpull: refusing to remove STATE_DIR='$(STATE_DIR)'"; exit 1; }
	-systemctl stop lanpull.service
	-systemctl disable lanpull.service
	-rm -f $(UNIT)
	-systemctl daemon-reload
	rm -f $(LANPULL)
	rm -rf $(STATE_DIR)
	@if [ "$(SHARE)" = "1" ]; then \
		[ -n "$(SHARE_DIR)" ] && [ "$(SHARE_DIR)" != "/" ] \
			|| { echo "lanpull: refusing to remove SHARE_DIR='$(SHARE_DIR)'"; exit 1; }; \
		echo "removing $(SHARE_DIR)"; rm -rf $(SHARE_DIR); \
	fi

wipe: distclean uninstall ## Remove everything lanpull created (CONFIRM=1, SHARE=1)

fmt: ## Format Rust sources
	$(CARGO) fmt --all

fmt-check: ## Check Rust formatting without changing files
	$(CARGO) fmt --all --check

lint: ## Run Clippy on all targets with warnings denied
	$(CARGO) clippy --locked --all-targets --all-features -- -D warnings

doc: ## Build docs and deny warnings (catches broken intra-doc links)
	RUSTDOCFLAGS="-D warnings" $(CARGO) doc --no-deps --locked

test: ## Run the Rust test suite
	$(CARGO) test --locked --workspace

deny: ## Enforce the dependency policy (advisories, licenses, bans, sources)
	$(CARGO) deny check

audit: ## Scan dependencies for known vulnerabilities
	$(CARGO) audit

audit-bin: build ## Audit the compiled binary's embedded dependency list
	$(CARGO) audit bin target/$(TARGET)/release/lanpull

geiger: ## Report unsafe usage across the dependency tree
	$(CARGO) geiger --forbid-only

security: deny audit audit-bin geiger ## Run every supply-chain security check

ci: fmt-check lint doc test security ## Run every Rust quality and security gate
