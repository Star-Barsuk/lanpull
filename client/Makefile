# lanpull client Makefile.
# Static analysis, tests, and dependency audit for the Python client.
# The tools live in a uv-managed virtual environment (`make setup`).

SHELL := /bin/sh

VENV       ?= .venv
BIN        := $(VENV)/bin
UV         ?= uv
PYTHON     := $(BIN)/python
RUFF       := $(BIN)/ruff
MYPY       := $(BIN)/mypy
PYTEST     := $(BIN)/pytest
PIP_AUDIT  := $(BIN)/pip-audit

.PHONY: help setup lint typecheck test audit ci

help: ## Show available targets
	@grep -E '^[a-zA-Z_-]+:.*?## ' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "  %-14s %s\n", $$1, $$2}'

setup: ## Create the dev environment and install pinned tools
	$(UV) venv $(VENV)
	$(UV) pip install --python $(PYTHON) -r requirements-dev.txt

lint: ## Lint and format-check the client
	$(RUFF) check .
	$(RUFF) format --check .

typecheck: ## Static type-check the client
	$(MYPY) .

test: ## Run the client tests
	$(PYTEST)

audit: ## Audit the development tools for known vulnerabilities
	$(PIP_AUDIT) -r requirements-dev.txt

ci: lint typecheck test audit ## Run every client quality and security gate
