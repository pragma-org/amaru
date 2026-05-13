---
type: process
status: accepted
---

# Code Annotations (NOTE, TODO and FIXME)

## Context

The codebase already contains a large number of `NOTE`, `TODO` and `FIXME`
comments, but their meaning and formatting are not always consistent.

This creates ambiguity for reviewers and maintainers. Some annotations refer to
small cleanup opportunities, while others identify correctness gaps, unsafe
shortcuts, or incomplete implementations that materially affect the readiness
of the software.

We need a simple convention so that these annotations remain useful over time,
can be interpreted consistently across crates, and communicate the priority of
the underlying work item without requiring additional context.

## Decision

We use `TODO` and `FIXME` as comment annotations to track work that is
required later on a particular piece of code and which cannot be completed
_immediately_ for a variety of reasons.

We use `NOTE` for drawing attention to a particular design constraint or
pre-condition.

Some annotations may be addressed as part of a same PR, but some may not. In
any case, we should systematically flag with an annotation code that requires
attention later (even if we are 100% sure that we will address it later as part
of the same PR; in case we actually don't).

### Format

In practice:

- a `NOTE` signals "this is known and important";
- a `TODO` signals "this should be improved";
- a `FIXME` signals "this must be fixed".

All annotations shall follow the same formatting convention, to be easily
extractable.

```rust
// <NOTE|TODO|FIXME>: <short one-line abstract>
//
// <optional multi-line description and context>
```

More specifically:

- the first line must start with `NOTE`, `TODO` or `FIXME` in uppercase,
  followed by a colon, followed by a short one-line summary;
- the multi-line body is optional for all annotations;
- when present, the second line must be blank;
- when present, the following lines must explain the problem, missing work,
  rationale, or expected direction for the future change;
- Never in doc comments (`///`).

Annotations should be written close to the code they refer to, and should be
specific enough that a future maintainer can understand why the annotation
exists and what kind of change is expected. When the short one-line abstract is
already self-explanatory, no further description is required.

### Use of `NOTE`

`NOTE` is used for implementation details that may be surprising. They are used
to draw the attention of the reader to a particular design decision or consequence
which is not necessarily a problem that must be addressed, but that is explicitly
acknowledged as _a reality_.

For example:

```rust
// NOTE: Cost of registering relay addresses
//
// This operation blocks the ledger for about 4ms (mainnet late 2025), so it
// should be called with care. Please cache the result, it  only changes
// meaningfully once per epoch.
```

### Use of `TODO`

`TODO` is used for follow-up work that is desirable, but not critical to safety
or correctness. This includes, for example:

- performance improvements, unless the performance issue becomes a security or safety concern;
- aesthetic or ergonomic refinements;
- cleaner abstractions;
- long-term maintainability improvements;
- better tests or documentation where the current state remains otherwise sound.

For example:

```rust
// TODO: Batch UTxO lookups in this validation path
//
// The current implementation performs one lookup per input. This is
// acceptable for now, but creates unnecessary I/O amplification and
// should be replaced with a batched store API once available.
```


### Use of `FIXME`

`FIXME` is used for critical follow-up work that must be addressed before the
software can be labelled as production ready. A `FIXME` identifies a shortcut,
workaround, incomplete behaviour, or unsound design that we do not consider an
acceptable steady state.

For example:

```rust
// FIXME: This translation silently drops invalid witness information
//
// The current logic accepts malformed data and continues with a partial
// representation. This is not sound and must be replaced by explicit
// validation before the ledger can be considered production ready.
```

## Consequences

- Developers have a shared vocabulary for distinguishing optional follow-up
  work from production-blocking debt.
- Reviewers can interpret the urgency of a code annotation without guessing the
  author's intent.
- Existing annotations can be progressively normalised as touched code is
  revisited; no dedicated repository-wide rewrite is required immediately.
- It is likely worth creating one or several follow-up issues to review the
  existing annotations and check whether they fall in the right categories and
  follow the expected format.
- `FIXME` comments become a visible signal of work that must be prioritised
  before claiming production readiness.
- Free-form or under-specified annotations become discouraged, since the
  convention expects at least a summary, and context whenever needed.

## Discussion points

- We intentionally keep the convention lightweight and local to the code,
  rather than requiring every `NOTE`, `TODO` or `FIXME` to be mirrored in a
  separate tracking system.

- This convention does not replace ordinary explanatory comments. If a comment
  does not describe future required work, it should not use `TODO` or `FIXME`.
