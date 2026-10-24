# lanpull server Makefile.
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
export PATH := $(CARGO_BIN):$(PATH)
CARGO       ?= $(if $(wildcard $(CARGO_BIN)/cargo),$(CARGO_BIN)/cargo,cargo)
RUSTC       ?= $(if $(wildcard $(CARGO_BIN)/rustc),$(CARGO_BIN)/rustc,rustc)
RUSTUP      ?= $(if $(wildcard $(CARGO_BIN)/rustup),$(CARGO_BIN)/rustup,rustup)
TARGET      ?= x86_64-unknown-linux-musl
MSRV        ?= 1.88.0
TOOLS_STAMP := target/.gate-tools
# Note: `clippy`, `test`, and `doc` run against the host toolchain (fast); the
# supply-chain gates (`deny`, `audit`) analyze the musl build graph, matching
# what `build` produces. A feature available only on one target could in
# principle differ, so the release binary stays validated by `e2e`/`audit-bin`.
# Quality-gate tools installed by `setup`. Versions are pinned so the gates are
# reproducible; bump a version here deliberately (TOOL_VERSIONS), never inline.
# `cargo-geiger` is an advisory inventory: it is installed by `setup` but is
# not part of `ci`, so `check-tools` does not require it.
GATE_TOOLS     := cargo-audit@0.22.2 cargo-deny@0.20.2 \
	cargo-auditable@0.7.7 cargo-machete@0.9.2 cargo-about@0.9.2
ADVISORY_TOOLS := cargo-geiger@0.13.0
TOOL_VERSIONS  := $(GATE_TOOLS) $(ADVISORY_TOOLS)
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
CLIENT_DIR     ?= ../client
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
        clean distclean confirm check-tools check-build-tools check-scripts check-core guard-test cli-test e2e dast load \
        release-setup licenses licenses-check msrv \
        fmt fmt-check lint doc test check-targets coverage bench fuzz machete deny audit audit-bin audit-config geiger security ci

help: ## Show available targets
	@$(SCRIPTS)/help.sh

doctor: ## Print the toolchain and optional tool status
	@printf 'server:\n'
	@printf '  cargo:    '; $(CARGO) --version
	@printf '  rustc:    '; $(RUSTC) --version 2>/dev/null || echo missing
	@printf '  rustup:   '; $(RUSTUP) show active-toolchain 2>/dev/null || echo missing
	@printf '  target:   '; $(RUSTUP) target list --installed 2>/dev/null | grep -qx '$(TARGET)' && echo '$(TARGET) installed' || echo '$(TARGET) missing'
	@for spec in $(TOOL_VERSIONS); do \
		name=$${spec%@*}; \
		if command -v $$name >/dev/null 2>&1; then \
			if [ "$$name" = "cargo-auditable" ]; then \
				printf '  %-20s %s\n' "$$name" "present"; \
			else \
				printf '  %-20s %s\n' "$$name" "$$($$name --version 2>/dev/null | head -n 1)"; \
			fi; \
		else \
			printf '  %-20s missing\n' "$$name"; \
		fi; \
	done
	@for name in shellcheck gitleaks testssl.sh git; do \
		if command -v $$name >/dev/null 2>&1; then \
			printf '  %-20s present\n' "$$name"; \
		else \
			printf '  %-20s missing (optional or full-gate tool)\n' "$$name"; \
		fi; \
	done

setup: ## Install the Rust target and the pinned quality-gate tools
	$(RUSTUP) target add $(TARGET)
	@mkdir -p target
	@if [ -f "$(TOOLS_STAMP)" ] && [ "$$(cat "$(TOOLS_STAMP)")" = "$(TOOL_VERSIONS)" ] \
		&& $(MAKE) --no-print-directory check-tools >/dev/null 2>&1 \
		&& (for spec in $(TOOL_VERSIONS); do \
			name=$${spec%@*}; command -v $$name >/dev/null 2>&1 || exit 1; \
		done); then \
		echo "lanpull: gate tools already installed"; \
	else \
		for tool in $(TOOL_VERSIONS); do \
			echo "lanpull: installing $$tool"; \
			case "$$tool" in \
				cargo-about@*) $(CARGO) install --locked --features cli $$tool || exit 1 ;; \
				*) $(CARGO) install --locked $$tool || exit 1 ;; \
			esac; \
		done; \
		printf '%s\n' "$(TOOL_VERSIONS)" > "$(TOOLS_STAMP)"; \
	fi

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
		--unit-src "$(abspath ../systemd/lanpull.service)" --unit-dest "$(UNIT)" \
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
	$(SCRIPTS)/distclean.sh ..

