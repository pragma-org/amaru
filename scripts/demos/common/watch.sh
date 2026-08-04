#!/usr/bin/env bash

# Follows and colorizes the demo logs listed in DEMO_LOG_FILES.
#
# Source labels are derived from the log file basenames (amaru-middle.log -> amaru-middle)
# and label colors are assigned from a fixed palette in DEMO_LOG_FILES order. Sources named
# amaru-* are treated as Amaru nodes for transaction highlighting; the submit-tx,
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

      # Reads a field out of a line in the tracing text format, where fields come as
      # `name="value"` and the node may have painted the name and the `=` with ANSI codes of its
      # own. The codes are dropped from a copy of the line so one pattern reads both forms, and
      # the name must start a word so `id` does not match inside `tx_id`.
      function text_field_value(text, field,    plain, esc, value) {
        plain = text
        esc = sprintf("%c", 27)
        gsub(esc "\\[[0-9;]*m", "", plain)
        if (match(plain, "(^|[^a-z_])" field "=\"[^\"]*\"") == 0) return ""
        value = substr(plain, RSTART, RLENGTH)
        sub(/^[^"]*"/, "", value)
        sub(/"$/, "", value)
        return value
      }

      function is_amaru_node(source) {
        return source ~ /^amaru-/
      }

      # Paint the tracing level of a log line, the way the nodes do when they write to a
      # terminal. Lines that already carry ANSI codes (a node asked for colors itself) are
      # left untouched.
      function paint_log_level(text,    level, styled) {
        if (color != "true" || index(text, "\033") > 0) return text
        if (match(text, / (TRACE|DEBUG|INFO|WARN|ERROR) /) == 0) return text
        level = substr(text, RSTART + 1, RLENGTH - 2)
        styled = paint(level_colors[level], level)
        return substr(text, 1, RSTART) styled substr(text, RSTART + RLENGTH - 1)
      }

      BEGIN {
        source = "log"
        label_width = length(source)
        label_count = split(labels, label_list, " ")
        level_colors["TRACE"] = "90"
        level_colors["DEBUG"] = "34"
        level_colors["INFO"] = "32"
        level_colors["WARN"] = "1;33"
        level_colors["ERROR"] = "1;31"
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

      # `tail -F` separates the output of two files with a blank line before the header it
      # prints for the second one; nothing is lost by dropping empty lines altogether.
      /^[[:space:]]*$/ { next }

      {
        line = $0
        lower = tolower(line)
        style = ""
        marker_style = ""
        marker = ""

        if (source == "submit-tx" && lower ~ /built transaction .*tx_id=/) {
          remember_submitted_tx(line)
        }
        # The submit API answers HTTP 202 with the bare quoted transaction id as its body.
        if (source == "submit-tx" && line ~ /^"[0-9a-f]+"$/) {
          tx_id = line
          gsub(/"/, "", tx_id)
          submitted_tx_ids[substr(tx_id, 1, 12)] = 1
        }

        if (is_amaru_node(source) && lower ~ /transaction\.evicted/ && lower ~ /included_in_adopted_block/ && (substr(text_field_value(line, "id"), 1, 12) in submitted_tx_ids)) {
          marker_style = "1;36"
          marker = ">>> TX IN BLOCK >>> "
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
        } else if (source == "submit-tx" && (lower ~ /submitting|building transaction|built transaction|response: http 202|selected utxo/ || line ~ /^"[0-9a-f]+"$/)) {
          marker_style = "1;36"
          marker = ">>> TX >>> "
        } else if (source == "prepare-wallet" && lower ~ /building preparation transaction|submitting preparation transaction/) {
          marker_style = "1;36"
          marker = ">>> TX >>> "
        } else if (is_amaru_node(source) && lower ~ /transaction\.accepted|transaction accepted into mempool/ && (substr(text_field_value(line, "id"), 1, 12) in submitted_tx_ids)) {
          marker_style = "1;36"
          marker = ">>> TX >>> "
        } else if (source == "cardano-upstream" && line ~ /TraceMempoolAddedTx/) {
          marker_style = "1;36"
          marker = ">>> TX >>> "
        }

        label_style = source in label_colors ? label_colors[source] : "37"
        # The brackets hug the source name and the padding follows them, so the messages still
        # line up. The padding stays outside the ANSI codes, which do not count as width.
        label_text = "[" source "]"
        label_pad = label_width + 2 - length(label_text)
        label = paint(label_style, label_text)
        if (label_pad > 0) {
          label = label sprintf("%" label_pad "s", "")
        }
        message = marker == "" ? line : paint(marker_style, marker) line
        message = style == "" ? paint_log_level(message) : paint(style, message)
        print label " " message
        fflush()
      }'
}

run_watch() {
  tail -n +1 -F "${DEMO_LOG_FILES[@]}" 2>/dev/null | colorize_watch_logs || true
}
