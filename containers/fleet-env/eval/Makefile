# Fleet behaviour-eval — orchestration entrypoint (Track F).
#
#   make eval                  full matrix, all reports, GC after
#   make eval PARALLEL=12      tune concurrency
#   make eval SCENARIOS=base BEHAVIOURS=terminal.new
#   make eval RETRIES=2        retry-on-flake (re-run failed rows, see below)
#   make list                  print registered scenarios + behaviours
#   make soak ROUNDS=5         run the matrix N times (stability / leak hunt)
#   make gc                    remove orphan eval containers + dangling images
#   make clean                 gc + wipe the artifacts dir
#
# Reports land in $(OUT): eval.json (§3.5), eval.xml (JUnit), eval.html (row report),
# index.html (screenshot review). run.mjs auto-emits XML/HTML via FLEET_EVAL_* and
# the JSON via --json. Exit code is non-zero on any unexpected fail/error.

SHELL      := /bin/bash
NODE       ?= node
PARALLEL   ?= 4
RETRIES    ?= 0
ROUNDS     ?= 3
OUT        ?= /tmp/fleet-eval
IMAGE      ?= fleet-env:latest

# Optional selectors → CLI flags only when set.
SCENARIOS  ?=
BEHAVIOURS ?=
TAGS       ?=
KEEP       ?=

SEL :=
ifneq ($(strip $(SCENARIOS)),)
SEL += --scenarios $(SCENARIOS)
endif
ifneq ($(strip $(BEHAVIOURS)),)
SEL += --behaviours $(BEHAVIOURS)
endif
ifneq ($(strip $(TAGS)),)
SEL += --tags $(TAGS)
endif
ifneq ($(strip $(KEEP)),)
SEL += --keep
endif

JSON  := $(OUT)/eval.json
JUNIT := $(OUT)/eval.xml
HTML  := $(OUT)/eval.html
REVIEW := $(OUT)/index.html

# Exported so run.mjs's Reporter.finish() emits the XML/HTML without extra flags.
export FLEET_EVAL_OUT   := $(OUT)
export FLEET_EVAL_JUNIT := $(JUNIT)
export FLEET_EVAL_HTML  := $(HTML)
export FLEET_EVAL_REVIEW := $(REVIEW)

.PHONY: eval list soak gc clean reports help install

help:
	@grep -E '^#( |$$)' Makefile | sed -E 's/^# ?//'

# Full matrix. RETRIES>0 → if the first pass exits non-zero, re-run ONLY the failed
# rows (parsed out of eval.json) up to RETRIES times; a row that passes on any
# attempt is treated as a (flaky) pass. GC always runs afterwards.
eval:
	@mkdir -p $(OUT)
	@set -e; \
	echo "[make] eval: parallel=$(PARALLEL) retries=$(RETRIES) image=$(IMAGE)"; \
	rc=0; \
	$(NODE) run.mjs --parallel $(PARALLEL) --json $(JSON) $(SEL) || rc=$$?; \
	attempt=0; \
	while [ $$rc -ne 0 ] && [ $$attempt -lt $(RETRIES) ]; do \
	  attempt=$$((attempt+1)); \
	  failed=$$($(NODE) scripts/failed-ids.mjs $(JSON) 2>/dev/null); \
	  if [ -z "$$failed" ]; then break; fi; \
	  echo "[make] retry $$attempt/$(RETRIES): re-running failed behaviours [$$failed]"; \
	  rc=0; \
	  $(NODE) run.mjs --parallel $(PARALLEL) --json $(OUT)/eval.retry.json --behaviours "$$failed" $(SEL) || rc=$$?; \
	  $(NODE) scripts/merge-report.mjs $(JSON) $(OUT)/eval.retry.json $(JSON) || true; \
	  if [ $$rc -eq 0 ]; then echo "[make] retry passed — treating prior failures as flaky"; fi; \
	done; \
	$(MAKE) --no-print-directory gc; \
	echo "[make] reports → $(JSON)  $(JUNIT)  $(HTML)  $(REVIEW)"; \
	exit $$rc

# Run the matrix ROUNDS times back-to-back (stability + leak hunting). Stops on the
# first round that fails. Each round GCs, so orphan count should stay flat.
soak:
	@set -e; \
	for r in $$(seq 1 $(ROUNDS)); do \
	  echo "[make] === soak round $$r/$(ROUNDS) ==="; \
	  $(MAKE) --no-print-directory eval || { echo "[make] soak failed on round $$r"; exit 1; }; \
	done; \
	echo "[make] soak: $(ROUNDS) rounds clean"

list:
	@$(NODE) run.mjs --list

reports:
	@echo "$(JSON)"; echo "$(JUNIT)"; echo "$(HTML)"; echo "$(REVIEW)"

# Garbage-collect: kill/remove any leftover eval containers (name fleet-eval-*) and
# prune dangling images. Safe to run anytime; never touches non-eval containers.
gc:
	@echo "[make] gc: removing orphan fleet-eval-* containers"; \
	ids=$$(docker ps -aq --filter "name=fleet-eval-" 2>/dev/null); \
	if [ -n "$$ids" ]; then docker rm -f $$ids >/dev/null 2>&1 || true; \
	  echo "[make] gc: removed $$(echo $$ids | wc -w | tr -d ' ') container(s)"; \
	else echo "[make] gc: none"; fi; \
	docker image prune -f >/dev/null 2>&1 || true

clean: gc
	@rm -rf $(OUT) && echo "[make] clean: wiped $(OUT)"

install:
	@npm install
