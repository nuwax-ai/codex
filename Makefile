# Gitea CI entry points. The GitHub workflows are untouched; everything here
# talks to the self-hosted instance instead.
#
#   make help                     # list targets
#   make gitea-check              # instance health, runners, latest runs
#   make gitea-smoke              # push HEAD to the mirror, wait for smoke
#   make gitea-release TAG=v0.18.30
#   make gitea-runs N=10
#   make gitea-logs RUN=12
#
# Configuration comes from the environment or scripts/gitea-ci/.env
# (see scripts/gitea-ci/.env.example).

CI := scripts/gitea-ci/ci.sh
N ?= 5
TAG ?=
RUN ?=

.PHONY: help gitea-check gitea-runs gitea-smoke gitea-release gitea-logs \
        gitea-artifacts gitea-inventory gitea-up gitea-down

help:
	@printf 'Gitea CI targets:\n'
	@printf '  make gitea-check                 instance health + runners + latest runs\n'
	@printf '  make gitea-runs [N=%s]           recent runs\n' '$(N)'
	@printf '  make gitea-smoke                 push HEAD to the Gitea mirror, wait for smoke\n'
	@printf '  make gitea-release TAG=vX.Y.Z    push a tag to trigger the release pipeline\n'
	@printf '  make gitea-logs RUN=<number>     tail one run'"'"'s job log\n'
	@printf '  make gitea-artifacts             list built archives on the host\n'
	@printf '  make gitea-inventory             read-only host inventory over ssh\n'
	@printf '  make gitea-up | gitea-down       start/stop the stack on the host\n'

gitea-check:
	@$(CI) check

gitea-runs:
	@$(CI) runs $(N)

gitea-smoke:
	@$(CI) smoke

gitea-release:
	@$(CI) release $(TAG)

gitea-logs:
	@$(CI) logs $(RUN)

gitea-artifacts:
	@$(CI) artifacts

gitea-inventory:
	@$(CI) inventory

gitea-up:
	@$(CI) up

gitea-down:
	@$(CI) down
