---
type: architecture
status: proposed
---

# Bump arenas and their workarounds

## Motivation

We sometimes have to deal with a directed acyclic graph of data that is created and processed in a somewhat arbitrary fashion. This is an easy pattern to model in many garbage-collected languages including Haskell, and sometimes we just have to be compatible with the architectural decisions made in that paradigm. Unfortunately, this usage pattern sometimes makes Rust unhappy.

## Decision

We have several tools to deal with the situation. The most straightforward is to use `Rc`, this is obviously the right place to start prototyping but can become a performance bottleneck. Another alternative is to store data in a database and replace references to other nodes by the unique key of that row in the database. This can be appropriate when the data is long-lived and accessed on demand. It is a terrible choice for short-lived, small, quickly accessed items. You can store the data in a `Vec` and replace pointers with indexes into the data structure. This can work well as long as all of your data is of the same type, and there's fairly clear patterns to where new entries are added. If you need to run generic graph algorithms on data stored in this format  `petgraph` is a good choice. Similarly if you need to make sure that he is our only used in the correct data structure `Slab` or `Slotmap` may work well. For `uplc` the user provided programs can create large amounts of data of heterogeneous type that only get accessed by the rest of their program. The best solution for this context was a `bumpalo::Bump`. By use of carefully written unsafe code it allows accessing entries using normal references, and storing new items with only a shared reference, and very fast deallocation.

## Consequences

Unfortunately, the very fast deallocation is achieved by ignoring any drop code required by the underlying type. Most types they get stored in a user provided program as processed by `uplc` either don't own data or only own data that itself is stored in the `Bump` arena. So generally this limitation turns out not to be a big deal. The exception is integers, which need to store their data on the heap because their specification does not mandate a largest potential value. The integer type stores its data in a separate allocation in the heap, relying on the drop implementation free that backing data. We put that integer in a `bumpalo::Bump`. Which does not run drop on its contents. The result is a pretty dramatic memory leak.

In [uplc-pr-36](https://github.com/pragma-org/uplc/pull/36) adds a New Type for `bumpalo::Bump` called `Arena`. This provides a place to handle more complicated logic. The PR also uses `AppendOnlyVec`, a alterintive to `Slab` or `Slotmap`. It is similar to `Bump` in that you can push/alloc with only a shared reference, and in that it never moves items once they've been allocated so it can have long lived shared references to items. It differs in that it can only store one type. It has extra overhead in that it can safely be used across multiple threads, which is functionality we do not need.

An alternative solution to this problem would be to find a large integer crate that supports storing its data in `Bump` allocation. At this time we did not find a library that supports arbitrary sized integers and custom allocations. Our preferred large integer crate is `nub-bigint` is probably only interested in adding compatibility with custom allocators once std stabilizes the API.

## Discussion points

<!-- Summarizes, a posteriori, the major discussion points around that decisions -->