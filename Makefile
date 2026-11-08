# lanpull Makefile.
# Thin dispatcher: one target per operation. Non-trivial, privileged, or
# destructive logic lives in scripts/ and is invoked from one-line recipes.
#
# This Makefile only works with the source tree: it builds the binary, installs
# it, and runs the quality gates. It never generates a configuration. After
# `make install`, the operator drives the server with the installed `lanpull`
# binary (see the `lanpull --help` tree); the configuration is created by
# `sudo lanpull init`.
#
# The Makefile locates the Rust toolchain itself ($CARGO_HOME/bin), so the
# documented commands work without editing PATH first.

SHELL := /bin/sh

# --- Toolchain (self-locating) -------------------------------------------
# Under sudo, HOME may be root's, so resolve the invoking user's home (the
# Makefile already reads SUDO_USER for OPERATOR). CARGO_HOME and RUSTUP_HOME are
# exported so a privileged invocation uses the invoking user's toolchain instead
# of creating a second one under /root. Prefer a plain `make install` (the
# install script escalates only the file-system steps) over `sudo make install`:
# a build performed as root would leave root-owned files in target/.
LANPULL_HOME ?= $(if $(SUDO_USER),$(shell getent passwd $(SUDO_USER) | cut -d: -f6),$(HOME))
CARGO_HOME  ?= $(LANPULL_HOME)/.cargo
export CARGO_HOME
RUSTUP_HOME ?= $(LANPULL_HOME)/.rustup
export RUSTUP_HOME
CARGO_BIN   := $(CARGO_HOME)/bin
export PATH := $(CARGO_BIN):$(LANPULL_HOME)/.local/bin:$(PATH)
CARGO       ?= $(if $(wildcard $(CARGO_BIN)/cargo),$(CARGO_BIN)/cargo,cargo)
RUSTC       ?= $(if $(wildcard $(CARGO_BIN)/rustc),$(CARGO_BIN)/rustc,rustc)
RUSTUP      ?= $(if $(wildcard $(CARGO_BIN)/rustup),$(CARGO_BIN)/rustup,rustup)
TARGET      ?= x86_64-unknown-linux-musl
BINARY       := target/$(TARGET)/release/lanpull
BINARY_DEBUG := target/debug/lanpull

# --- Install locations ---------------------------------------------------
BIN_DIR        ?= /usr/local/bin
LANPULL        ?= $(BIN_DIR)/lanpull
SERVICE        ?= lanpull.service
UNIT           ?= /etc/systemd/system/$(SERVICE)
CONFIG_DIR     ?= /etc/lanpull
OPERATOR       ?= $(if $(SUDO_USER),$(SUDO_USER),$(shell id -un))
OPERATOR_GROUP ?= $(shell id -gn $(OPERATOR) 2>/dev/null || id -gn)
CLIENT_DIR     ?= client
STATE_DIR      ?= /var/lib/lanpull
STATE_DIR_ABS  := $(abspath $(STATE_DIR))
CONFIG_DIR_ABS := $(abspath $(CONFIG_DIR))
CONFIRM ?=

SCRIPTS := scripts
CRATE_DIR := crates
CRATES    := core store http cli

UNINSTALL_CMD = $(SCRIPTS)/uninstall.sh --binary "$(LANPULL)" --unit "$(UNIT)" \
	--state-dir "$(STATE_DIR_ABS)" --conf-dir "$(CONFIG_DIR_ABS)" \
	--service "$(SERVICE)"

.PHONY: help doctor setup build build-debug install uninstall wipe \
        clean distclean confirm check-build-tools dast load soak \
        test-musl lint-whitespace check-git

help: ## Show available targets
	@$(SCRIPTS)/help.sh

doctor: ## Print the toolchain and optional tool status
	@printf 'server:\n'
	@printf '  cargo:    '; $(CARGO) --version
	@printf '  rustc:    '; $(RUSTC) --version 2>/dev/null || echo missing
	@printf '  rustup:   '; $(RUSTUP) show active-toolchain 2>/dev/null || echo missing
	@printf '  target:   '; $(RUSTUP) target list --installed 2>/dev/null | grep -qx '$(TARGET)' && echo '$(TARGET) installed' || echo '$(TARGET) missing'
	@for name in shellcheck gitleaks testssl.sh git; do \
		if command -v $$name >/dev/null 2>&1; then \
			printf '  %-20s present\n' "$$name"; \
		else \
			printf '  %-20s missing (optional or project tool)\n' "$$name"; \
		fi; \
	done

