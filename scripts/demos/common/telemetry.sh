#!/usr/bin/env bash

# Manages the Grafana/Tempo/Prometheus telemetry stack for the demos.
#
# Callers must set TELEMETRY_DIR, TELEMETRY_PROFILES, TELEMETRY_GRAFANA_URL,
# TELEMETRY_PROMETHEUS_URL, and START_TELEMETRY, and define a telemetry_urls function
# printing the Grafana URLs that open_telemetry opens in the browser. The Docker Compose
# files are the base $TELEMETRY_DIR/docker-compose.yml plus, for each selected profile,
# its $TELEMETRY_DIR/profiles/<profile>/docker-compose.yml when that file exists.
#
# A demo can layer its own services or mounts (e.g. Grafana dashboards) on top of the shared
# stack by setting TELEMETRY_COMPOSE_OVERRIDE_FILE to an extra compose file; that file can
# anchor host paths on the exported TELEMETRY_COMPOSE_OVERRIDE_DIR.

telemetry_compose() {
  have docker || die "docker not found; install Docker or set START_TELEMETRY=false"
  local -a compose_args=()
  local -a profile_args=()
  local -a collector_configs=("--config=/etc/otlp-collector.yml")
  local profile profile_compose_file
  [[ -f "$TELEMETRY_DIR/docker-compose.yml" ]] || die "telemetry compose file not found: $TELEMETRY_DIR/docker-compose.yml"
  compose_args=(-f "$TELEMETRY_DIR/docker-compose.yml")
  for profile in $TELEMETRY_PROFILES; do
    profile_args+=(--profile "$profile")
    profile_compose_file="$TELEMETRY_DIR/profiles/$profile/docker-compose.yml"
    if [[ -f "$profile_compose_file" ]]; then
      compose_args+=(-f "$profile_compose_file")
    fi
    if [[ -f "$TELEMETRY_DIR/profiles/$profile/otlp-collector.yml" ]]; then
      collector_configs+=("--config=/etc/otlp-collector-$profile.yml")
    fi
  done
  # Exported for override compose files (e.g. a demo layer) whose command must restate the
  # collector configs contributed by the active profiles, since Docker Compose replaces the
  # command list rather than appending to it.
  export TELEMETRY_COLLECTOR_CONFIGS="${collector_configs[*]}"
  if [[ -n "${TELEMETRY_COMPOSE_OVERRIDE_FILE:-}" ]]; then
    [[ -f "$TELEMETRY_COMPOSE_OVERRIDE_FILE" ]] || die "telemetry compose override not found: $TELEMETRY_COMPOSE_OVERRIDE_FILE"
    export TELEMETRY_COMPOSE_OVERRIDE_DIR="$(cd "$(dirname "$TELEMETRY_COMPOSE_OVERRIDE_FILE")" && pwd)"
    compose_args+=(-f "$TELEMETRY_COMPOSE_OVERRIDE_FILE")
  fi
  docker compose "${compose_args[@]}" "${profile_args[@]}" "$@"
}

telemetry_up() {
  if ! truthy "$START_TELEMETRY"; then
    echo "[telemetry] skipped because START_TELEMETRY=false"
    return 0
  fi

  echo "[telemetry] removing old Tempo spans..."
  telemetry_reset
  echo "[telemetry] starting Grafana, Tempo, Prometheus, and the OTLP collector..."
  telemetry_compose up -d
  echo "[telemetry] Grafana: $TELEMETRY_GRAFANA_URL"
  echo "[telemetry] Prometheus: $TELEMETRY_PROMETHEUS_URL"
}

telemetry_down() {
  telemetry_compose down
}

telemetry_reset() {
  telemetry_compose down --volumes --remove-orphans
}

urlencode() {
  if have jq; then
    jq -nr --arg value "$1" '$value|@uri'
  else
    die "jq not found; cannot encode telemetry URLs"
  fi
}

# Prints a Grafana Explore URL for a Tempo TraceQL query.
grafana_trace_url() {
  local pane_id="$1" query="$2" panes
  panes="$(
    jq -cn \
      --arg pane_id "$pane_id" \
      --arg query "$query" \
      '{
        ($pane_id): {
          datasource: "tempo",
          queries: [
            {
              refId: "A",
              datasource: {type: "tempo", uid: "tempo"},
              queryType: "traceql",
              query: $query
            }
          ],
          range: {from: "now-15m", to: "now"}
        }
      }'
  )"
  printf '%s/explore?orgId=1&schemaVersion=1&refresh=5s&panes=%s\n' "$TELEMETRY_GRAFANA_URL" "$(urlencode "$panes")"
}

# Prints a Grafana Explore URL for a JSON array of Prometheus queries ({name, expr, legend}).
grafana_metrics_url() {
  local queries_json="$1"
  local panes
  panes="$(
    jq -cn \
      --argjson queries_json "$queries_json" \
      '{
        metrics: {
          datasource: "prometheus",
          queries: [
            $queries_json[]
            | {
              refId: .name,
              datasource: {type: "prometheus", uid: "prometheus"},
              expr: .expr,
              legendFormat: (.legend // .name),
              range: true,
              instant: false
            }
          ],
          range: {from: "now-15m", to: "now"}
        }
      }'
  )"
  printf '%s/explore?orgId=1&schemaVersion=1&refresh=5s&panes=%s\n' "$TELEMETRY_GRAFANA_URL" "$(urlencode "$panes")"
}

# Prints the URL of a provisioned Grafana dashboard identified by its uid.
grafana_dashboard_url() {
  printf '%s/d/%s?orgId=1&refresh=5s\n' "$TELEMETRY_GRAFANA_URL" "$1"
}

grafana_dashboard_panel_url() {
  printf '%s/d/%s?orgId=1&refresh=5s&viewPanel=%s\n' "$TELEMETRY_GRAFANA_URL" "$1" "$2"
}

open_telemetry() {
  local url
  if have open; then
    telemetry_urls | while IFS= read -r url; do
      open "$url"
    done
  elif have xdg-open; then
    telemetry_urls | while IFS= read -r url; do
      xdg-open "$url"
    done
  else
    telemetry_urls
  fi
}
