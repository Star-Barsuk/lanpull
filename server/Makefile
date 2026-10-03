# lanpull server Makefile.
# Thin dispatcher: one target per operation. Non-trivial, privileged, or
# destructive logic lives in scripts/ and is invoked from one-line recipes.
#
# The Makefile locates the Rust toolchain itself ($CARGO_HOME/bin), so the
# documented commands work without editing PATH first.

SHELL := /bin/sh

LANPULL_CONF ?= ../config/lanpull.conf
LANPULL_CONF_ABS := $(abspath $(LANPULL_CONF))
-include $(LANPULL_CONF)

# --- Toolchain (self-locating) -------------------------------------------
CARGO_HOME  ?= $(HOME)/.cargo
CARGO_BIN   := $(CARGO_HOME)/bin
export PATH := $(CARGO_BIN):$(PATH)
CARGO       ?= $(if $(wildcard $(CARGO_BIN)/cargo),$(CARGO_BIN)/cargo,cargo)
RUSTUP      ?= $(if $(wildcard $(CARGO_BIN)/rustup),$(CARGO_BIN)/rustup,rustup)
TARGET      ?= x86_64-unknown-linux-musl
TOOLS       ?= cargo-audit cargo-deny cargo-geiger cargo-auditable
BINARY      := target/$(TARGET)/release/lanpull

# --- Install locations ---------------------------------------------------
BIN_DIR        ?= /usr/local/bin
LANPULL        ?= $(BIN_DIR)/lanpull
SERVICE        ?= lanpull.service
UNIT           ?= /etc/systemd/system/$(SERVICE)
OPERATOR       ?= $(if $(SUDO_USER),$(SUDO_USER),$(shell id -un))
OPERATOR_GROUP ?= $(shell id -gn $(OPERATOR) 2>/dev/null || id -gn)
LANPULL_CLIENTS ?= ../config/lanpull.clients
CLIENT_DIR     ?= ../client
STATE_DIR      ?= /var/lib/lanpull
STATE_DIR_ABS  := $(abspath $(STATE_DIR))
SHARES_FILE    ?= $(LANPULL_CONF_ABS).shares

CONFIRM ?=
SHARE   ?=

SCRIPTS := scripts
CONFIG_ARGS ?=

# The share list is materialized before any removal, because `wipe` deletes the
# configuration (and so the share definitions) before `uninstall` runs.
UNINSTALL_CMD = $(SCRIPTS)/uninstall.sh --binary "$(LANPULL)" --unit "$(UNIT)" \
	--state-dir "$(STATE_DIR_ABS)" --shares-file "$(SHARES_FILE)" \
	--share "$(SHARE)" --service "$(SERVICE)"

.PHONY: help setup build config client-bundle install uninstall wipe \
        up down restart logs status rescan cert \
        add-client remove-client list-clients grant revoke passwd arm disarm report \
        clean distclean confirm check-tools \
        fmt fmt-check lint doc test deny audit audit-bin geiger security ci

help: ## Show available targets
	@$(SCRIPTS)/help.sh

setup: ## Install the Rust target and the quality-gate tools
	$(RUSTUP) target add $(TARGET)
	$(CARGO) install --locked $(TOOLS)

build: ## Build the static server binary with embedded dependency metadata
	$(CARGO) auditable build --release --locked --target $(TARGET)

config: ## Generate config/lanpull.conf for this machine
	$(SCRIPTS)/init-config.sh --config $(LANPULL_CONF_ABS) $(CONFIG_ARGS)

client-bundle: ## Stage client/ into $STATE_DIR/client
	install -Dm755 $(CLIENT_DIR)/pull.py $(STATE_DIR)/client/pull.py
	install -Dm644 $(CLIENT_DIR)/VERSION $(STATE_DIR)/client/VERSION

install: build ## Install the binary, unit, and client bundle (escalates as needed)
	$(SCRIPTS)/install.sh \
		--binary "$(BINARY)" --bin-dir "$(BIN_DIR)" \
		--unit-src "$(abspath ../systemd/lanpull.service)" --unit-dest "$(UNIT)" \
		--state-dir "$(STATE_DIR_ABS)" --operator "$(OPERATOR)" \
		--group "$(OPERATOR_GROUP)" --conf "$(LANPULL_CONF_ABS)" \
		--client-dir "$(CLIENT_DIR)"

