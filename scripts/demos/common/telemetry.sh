#!/usr/bin/env bash

# Manages the Grafana/Tempo/Prometheus telemetry stack for the demos.
#
# Callers must set TELEMETRY_DIR, TELEMETRY_PROFILES, TELEMETRY_GRAFANA_URL,
# TELEMETRY_PROMETHEUS_URL, and START_TELEMETRY, and define a telemetry_urls function
# printing the Grafana URLs that open_telemetry opens in the browser. The Docker Compose
# files are the base $TELEMETRY_DIR/docker-compose.yml plus, for each selected profile,
# its $TELEMETRY_DIR/profiles/<profile>/docker-compose.yml when that file exists.

telemetry_compose() {
  have docker || die "docker not found; install Docker or set START_TELEMETRY=false"
  local -a compose_args=()
  local -a profile_args=()
  local profile profile_compose_file
  [[ -f "$TELEMETRY_DIR/docker-compose.yml" ]] || die "telemetry compose file not found: $TELEMETRY_DIR/docker-compose.yml"
  compose_args=(-f "$TELEMETRY_DIR/docker-compose.yml")
  for profile in $TELEMETRY_PROFILES; do
    profile_args+=(--profile "$profile")
    profile_compose_file="$TELEMETRY_DIR/profiles/$profile/docker-compose.yml"
    if [[ -f "$profile_compose_file" ]]; then
      compose_args+=(-f "$profile_compose_file")
    fi
  done
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

grafana_mempool_dashboard_url() {
  grafana_metrics_url '[
    {
      "name": "A",
      "expr": "sum by (origin, result) (increase(amaru_metrics_mempoolTxInsertionsNum_int[5m]))",
      "legend": "mempool insertions {{origin}} {{result}}"
    },
    {
      "name": "B",
      "expr": "cardano_node_metrics_mempoolBytes_int",
      "legend": "mempool bytes"
    },
    {
      "name": "C",
      "expr": "cardano_node_metrics_blockNum_int",
      "legend": "block height"
    },
    {
      "name": "D",
      "expr": "process_memory_live_resident",
      "legend": "resident memory"
    },
    {
      "name": "E",
      "expr": "process_cpu_live",
      "legend": "cpu"
    }
  ]'
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
