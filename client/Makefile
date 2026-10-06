# lanpull client Makefile.
# Static analysis, tests, and dependency audit for the Python client.
#
# The gates bootstrap their own uv-managed virtual environment: the stamp file
# is a prerequisite of every gate, so `make ci` works from a clean tree.

SHELL := /bin/sh

VENV       ?= .venv
BIN        := $(VENV)/bin
STAMP      := $(VENV)/.stamp
UV         ?= uv
PYTHON     := $(BIN)/python
RUFF       := $(BIN)/ruff
MYPY       := $(BIN)/mypy
PYTEST     := $(BIN)/pytest
PIP_AUDIT  := $(BIN)/pip-audit

.PHONY: help setup lint typecheck test audit clean ci

help: ## Show available targets
	@awk 'BEGIN { FS = " ## " } /^[a-zA-Z0-9_-]+:.*## / { split($$1, t, ":"); printf "  %-16s %s\n", t[1], $$2 }' $(MAKEFILE_LIST)

$(STAMP): requirements-dev.txt
	@test -d $(VENV) || $(UV) venv $(VENV)
	$(UV) pip install --python $(PYTHON) -r requirements-dev.txt
	@touch $(STAMP)

setup: $(STAMP) ## Create the dev environment and install pinned tools

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

ci: lint typecheck test audit ## Run every client quality and security gate