uninstall: confirm ## Remove the binary, unit, and state (CONFIRM=1, SHARE=1)
	@if [ "$(SHARE)" = "1" ]; then \
		$(SCRIPTS)/shares.sh "$(LANPULL_CONF_ABS)" >"$(SHARES_FILE)" || exit 1; \
	fi
	$(UNINSTALL_CMD)
	@rm -f "$(SHARES_FILE)"

wipe: confirm ## Remove everything lanpull created (CONFIRM=1, SHARE=1)
	@if [ "$(SHARE)" = "1" ]; then \
		$(SCRIPTS)/shares.sh "$(LANPULL_CONF_ABS)" >"$(SHARES_FILE)" || exit 1; \
	fi
	$(MAKE) distclean CONFIRM=1
	$(UNINSTALL_CMD)
	@rm -f "$(SHARES_FILE)"

up: ## Start the service (no autostart)
	$(SCRIPTS)/service.sh start $(SERVICE)

down: ## Stop the service
	$(SCRIPTS)/service.sh stop $(SERVICE)

restart: ## Restart the service
	$(SCRIPTS)/service.sh restart $(SERVICE)

logs: ## Follow the service log
	$(SCRIPTS)/service.sh logs $(SERVICE)

status: ## Warn on a stale manifest and show armed clients
	$(LANPULL) status --config $(LANPULL_CONF_ABS)

rescan: ## Regenerate the manifest
	$(LANPULL) manifest --config $(LANPULL_CONF_ABS)

cert: ## Generate the self-signed TLS certificate
	$(LANPULL) cert --config $(LANPULL_CONF_ABS)

add-client: ## Create an account: make add-client NAME=<client-name> [IP=<client-ip>] OUTPUT=<output-dir>
	$(LANPULL) add-client $(NAME) $(if $(IP),--ip $(IP),) --output $(OUTPUT) --config $(LANPULL_CONF_ABS)

remove-client: ## Revoke an account: make remove-client NAME=<client-name>
	$(LANPULL) remove-client $(NAME) --config $(LANPULL_CONF_ABS)

list-clients: ## List accounts, IPs, and access rules
	$(LANPULL) list-clients --config $(LANPULL_CONF_ABS)

grant: ## Add an access rule: make grant NAME=<client-name> RULE=<share>[:<glob>]
	$(LANPULL) grant $(NAME) "$(RULE)" --config $(LANPULL_CONF_ABS)

revoke: ## Remove access rules: make revoke NAME=<client-name> [RULE=<share>[:<glob>]]
	$(LANPULL) revoke $(NAME) $(if $(RULE),"$(RULE)",) --config $(LANPULL_CONF_ABS)

passwd: ## Rotate a password: make passwd NAME=<client-name>
	$(LANPULL) passwd $(NAME) --config $(LANPULL_CONF_ABS)

arm: ## Arm an account: make arm NAME=<client-name> TTL=15m
	$(LANPULL) arm $(NAME) --ttl $(TTL) --config $(LANPULL_CONF_ABS)

disarm: ## Clear an arm window: make disarm NAME=<client-name>
	$(LANPULL) disarm $(NAME) --config $(LANPULL_CONF_ABS)

report: ## Show who pulled what
	$(LANPULL) report --config $(LANPULL_CONF_ABS)

clean: ## Remove build artifacts and caches (no root)
	$(CARGO) clean
	$(MAKE) -C $(CLIENT_DIR) clean

confirm: ## Refuse a destructive target unless CONFIRM=1 is given
	@if [ "$(CONFIRM)" != "1" ]; then \
		echo "lanpull: this target deletes configuration, credentials, and/or state."; \
		echo "         re-run with CONFIRM=1 to proceed (SHARE=1 also removes SHARE_DIR)."; \
		exit 1; \
	fi

distclean: confirm ## Remove build artifacts, generated config, and accounts (CONFIRM=1)
	$(MAKE) clean
	rm -f $(LANPULL_CONF) $(LANPULL_CLIENTS)
	$(SCRIPTS)/distclean.sh ..

check-tools: ## Fail early when a gate tool is missing (run make setup)
	@for tool in $(TOOLS); do \
		command -v $$tool >/dev/null 2>&1 || { \
			echo "lanpull: missing $$tool; run 'make setup'" >&2; exit 1; \
		}; \
	done

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
	$(CARGO) audit bin $(BINARY)

geiger: ## Report unsafe usage across the dependency tree
	$(CARGO) geiger --forbid-only

security: deny audit audit-bin geiger ## Run every supply-chain security check

ci: check-tools fmt-check lint doc test security ## Run every Rust quality and security gate
