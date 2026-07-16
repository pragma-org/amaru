#!/usr/bin/env bash

# Follows and colorizes the demo logs listed in DEMO_LOG_FILES.
#
# Source labels are derived from the log file basenames (amaru-middle.log -> amaru-middle)
# and label colors are assigned from a fixed palette in DEMO_LOG_FILES order. Sources named
# amaru-* are treated as Amaru nodes for transaction and block highlighting; the submit-tx,
# prepare-wallet, and cardano-upstream labels come from the log file names used by the
# shared tx and cardano-node helpers. Set WATCH_COLOR=never to disable ANSI colors.

colorize_watch_logs() {
  local color="${WATCH_COLOR:-always}"
  if [[ "$color" == "never" || "$color" == "false" ]]; then
    color=false
  else
    color=true
  fi

  local labels="" file
  for file in "${DEMO_LOG_FILES[@]}"; do
    file="${file##*/}"
    labels+="${labels:+ }${file%.log}"
  done

  awk \
    -v color="$color" \
    -v labels="$labels" '
      function paint(code, text) {
        return color == "true" ? sprintf("%c[%sm%s%c[0m", 27, code, text, 27) : text
      }

      function source_for(path,    name) {
        name = path
        sub(/^.*\//, "", name)
        sub(/\.log$/, "", name)
        return name
      }

      function field_value(text, field,    prefix, start, rest, stop) {
        prefix = "\"" field "\":\""
        start = index(text, prefix)
        if (start == 0) return ""
        rest = substr(text, start + length(prefix))
        stop = index(rest, "\"")
        return stop == 0 ? rest : substr(rest, 1, stop - 1)
      }

      function remember_submitted_tx(text,    tx_id) {
        tx_id = text
        sub(/^.*tx_id=/, "", tx_id)
        sub(/[^0-9a-fA-F].*$/, "", tx_id)
        if (tx_id != "") submitted_tx_ids[substr(tx_id, 1, 12)] = 1
      }

      function is_amaru_node(source) {
        return source ~ /^amaru-/
      }

      BEGIN {
        source = "log"
        label_width = length(source)
        label_count = split(labels, label_list, " ")
        palette_size = split("33 36 34 35 32 31", palette, " ")
        for (i = 1; i <= label_count; i++) {
          label_colors[label_list[i]] = palette[((i - 1) % palette_size) + 1]
          if (length(label_list[i]) > label_width) label_width = length(label_list[i])
        }
      }

      /^==> .* <==$/ {
        current = $0
        sub(/^==> /, "", current)
        sub(/ <==$/, "", current)
        source = source_for(current)
        next
      }

      {
        line = $0
        lower = tolower(line)
        style = ""
        marker_style = ""
        marker = ""

        if (source == "submit-tx" && lower ~ /built transaction .*tx_id=/) {
          remember_submitted_tx(line)
        }

        if (is_amaru_node(source) && lower ~ /transaction accepted into mempool/) {
          pending_txs[source]++
        } else if (source == "cardano-upstream" && line ~ /TraceMempoolAddedTx/) {
          pending_txs[source]++
        }

        if (is_amaru_node(source) && pending_txs[source] > 0 && lower ~ /adopted tip/) {
          marker_style = "1;32"
          marker = sprintf(">>> BLOCK AFTER %d TX >>> ", pending_txs[source])
          pending_txs[source] = 0
        } else if (source == "cardano-upstream" && pending_txs[source] > 0 && line ~ /Chain extended, new tip:/) {
          marker_style = "1;32"
          marker = sprintf(">>> BLOCK AFTER %d TX >>> ", pending_txs[source])
          pending_txs[source] = 0
        } else if (is_amaru_node(source) && lower ~ /transaction found in block/ && (substr(field_value(line, "tx_id"), 1, 12) in submitted_tx_ids)) {
          marker_style = "1;36"
          marker = ">>> TX >>> "
        } else if (is_amaru_node(source) && lower ~ /transaction invalid in block/ && (substr(field_value(line, "tx_id"), 1, 12) in submitted_tx_ids)) {
          marker_style = "1;36"
          marker = ">>> TX >>> "
        } else if (lower ~ /error|rejected|giving up|failed|non-retryable/) {
          style = "1;31"
        } else if (lower ~ /warn/) {
          style = "1;33"
        } else if (source == "submit-tx" && lower ~ /submitting|building transaction|built transaction|response: http 202|selected utxo/) {
          marker_style = "1;36"
          marker = ">>> TX >>> "
        } else if (source == "prepare-wallet" && lower ~ /building preparation transaction|submitting preparation transaction/) {
          marker_style = "1;36"
          marker = ">>> TX >>> "
        } else if (is_amaru_node(source) && lower ~ /transaction accepted into mempool/) {
          marker_style = "1;36"
          marker = ">>> TX >>> "
        } else if (source == "cardano-upstream" && line ~ /TraceMempoolAddedTx/) {
          marker_style = "1;36"
          marker = ">>> TX >>> "
        }

        label_style = source in label_colors ? label_colors[source] : "37"
        label = sprintf("[%-" label_width "s]", source)
        label = paint(label_style, label)
        message = marker == "" ? line : paint(marker_style, marker) line
        print label " " (style == "" ? message : paint(style, message))
        fflush()
      }'
}

run_watch() {
  tail -n +1 -F "${DEMO_LOG_FILES[@]}" 2>/dev/null | colorize_watch_logs || true
}