setup: ## Install the Rust target
	$(RUSTUP) target add $(TARGET)

build: check-build-tools ## Build the static server binary with embedded dependency metadata
	$(CARGO) auditable build --release --locked --target $(TARGET)

build-debug: check-build-tools ## Build the host debug binary for the CLI test
	$(CARGO) build --locked

check-build-tools:
	@command -v $(CARGO) >/dev/null 2>&1 || { \
		echo "lanpull: cargo is missing; install Rust and re-run 'make setup'" >&2; exit 1; }
	@$(CARGO) auditable --help >/dev/null 2>&1 || { \
		echo "lanpull: cargo-auditable is missing; run 'make setup'" >&2; exit 1; }
	@$(RUSTUP) target list --installed 2>/dev/null | grep -qx '$(TARGET)' || { \
		echo "lanpull: Rust target $(TARGET) is missing; run 'make setup'" >&2; exit 1; }

install: build ## Install the binary, unit, config dir, and client bundle
	$(SCRIPTS)/install.sh \
		--binary "$(BINARY)" --bin-dir "$(BIN_DIR)" \
		--unit-src "$(abspath ./systemd/lanpull.service)" --unit-dest "$(UNIT)" \
		--state-dir "$(STATE_DIR_ABS)" --operator "$(OPERATOR)" \
		--group "$(OPERATOR_GROUP)" --conf-dir "$(CONFIG_DIR_ABS)" \
		--client-dir "$(CLIENT_DIR)"

uninstall: confirm ## Remove the binary, unit, /etc/lanpull, and state (CONFIRM=1)
	$(UNINSTALL_CMD)

wipe: confirm ## Remove everything lanpull created (CONFIRM=1)
	$(MAKE) distclean CONFIRM=1
	$(UNINSTALL_CMD)

clean: ## Remove build artifacts and caches (no root)
	$(CARGO) clean
	rm -f ./*.profraw
	@if [ -d "$(CLIENT_DIR)" ]; then \
		$(MAKE) -C $(CLIENT_DIR) clean; \
	else \
		echo "lanpull: skipping client clean (no $(CLIENT_DIR))" >&2; \
	fi

confirm:
	@if [ "$(CONFIRM)" != "1" ]; then \
		echo "lanpull: this target deletes configuration, credentials, and/or state."; \
		echo "         re-run with CONFIRM=1 to proceed."; \
		exit 1; \
	fi

distclean: confirm ## Remove build artifacts and repository runtime leftovers (CONFIRM=1)
	$(MAKE) clean
	$(SCRIPTS)/distclean.sh .

LOAD_CLIENTS ?= 8
SOAK_ROUNDS  ?= 3
load: build ## Run concurrent loopback clients (periodic; not a gate)
	$(SCRIPTS)/load_test.sh "$(abspath $(BINARY))" "$(abspath $(CLIENT_DIR))" "$(LOAD_CLIENTS)"

soak: build ## Repeat the concurrent load smoke (periodic; not a gate)
	@round=1; while [ "$$round" -le "$(SOAK_ROUNDS)" ]; do \
		echo "soak: round $$round/$(SOAK_ROUNDS)"; \
		$(SCRIPTS)/load_test.sh "$(abspath $(BINARY))" "$(abspath $(CLIENT_DIR))" "$(LOAD_CLIENTS)" || exit 1; \
		round=$$((round + 1)); \
	done

dast: build ## Run the local DAST checks (testssl.sh + HTTP probes; not a gate)
	@command -v testssl.sh >/dev/null 2>&1 || [ -n "$$TESTSSL" ] || { \
		echo "lanpull: missing testssl.sh; set TESTSSL=/path/to/testssl.sh" >&2; exit 1; }
	$(SCRIPTS)/dast.sh "$(abspath $(BINARY))" "$(abspath $(CLIENT_DIR))"

test-musl: check-build-tools ## Run the test suite against the musl target
	$(CARGO) test --locked --workspace --target $(TARGET)
















lint-whitespace: ## Reject whitespace errors in the working diff
	@git diff --check

check-git: ## Fail when the working tree is not clean
	@git diff --quiet && git diff --cached --quiet || { echo "working tree not clean" >&2; exit 1; }
