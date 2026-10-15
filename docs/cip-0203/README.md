# Amaru block producer software tag

This document is the payload specification for Amaru under [CIP-0203, Block Producer Identifier Registry](https://github.com/cardano-foundation/CIPs/pull/1276). It defines what Amaru writes into the minor component of the block header `protocol_version` when it forges a block, and how to read it back.

The identifier registered for Amaru is `65` under scheme `0`.

An operator who does not want to disclose their software passes `--software-tag-off` to
`amaru node run`, or sets `AMARU_SOFTWARE_TAG_OFF=true`. Amaru then writes the CIP-0203 opt-out
value: a minor of `255`, with no payload.

## Payload layout

```
 21 20 | 19       13 | 12      9 | 8       4 | 3     0
+------+-------------+-----------+-----------+---------+
| ver  |     YY      |    MM     |    DD     |  flags  |
|  2   |   7 bits    |  4 bits   |  5 bits   | 4 bits  |
+------+-------------+-----------+-----------+---------+
```

| Field   | Bits    | Width | Range   | Meaning |
|---------|---------|-------|---------|---------|
| `ver`   | 21 - 20 | 2     | 0 - 3   | Version of this payload layout. `0` denotes the layout in this document. |
| `YY`    | 19 - 13 | 7     | 0 - 127 | Last two digits of release year (UTC). |
| `MM`    | 12 - 9  | 4     | 1 - 12  | Release month (UTC). |
| `DD`    | 8 - 4   | 5     | 1 - 31  | Release day of the month (UTC). |
| `flags` | 3 - 0   | 4     | 0 - 15  | Feature flags. None are assigned. |

`YY`, `MM` and `DD` are all `0` for an unreleased build. The last representable day is
2127-12-31.

### Construction and extraction

```
date    = (YY << 9) | (MM << 5) | DD
payload = (ver << 20) | (date << 4) | flags
minor   = (payload << 8) | 65

ver     =  payload >> 20
YY      = (payload >> 13) & 0x7F
MM      = (payload >> 9)  & 0xF
DD      = (payload >> 4)  & 0x1F
flags   =  payload        & 0xF
```

### `ver`

Amaru sets `ver` to `0`. Values `1`, `2` and `3` are reserved for later revisions of this
document. A later revision may keep the layout of bits 19 - 0 or redefine them entirely, and
must not change the meaning of `ver = 0`. A consumer must not interpret bits 19 - 0 when `ver`
is a value it does not implement.

### `YY`, `MM` and `DD`

Every published Amaru release is versioned `MAJOR.MINOR.YYYYMMDD`, where the last component is
the UTC day the release was cut. A release build writes that day into `YY`, `MM` and `DD`, so a
consumer recovers the release day without any lookup.

Any other build has `0` as the last component of its version and sets all three fields to `0`.
This covers builds from source, builds of unmerged branches, and forks. `MM = 0` is not a month,
so the all-zero value never names a release. Any other combination that is not a calendar day is
invalid.

### `flags`

Each flag reports a capability or mode of the running node that operators or observers may want
to count across the chain. Flags are assigned in this document, one bit at a time, starting from
bit 0. An assignment is never reused or moved. Bits that are not assigned must be `0`, and a
consumer must ignore them. When all four are assigned, the next flag requires a new `ver`.

| Bit | Name | Meaning |
|-----|------|---------|
| 0 - 3 | unassigned | Must be `0`. |

## Decoding

`amaru dev software-tag decode <VERSION>` performs these steps. `VERSION` is the header's
protocol version (`11.55808065`), the minor alone (`55808065`), or the minor in hex
(`0x03539041`).

1. Read the minor version as an unsigned integer. Reject values above `4294967295`.
2. Take `scheme = minor >> 30`. Stop if it is not `0`.
3. Take `id = minor & 0xFF`. If it is `255` the operator opted out; if it is `0` there is no
   signal. Stop unless it is `65`.
4. Take `payload = (minor >> 8) & 0x3FFFFF` and `ver = payload >> 20`. Stop if `ver` is not `0`.
5. Extract `YY`, `MM`, `DD` and `flags` as shown above. Stop if any unassigned flag is set.
6. If `YY`, `MM` and `DD` are all `0` the build is unreleased. Otherwise the release day is
  `YY`-`MM`-`DD`; stop if that is not a calendar day.

## Test vectors

| `minor` (decimal) | `minor` (hex) | `ver` | `YY` | `MM` | `DD` | `flags` | Decoded |
|-------------------|---------------|-------|------|------|------|---------|---------|
| 65                | `0x00000041`  | 0     | 0    | 0    | 0    | 0       | Unreleased build. |
| 55357505          | `0x034CB041`  | 0     | 26   | 6    | 11   | 0       | Release day 2026-06-11. |
| 55808065          | `0x03539041`  | 0     | 26   | 9    | 25   | 0       | Release day 2026-09-25. |
| 55808321          | `0x03539141`  | 0     | 26   | 9    | 25   | 1       | Release day 2026-09-25 with flag bit 0 set. Invalid while bit 0 is unassigned. |
| 54911041          | `0x0345E041`  | 0     | 26   | 2    | 30   | 0       | 2026-02-30 is not a calendar day. Invalid. |
| 268038209         | `0x0FF9F041`  | 0     | 127  | 12   | 31   | 0       | Release day 2127-12-31, the largest representable day. |
| 255               | `0x000000FF`  |       |      |      |      |         | Operator opted out. Not attributable to Amaru. |
| 1073741889        | `0x40000041`  |       |      |      |      |         | `scheme = 1`. A consumer of this document must not decode the remaining bits. |
