#!/usr/bin/env -S jq -rf
# Copyright 2026 PRAGMA
#
# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at
#
#     http://www.apache.org/licenses/LICENSE-2.0
#
# Unless required by applicable law or agreed to in writing, software
# distributed under the License is distributed on an "AS IS" BASIS,
# WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
# See the License for the specific language governing permissions and
# limitations under the License.

# Inverse CDF of external-effect durations from an ndjson log of span-close events.
# You can obtain this by adding `--log-output timings.ndjson:none,amaru_pure_stage::effect=debug`
# when running Amaru.
#
# Each span close contributes its busy time and its idle time as separate samples.
# tracing-subscriber prints those as a number plus ns, µs, ms, or s.
#
# One raw line per effect. The first field is `type_name` with `:` replaced by `_`.
# The rest is a Rust `DurationDist::cdf` expression: probability is an f32, latency
# is nanoseconds. Knots are the inverse empirical CDF at
# 0%, 10%, 20%, 40%, 60%, 80%, 90%, 95%, 98%, 99%, and 100%.
#
# Usage (NDJSON on stdin):
#   scripts/effect-timings.jq < timings.ndjson | scripts/generate-effect-timings

def parse_duration_ns:
  capture("^(?<magnitude>[0-9]+(?:[.][0-9]+)?)(?<unit>ns|\u00b5s|ms|s)$")
  | (.magnitude | tonumber) as $magnitude
  | (
      if .unit == "ns" then $magnitude
      elif .unit == "\u00b5s" then $magnitude * 1000
      elif .unit == "ms" then $magnitude * 1000000
      else $magnitude * 1000000000
      end
    )
  | round;

# Smallest sample at which the empirical distribution reaches probability p.
def quantile($p):
  length as $n
  | if $p <= 0 then .[0]
    else .[(($p * $n) | ceil) - 1]
    end
  | [$p, .];

def cdf_deciles:
  sort as $sorted
  | [[0,10,20,40,60,80,90,95,98,99,100]|.[] as $step | $sorted | quantile($step / 100)];

def fmt_prob:
  if . == 0 or . == 1 then "\(.).0" else "\(.)" end;

def rust_cdf:
  map("(\(.[0] | fmt_prob), \(.[1]))") | join(", ")
  | "DurationDist::cdf(&[\(.)])";

# `.` is the first stdin value and `inputs` is the rest, so a shebang without `-n` still
# keeps every record.
[
  ., inputs
  | select(.target == "amaru_pure_stage::effect" and .fields.message == "close" and .fields.type_name != null)
  | [.fields.type_name, (.fields["time.busy", "time.idle"] | parse_duration_ns)]
]
| group_by(.[0])
| map({name: .[0][0], knots: (map(.[1]) + map(.[2]) | cdf_deciles)})
| sort_by(.name)[]
| (.name | gsub(":"; "_")) as $name
| "\($name) \(.knots | rust_cdf)"
