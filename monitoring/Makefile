COMPOSE ?= docker compose
MONITORING_DIR := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))
AMARU_DIR := $(abspath $(MONITORING_DIR)..)
STACK := $(COMPOSE) -f $(MONITORING_DIR)docker-compose.yml

.DEFAULT_GOAL := help

.PHONY: help up run down logs ps config pull

help:
	@echo "\033[1;4mUnified monitoring stack:\033[00m"
	@grep -E '^[a-z][a-z-]*:.*## ' $(MONITORING_DIR)Makefile | while read -r line; do printf "  \033[1;32m%s\033[00m:%s\n" "$$(echo $$line | cut -f 1 -d':')" "$$(echo $$line | cut -f 3- -d'#')"; done

up: ## Start Grafana, Tempo, Prometheus, Loki, and the OTLP collector
	$(STACK) up -d

run-amaru: ## Run Amaru with OpenTelemetry enabled
	cd $(AMARU_DIR) && \
	OTEL_SERVICE_NAME=amaru \
	OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4317 \
	OTEL_METRIC_EXPORT_INTERVAL=1000 \
	amaru --with-open-telemetry node run

down: ## Stop and remove the monitoring stack
	$(STACK) down --remove-orphans

logs: ## Follow monitoring stack logs
	$(STACK) logs --follow

ps: ## List monitoring stack services
	$(STACK) ps

config: ## Validate the monitoring stack configuration
	$(STACK) config --quiet

pull: ## Pull images used by the monitoring stack
	$(STACK) pull
