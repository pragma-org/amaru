---
type: architecture
status: proposed
---

# Avoid Primitive Obsession

## Motivation

In software development, ["primitive obsession"](https://wiki.c2.com/?PrimitiveObsession) refers to the overuse of primitive data types (like integers, strings, booleans) to represent domain concepts. This can lead to several issues:

1. **Type safety issues**: Using primitives for domain concepts can lead to errors where semantically different values with the same primitive type are accidentally mixed.
2. **Missing domain logic**: When domain concepts are represented as primitives, the logic associated with those concepts is often scattered throughout the codebase.
3. **Reduced code readability**: Code that heavily uses primitives for domain concepts is often harder to understand as the semantic meaning is not immediately clear.
4. **Difficulty in refactoring**: When a domain concept represented by a primitive needs to change, all usages must be found and updated.

In a complex system like Amaru, where we deal with many domain-specific concepts (slots, epochs, hashes, etc.), avoiding primitive obsession becomes crucial for maintainability and correctness.

## Decision

We will avoid primitive obsession by:

1. **Creating domain-specific types**: Instead of using primitive types directly, we will create custom types that represent domain concepts, even when they wrap a single primitive value.

2. **Using newtypes**: In Rust, we'll use the [newtype pattern](https://www.worthe-it.co.za/blog/2020-10-31-newtype-pattern-in-rust.html) to create zero-cost abstractions over primitive types. This provides type safety without runtime overhead.

3. **Implementing appropriate traits**: Our domain-specific types will implement traits that make sense for their semantics, such as:
   - `PartialEq`, `Eq`, `PartialOrd`, `Ord` for comparison
   - `Display`, `Debug` for formatting
   - `Serialize`, `Deserialize` for serialization
   - [derive-more]() could be used for non-standard traits
   - CBOR `Encode` and `Decode` need to be written manually

4. **Encapsulating validation logic**: Domain-specific types will encapsulate validation logic to ensure they always represent valid values.

5. **Providing clear conversion methods**: When conversion between types is necessary, we'll provide explicit `From` trait instances.

## Consequences

### Guidelines

- A newtype is _required_ for a type that semantically cannot support all operations offered by the primitive Rust type (which also includes permitting only a subset of the possible values).
- A newtype is _recommended_ where the primitive type has a particular and well-established meaning that exists not only in the local code scope, because this implies that uses of the primitive type will overlap with other semantics that shall remain distinct.
- A newtype will typically _not be used_ for external inputs that haven’t yet been parsed or validated (like Vec<u8> from files or network sockets).

### Example

Here is a full example of the definition of a newtype for `Slot`.

Define the newtype using a [transparent](https://doc.rust-lang.org/nomicon/other-reprs.html#reprtransparent) representation to reduce runtime overhead, and make single field private.

> ![NOTICE]
> The newtype's private field is only private for code within modules which not children of the module it is defined in.

```rust
#[derive(Clone, Debug, Copy, PartialEq, PartialOrd, Eq, Serialize, Deserialize)]
#[repr(transparent)]
pub struct Slot(u64);
```

Provide dedicated methods to  manipulate the newtype in a domain-specific way.

```rust
impl Slot {
    fn elapsed_from(&self, slot: Slot) -> u64 {
        self.0 - slot.0
    }

    fn offset_by(&self, slots_elapsed: u64) -> Slot {
        Slot(self.0 + slots_elapsed)
    }
}
```

Provide common conversions to/from primitives type:

```rust

impl From<u64> for Slot {
    fn from(slot: u64) -> Slot {
        Slot(slot)
    }
}

impl From<Slot> for u64 {
    fn from(slot: Slot) -> u64 {
        slot.0
    }
}
```

Provide `Encode`/`Decode` implementations to make it easy to use newtype with `to_cbor()` and `from_cbor()` functions.

**NOTE**: This should probably be provided by dedicated macros.

```rust
impl<C> Encode<C> for Slot {
    fn encode<W: minicbor::encode::Write>(
        &self,
        e: &mut minicbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), minicbor::encode::Error<W::Error>> {
        self.0.encode(e, ctx)
    }
}

impl<'b, C> Decode<'b, C> for Slot {
    fn decode(d: &mut Decoder<'b>, _ctx: &mut C) -> Result<Self, minicbor::decode::Error> {
        d.u64().map(Slot)
    }
}
```

### Positive Outcomes

- **Improved type safety**: The compiler will catch type errors that would otherwise manifest as runtime bugs.
- **Better domain modeling**: Domain concepts are explicitly represented in the code, making the model clearer.
- **Centralized validation**: Validation logic is centralized in the type definition, reducing duplication.
- **Enhanced readability**: Code becomes more self-documenting as types clearly indicate their purpose.
- **Easier refactoring**: Changes to domain concepts are localized to their type definitions.

### Negative Outcomes

- **Increased boilerplate**: Creating custom types requires more code than using primitives directly.
- **Conflicting definitions**: Re-definitions of concepts which are common across the Cardano universe could lead to confusion and require annoying conversions back and forth when depending on libraries which also define those concepts.
- **Increased coupling across domains**: Dependencies to libraries defining common types and concepts might increase coupling between domains.
  - When A and B share the definitions of types from C, they implicitly depend on each other: Changes in `C` required by `A` could break `B` or at least require updating dependencies. In the worst case, this could lead to diamond dependency problem.

## Discussion points

- We should establish guidelines for when a new type is warranted versus when using a primitive is acceptable.
- We should consider creating macros or helper functions to reduce the boilerplate associated with creating new types, especially for CBOR encoding/decoding which is non-standard
- Integration with existing libraries that expect primitive types, or that already use their custom `newtypes` needs careful consideration, eg. `Slot` is [defined](https://github.com/txpipe/pallas/blob/c9913a08f1fbc5880c58b72169167dd32cc1b959/pallas-network/src/miniprotocols/txmonitor/protocol.rs#L3) and [redefined](https://github.com/txpipe/pallas/blob/c9913a08f1fbc5880c58b72169167dd32cc1b959/pallas-addresses/src/lib.rs#L67) in Pallas and amaru:

```
crates/ouroboros/src/lib.rs:32:pub type Slot = u64;
crates/amaru-kernel/src/lib.rs:203:pub type Slot = u64;
crates/amaru/src/sync.rs:44:pub type Slot = u64;
crates/amaru-ledger/src/store/columns/slots.rs:21:pub type Key = Slot;
```
