#!/usr/bin/env bash

# Builds the Grafana URLs the demos point at.
#
# The monitoring stack itself is not managed here: it is the shared one under monitoring/,
# started and stopped independently of any demo (`docker compose up -d` in that directory).
# The nodes only export to it, over the OTEL_EXPORTER_OTLP_* endpoints.
#
# Callers must set TELEMETRY_GRAFANA_URL and define a telemetry_urls function printing the
# Grafana URLs that open_telemetry opens in the browser.

# Whether the OTLP collector named in OTEL_EXPORTER_OTLP_ENDPOINT is there. curl exits 6 when
# the host does not resolve, 7 when nothing listens and 28 when the connection hangs; anything
# else, including the empty reply a gRPC port sends back to an HTTP/1.1 request, means the
# collector answered.
open_telemetry_endpoint_reachable() {
  local endpoint="${1:-$OTEL_EXPORTER_OTLP_ENDPOINT}" status=0
  curl -sS -o /dev/null --max-time 2 "$endpoint" >/dev/null 2>&1 || status=$?
  case "$status" in
    6 | 7 | 28) return 1 ;;
    *) return 0 ;;
  esac
}

# Turns auto|true|false into the true/false the nodes' --with-open-telemetry flag expects.
resolve_open_telemetry() {
  case "${1:-auto}" in
    auto)
      if open_telemetry_endpoint_reachable; then echo true; else echo false; fi
      ;;
    *)
      if truthy "$1"; then echo true; else echo false; fi
      ;;
  esac
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

grafana_logs_url() {
  local pane_id="$1" query="$2" panes
  panes="$(
    jq -cn \
      --arg pane_id "$pane_id" \
      --arg query "$query" \
      '{
        ($pane_id): {
          datasource: "loki",
          queries: [
            {
              refId: "A",
              datasource: {type: "loki", uid: "loki"},
              editorMode: "code",
              expr: $query,
              queryType: "range"
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
