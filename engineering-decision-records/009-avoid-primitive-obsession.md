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
   - CBOR `Encode` and `Encode` need to be written manually

4. **Encapsulating validation logic**: Domain-specific types will encapsulate validation logic to ensure they always represent valid values.

5. **Providing clear conversion methods**: When conversion between types is necessary, we'll provide explicit methods rather than relying on implicit conversions.

## Consequences

### Positive Outcomes

- **Improved type safety**: The compiler will catch type errors that would otherwise manifest as runtime bugs.
- **Better domain modeling**: Domain concepts are explicitly represented in the code, making the model clearer.
- **Centralized validation**: Validation logic is centralized in the type definition, reducing duplication.
- **Enhanced readability**: Code becomes more self-documenting as types clearly indicate their purpose.
- **Easier refactoring**: Changes to domain concepts are localized to their type definitions.

### Negative Outcomes

- **Increased boilerplate**: Creating custom types requires more code than using primitives directly.
- **Learning curve**: New contributors need to learn the domain-specific types rather than working with familiar primitives.
- **Potential performance overhead**: In some cases, wrapping primitives might introduce minor performance overhead (though this is usually negligible with Rust's zero-cost abstractions). Wherever possible this should be avoided by using the `#[repr(transparent)]` annotation to ensure compiled representation does not carry with it

## Discussion points

- We should establish guidelines for when a new type is warranted versus when using a primitive is acceptable.
- We need to balance the benefits of domain-specific types with the additional complexity they introduce.
- We should consider creating macros or helper functions to reduce the boilerplate associated with creating new types.
- Integration with existing libraries that expect primitive types, or that already use their custom `newtypes` needs careful consideration.
