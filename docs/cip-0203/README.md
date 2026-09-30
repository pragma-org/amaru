# Amaru block producer software tag

This document is the payload specification for Amaru under [CIP-0203, Block Producer Identifier Registry](https://github.com/cardano-foundation/CIPs/pull/1276). It defines what Amaru writes into the minor component of the block header `protocol_version` when it forges a block, and how to read it back.

The identifier registered for Amaru is `65` under scheme `0`.

## Payload layout

```
 21 20 | 19                              6 | 5            0
+------+-----------------------------------+---------------+
| ver  |       days since 2026-01-01       |     flags     |
|  2   |              14 bits              |     6 bits    |
+------+-----------------------------------+---------------+
```

| Field   | Bits    | Width | Range      | Meaning |
|---------|---------|-------|------------|---------|
| `ver`   | 21 - 20 | 2     | 0 - 3      | Version of this payload layout. `0` denotes the layout in this document. |
| `days`  | 19 - 6  | 14    | 0 - 16383  | Release day as days elapsed since 2026-01-01 (UTC), or `0` for an unreleased build. The last representable day is 2070-11-09. |
| `flags` | 5 - 0   | 6     | 0 - 63     | Feature flags. None are assigned. |

### Construction and extraction

```
payload = (ver << 20) | (days << 6) | flags
minor   = (payload << 8) | 65

ver     =  payload >> 20
days    = (payload >> 6) & 0x3FFF
flags   =  payload       & 0x3F
```

### `ver`

Amaru sets `ver` to `0`. Values `1`, `2` and `3` are reserved for later revisions of this
document. A later revision may keep the layout of bits 19 - 0 or redefine them entirely, and
must not change the meaning of `ver = 0`. A consumer must not interpret bits 19 - 0 when `ver`
is a value it does not implement.

### `days`

Every published Amaru release is versioned `MAJOR.MINOR.YYYYMMDD`, where the last component is
the UTC day the release was cut. A release build sets `days` to that day, so a consumer
recovers the release day without any lookup.

Any other build has `0` as the last component of its version and sets `days` to `0`. This
covers builds from source, builds of unmerged branches, and forks. No release was cut on
2026-01-01, so `0` never names a release.

### `flags`

Each flag reports a capability or mode of the running node that operators or observers may want
to count across the chain. Flags are assigned in this document, one bit at a time, starting from
bit 0. An assignment is never reused or moved. Bits that are not assigned must be `0`, and a
consumer must ignore them. When all six are assigned, the next flag requires a new `ver`.

| Bit | Name | Meaning |
|-----|------|---------|
| 0 - 5 | unassigned | Must be `0`. |

## Decoding

`amaru dev software-tag decode <VERSION>` performs these steps. `VERSION` is the header's
protocol version (`11.4374593`), the minor alone (`4374593`), or the minor in hex
(`0x0042c041`).

1. Read the minor version as an unsigned integer. Reject values above `4294967295`.
2. Take `scheme = minor >> 30`. Stop if it is not `0`.
3. Take `id = minor & 0xFF`. If it is `255` the operator opted out; if it is `0` there is no
   signal. Stop unless it is `65`.
4. Take `payload = (minor >> 8) & 0x3FFFFF` and `ver = payload >> 20`. Stop if `ver` is not `0`.
5. Extract `days` and `flags` as shown above. If `days` is `0` the build is unreleased.
   Otherwise convert it to a calendar day by adding it to 2026-01-01 and look the day up in the
   prefix table to form the release version.

## Test vectors

| `minor` (decimal) | `minor` (hex) | `ver` | `days` | `flags` | Decoded |
|-------------------|---------------|-------|--------|---------|---------|
| 65                | `0x00000041`  | 0     | 0      | 0       | Unreleased build. |
| 2637889           | `0x00284041`  | 0     | 161    | 0       | Release `10.10.20260611`. |
| 4374593           | `0x0042C041`  | 0     | 267    | 0       | Release `10.11.20260925`. |
| 4374849           | `0x0042C141`  | 0     | 267    | 1       | Release `10.11.20260925` with flag bit 0 set. Invalid while bit 0 is unassigned. |
| 268419137         | `0x0FFFC041`  | 0     | 16383  | 0       | Release day 2070-11-09, the largest representable day. |
| 255               | `0x000000FF`  |       |        |         | Operator opted out. Not attributable to Amaru. |
| 1073741889        | `0x40000041`  |       |        |         | `scheme = 1`. A consumer of this document must not decode the remaining bits. |
