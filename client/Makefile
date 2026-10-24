# lanpull client Makefile.
# Static analysis, tests, and dependency audit for the Python client.
#
# The gates bootstrap their own uv-managed virtual environment on the minimum
# supported Python (3.11), so a gate failure reflects the target runtime rather
# than the developer's interpreter. The stamp is a prerequisite of every gate,
# so `make ci` works from a clean tree.

SHELL := /bin/sh

VENV           ?= .venv
PYTHON_VERSION ?= 3.11
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

.PHONY: help doctor setup check-tools format lint typecheck test coverage audit lock-check clean ci

help: ## Show available targets
	@awk 'BEGIN { FS = " ## " } /^[a-zA-Z0-9_-]+:.*## / { split($$1, t, ":"); printf "  %-16s %s\n", t[1], $$2 }' $(MAKEFILE_LIST)

doctor: ## Print the client development environment status
	@printf 'client:\n'
	@printf '  uv:       '; $(UV) --version 2>/dev/null || echo missing
	@printf '  venv:     '; if [ -d $(VENV) ]; then echo present; else echo 'missing (run make setup)'; fi
	@printf '  python:   '; if [ -x $(PYTHON) ]; then $(PYTHON) --version; else echo 'venv python missing'; fi

check-tools:
	@command -v $(UV) >/dev/null 2>&1 || { \
		echo "lanpull: missing $(UV); install it and re-run (see README.md)" >&2; exit 1; }
	@command -v python3 >/dev/null 2>&1 || { \
		echo "lanpull: missing python3" >&2; exit 1; }

$(STAMP): requirements-dev.in requirements-dev.txt pyproject.toml | check-tools
	@test -d $(VENV) || $(UV) venv --python $(PYTHON_VERSION) $(VENV)
	$(UV) pip install --require-hashes --python $(PYTHON) -r requirements-dev.txt
	@touch $(STAMP)

setup: check-tools $(STAMP) ## Create the dev environment and install pinned tools

format: $(STAMP) ## Format the client sources
	$(RUFF) format .

lint: $(STAMP) ## Lint and format-check the client
	$(RUFF) check .
	$(RUFF) format --check .

typecheck: $(STAMP) ## Static type-check the client
	$(MYPY) .

test: $(STAMP) ## Run the client tests
	$(PYTEST)

coverage: $(STAMP) ## Run the client tests with coverage (floor 80%; not a gate)
	$(PYTEST) --cov=pull --cov-report=term-missing --cov-fail-under=80

audit: $(STAMP) ## Audit the development tools for known vulnerabilities
	$(PIP_AUDIT) -r requirements-dev.txt

lock-check: check-tools ## Fail when requirements-dev.txt is stale
	@tmp=$$(mktemp); expected=$$(mktemp); \
	trap 'rm -f "$$tmp" "$$expected"' EXIT; \
	$(UV) pip compile --no-header --generate-hashes --python-version $(PYTHON_VERSION) \
		requirements-dev.in -o "$$tmp" >/dev/null; \
	sed '1,2d' requirements-dev.txt >"$$expected"; \
	cmp -s "$$tmp" "$$expected" || { \
		echo "lanpull: requirements-dev.txt is stale; regenerate it from requirements-dev.in" >&2; \
		exit 1; \
	}

clean: ## Remove the virtual environment, coverage output, and Python caches
	rm -rf $(VENV) __pycache__ .pytest_cache .mypy_cache .ruff_cache \
		.coverage .coverage.* htmlcov coverage.xml
	find . -name '*.pyc' -delete

ci: check-tools lock-check lint typecheck test audit ## Run every client quality and security gate