check-tools: check-build-tools
	@for spec in $(GATE_TOOLS); do \
		name=$${spec%@*}; want=$${spec#*@}; \
		command -v $$name >/dev/null 2>&1 || { \
			echo "lanpull: missing $$name; run 'make setup'" >&2; exit 1; \
		}; \
		if [ "$$name" = "cargo-auditable" ]; then \
			$(CARGO) install --list 2>/dev/null | grep -q "$$name v$$want" || { \
				echo "lanpull: $$name $$want is not installed; run 'make setup'" >&2; exit 1; \
			}; \
			continue; \
		fi; \
		got=$$($$name --version 2>/dev/null | awk '{print $$NF}'); \
		if [ "$$got" != "$$want" ]; then \
			echo "lanpull: $$name $$got is installed, want $$want; run 'make setup'" >&2; \
			exit 1; \
		fi; \
	done

check-core: ## Enforce that lanpull-core stays free of the HTTP stack
	@deps=$$($(CARGO) tree -p lanpull-core --edges all --prefix none) || { \
		echo "lanpull: cargo tree failed; cannot verify lanpull-core" >&2; exit 1; }; \
	if printf '%s\n' "$$deps" | grep -Eq '^(axum|axum-server|tower|tower-http|hyper|hyper-util) v'; then \
		echo "lanpull: lanpull-core must not depend on the HTTP stack" >&2; \
		printf '%s\n' "$$deps" | grep -E '^(axum|axum-server|tower|tower-http|hyper|hyper-util) v' >&2; \
		exit 1; \
	fi
	@for crate in lanpull-store lanpull-cli; do \
		deps=$$($(CARGO) tree -p $$crate --depth 1 --edges normal --prefix none) || { \
			echo "lanpull: cargo tree failed for $$crate" >&2; exit 1; }; \
		if printf '%s\n' "$$deps" | grep -Eq '^(axum|axum-server) v'; then \
			echo "lanpull: $$crate must not depend on axum directly" >&2; \
			printf '%s\n' "$$deps" | grep -E '^(axum|axum-server) v' >&2; \
			exit 1; \
		fi; \
	done

check-scripts: ## Parse the server shell scripts (shellcheck runs when installed)
	@for script in $(SCRIPTS)/*.sh; do sh -n "$$script" || exit 1; done
	@if command -v shellcheck >/dev/null 2>&1; then \
		shellcheck -x -s sh -P $(SCRIPTS) $(SCRIPTS)/*.sh || exit 1; \
	elif [ "$${REQUIRE_SHELLCHECK:-0}" = "1" ]; then \
		echo "lanpull: shellcheck is required by the full gate; install it and re-run" >&2; \
		exit 1; \
	else \
		echo "lanpull: shellcheck not installed; linting the scripts by 'sh -n' only" >&2; \
	fi

guard-test: ## Run negative safety tests for the privileged scripts
	$(SCRIPTS)/guard_test.sh

e2e: build ## Run the loopback end-to-end smoke test (dev only; needs python3)
	$(SCRIPTS)/e2e.sh "$(abspath $(BINARY))" "$(abspath $(CLIENT_DIR))"

LOAD_CLIENTS ?= 8
load: build ## Run concurrent loopback clients (periodic; not a gate)
	$(SCRIPTS)/load_test.sh "$(abspath $(BINARY))" "$(abspath $(CLIENT_DIR))" "$(LOAD_CLIENTS)"

dast: build ## Run the local DAST checks (testssl.sh + HTTP probes; not a gate)
	@command -v testssl.sh >/dev/null 2>&1 || [ -n "$$TESTSSL" ] || { \
		echo "lanpull: missing testssl.sh; set TESTSSL=/path/to/testssl.sh" >&2; exit 1; }
	$(SCRIPTS)/dast.sh "$(abspath $(BINARY))" "$(abspath $(CLIENT_DIR))"

fmt: ## Format Rust sources (workspace and the detached fuzz crate)
	$(CARGO) fmt --all
	$(CARGO) fmt --manifest-path fuzz/Cargo.toml

fmt-check: ## Check Rust formatting without changing files
	$(CARGO) fmt --all --check
	$(CARGO) fmt --manifest-path fuzz/Cargo.toml --check

lint: ## Run Clippy on all targets with warnings denied
	$(CARGO) clippy --locked --all-targets --all-features -- -D warnings

doc: ## Build docs and deny warnings (catches broken intra-doc links)
	RUSTDOCFLAGS="-D warnings" $(CARGO) doc --no-deps --locked

test: ## Run the Rust test suite
	$(CARGO) test --locked --workspace

check-targets: ## Compile tests, benches, and examples without running them
	$(CARGO) test --locked --workspace --all-targets --no-run

msrv: ## Check the declared MSRV (requires the MSRV toolchain)
	@$(RUSTUP) toolchain list | grep -q "^$(MSRV)" || { \
		echo "lanpull: Rust $(MSRV) is not installed; run '$(RUSTUP) toolchain install $(MSRV) --profile minimal'" >&2; \
		exit 1; \
	}
	$(CARGO) +$(MSRV) check --locked --workspace --all-targets

bench: ## Run the criterion benchmarks (advisory; not a gate)
	$(CARGO) bench --locked --workspace

coverage: ## Report test coverage (floor 53% lines; not a gate)
	@command -v cargo-llvm-cov >/dev/null 2>&1 || { \
		echo "lanpull: missing cargo-llvm-cov; install it with 'cargo install cargo-llvm-cov --locked' and 'rustup component add llvm-tools-preview'" >&2; \
		exit 1; \
	}
	$(CARGO) llvm-cov --locked --workspace --summary-only --fail-under-lines 53

FUZZ_TARGETS := config glob manifest relpath
FUZZ_TIME    ?= 20

release-setup: ## Install cargo-about only (already part of setup)
	$(CARGO) install --locked --features cli cargo-about@0.9.2

licenses: ## Regenerate THIRD_PARTY_LICENSES.md from Cargo.lock
	@command -v cargo-about >/dev/null 2>&1 || { \
		echo "lanpull: missing cargo-about; run 'make -C server release-setup'" >&2; exit 1; \
	}
	$(CARGO) about generate --manifest-path Cargo.toml --config about.toml about.hbs \
		-o ../THIRD_PARTY_LICENSES.md

licenses-check: ## Fail when THIRD_PARTY_LICENSES.md is stale
	@command -v cargo-about >/dev/null 2>&1 || { \
		echo "lanpull: missing cargo-about; run 'make -C server release-setup'" >&2; exit 1; \
	}
	@set -e; tmp=$$(mktemp); trap 'rm -f "$$tmp"' EXIT; \
		$(CARGO) about generate --manifest-path Cargo.toml --config about.toml about.hbs -o "$$tmp"; \
		cmp -s "$$tmp" ../THIRD_PARTY_LICENSES.md || { \
			echo "lanpull: THIRD_PARTY_LICENSES.md is stale; run 'make -C server licenses'" >&2; \
			exit 1; \
		}

fuzz: ## Run a short fuzz smoke over the parsers (nightly + cargo-fuzz; not a gate)
	@command -v cargo-fuzz >/dev/null 2>&1 || { \
		echo "lanpull: missing cargo-fuzz; run 'cargo install cargo-fuzz --locked' and 'rustup toolchain install nightly'" >&2; \
		exit 1; \
	}
	@for target in $(FUZZ_TARGETS); do \
		echo "fuzz: $$target ($(FUZZ_TIME)s)"; \
		$(CARGO) +nightly fuzz run $$target -- -max_total_time=$(FUZZ_TIME) -rss_limit_mb=2048 || exit 1; \
	done

machete: ## Check for unused dependencies in every crate
	$(CARGO) machete

cli-test: build-debug ## Run the operator-style CLI test against the debug binary
	$(SCRIPTS)/cli_test.sh "$(abspath $(BINARY_DEBUG))"

deny: ## Enforce the dependency policy (advisories, licenses, bans, sources)
	$(CARGO) deny check

audit: ## Scan dependencies for known vulnerabilities
	$(CARGO) audit

audit-config: ## Check that cargo-audit and cargo-deny ignore the same advisories
	$(SCRIPTS)/check-advisory-ignores.sh

audit-bin: build ## Audit the compiled binary's embedded dependency list
	$(CARGO) audit bin $(BINARY)

geiger: ## Report unsafe usage across the dependency tree (advisory; not a gate)
	@command -v cargo-geiger >/dev/null 2>&1 || { \
		echo "lanpull: missing cargo-geiger; run 'make setup'" >&2; exit 1; }
	@for member in $(CRATES); do \
		echo "geiger: $$member"; \
		(cd $(CRATE_DIR)/$$member && $(CARGO) geiger --forbid-only) || exit 1; \
	done

security: deny audit audit-bin ## Run every blocking supply-chain security check

ci: check-tools check-scripts check-core guard-test fmt-check deny audit-config machete lint test check-targets doc cli-test ## Run the fast Rust quality and security gate
