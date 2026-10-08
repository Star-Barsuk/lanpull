# lanpull client Makefile.
# Static analysis, tests, and dependency audit for the Python client.
#
# The gates bootstrap their own uv-managed virtual environment: the stamp file
# is a prerequisite of every gate, so `make ci` works from a clean tree.

SHELL := /bin/sh

VENV       ?= .venv
BIN        := $(VENV)/bin
STAMP      := $(VENV)/.stamp
# Self-locate uv (commonly ~/.local/bin/uv) so the documented gates work
# without editing PATH first; fall back to whatever is on PATH.
UV         ?= $(if $(wildcard $(HOME)/.local/bin/uv),$(HOME)/.local/bin/uv,uv)
PYTHON     := $(BIN)/python
RUFF       := $(BIN)/ruff
MYPY       := $(BIN)/mypy
PYTEST     := $(BIN)/pytest
PIP_AUDIT  := $(BIN)/pip-audit

.PHONY: help setup check-tools lint typecheck test audit clean ci

help: ## Show available targets
	@awk 'BEGIN { FS = " ## " } /^[a-zA-Z0-9_-]+:.*## / { split($$1, t, ":"); printf "  %-16s %s\n", t[1], $$2 }' $(MAKEFILE_LIST)

check-tools: ## Fail early when uv or python3 is missing
	@command -v $(UV) >/dev/null 2>&1 || { \
		echo "lanpull: missing $(UV); install it and re-run (see README.md)" >&2; exit 1; }
	@command -v python3 >/dev/null 2>&1 || { \
		echo "lanpull: missing python3" >&2; exit 1; }

$(STAMP): requirements-dev.txt | check-tools
	@test -d $(VENV) || $(UV) venv $(VENV)
	$(UV) pip install --python $(PYTHON) -r requirements-dev.txt
	@touch $(STAMP)

setup: check-tools $(STAMP) ## Create the dev environment and install pinned tools

lint: $(STAMP) ## Lint and format-check the client
	$(RUFF) check .
	$(RUFF) format --check .

typecheck: $(STAMP) ## Static type-check the client
	$(MYPY) .

test: $(STAMP) ## Run the client tests
	$(PYTEST)

audit: $(STAMP) ## Audit the development tools for known vulnerabilities
	$(PIP_AUDIT) -r requirements-dev.txt

clean: ## Remove the virtual environment and Python caches
	rm -rf $(VENV) __pycache__ .pytest_cache .mypy_cache .ruff_cache
	find . -name '*.pyc' -delete

ci: check-tools lint typecheck test audit ## Run every client quality and security gate
