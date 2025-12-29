# Tx Submission Miniprotocol Conversion Plan

## Overview

This document outlines the plan for converting the `tx_submission` protocol from the current manual stage-based approach to the miniprotocol pattern, as implemented in `keepalive`.

## Current Architecture

### Current Structure

- **Manual stage functions**: `initiator_stage()` and `responder_stage()` in `stage.rs`
- **Custom message handling**: `TxSubmissionMessage` enum (Registered, FromNetwork)
- **Custom outcome type**: `Outcome` enum (Done, Error, Send)
- **State separation**:
  - `TxSubmissionState` - protocol state machine (Init, Idle, Done, Txs, TxIdsBlocking, TxIdsNonBlocking)
  - `TxSubmissionInitiatorState` - initiator-specific state (window, last_seq)
  - `TxSubmissionResponderState` - responder-specific state (window, pending_fetch, inflight_fetch, etc.)
- **Manual muxer registration**: Direct handling of `MuxMessage` and `HandlerMessage`
- **Mempool access**: Via `MemoryPool::new(eff.clone())` wrapper

### Current Flow

1. `register_tx_submission()` creates stages and wires them up
2. Messages come in as `TxSubmissionMessage` (Registered or FromNetwork)
3. Stage functions decode messages and call role-specific `step()` methods
4. `step()` returns `(TxSubmissionState, Outcome)`
5. `process_outcome()` handles sending messages and error handling

## Target Architecture (Miniprotocol Pattern)

### Target Structure (from keepalive)

- **Protocol state**: Implements `ProtocolState<R>` trait
- **Stage state**: Implements `StageState<Proto, R>` trait  
- **Miniprotocol function**: Uses `miniprotocol()` to create handler
- **Standard outcome**: `Outcome<S, D>` from miniprotocol module
- **Standard inputs**: `Inputs<L>` enum (Local, Network)
- **Automatic message handling**: CBOR encoding/decoding handled by miniprotocol
- **Protocol spec**: `spec()` function for protocol validation

### Key Components

1. **ProtocolState trait** - Pure protocol state machine
   - `init()` - Initial state behavior
   - `network()` - Handle incoming network messages
   - `local()` - Handle local actions
   - Types: `WireMsg`, `Action`, `Out`

2. **StageState trait** - Decision-making logic
   - `local()` - Handle local inputs, return actions
   - `network()` - Handle protocol outputs, return actions
   - Type: `LocalIn`

3. **Actions** - Commands from stage state to protocol state
   - Initiator: `InitiatorAction` enum
   - Responder: `ResponderAction` enum

4. **Results** - Information from protocol state to stage state
   - Initiator: `InitiatorResult` struct
   - Responder: `ResponderResult` struct

## Conversion Steps

### Phase 1: Create New Module Structure

1. **Create `initiator.rs` module**
   - Define `TxSubmissionInitiator` struct (stage state)
   - Define `InitiatorAction` enum
   - Define `InitiatorResult` struct (if needed)
   - Implement `StageState` for `TxSubmissionInitiator`
   - Implement `ProtocolState` for `TxSubmissionState` (Initiator role)
   - Create `initiator()` function returning `Miniprotocol`
   - Create `register_deserializers()` function

2. **Create `responder.rs` module**
   - Define `TxSubmissionResponder` struct (stage state)
   - Define `ResponderAction` enum
   - Define `ResponderResult` struct
   - Implement `StageState` for `TxSubmissionResponder`
   - Implement `ProtocolState` for `TxSubmissionState` (Responder role)
   - Create `responder()` function returning `Miniprotocol`
   - Create `register_deserializers()` function

3. **Update `mod.rs`**
   - Add `initiator` and `responder` modules
   - Create `register_deserializers()` that combines both
   - Create `spec()` function for protocol validation
   - Update `register_tx_submission()` to use miniprotocol pattern

### Phase 2: Protocol State Implementation

1. **Implement `ProtocolState<Initiator>` for `TxSubmissionState`**
   - `init()`: Return `Outcome` with `Message::Init` to send, transition to `Idle`
   - `network()`: Handle `Message::RequestTxIds` and `Message::RequestTxs`
     - Map to appropriate `InitiatorResult` or error
   - `local()`: Handle `InitiatorAction` (SendReplyTxIds, SendReplyTxs)
     - Return `Outcome` with message to send

2. **Implement `ProtocolState<Responder>` for `TxSubmissionState`**
   - `init()`: Return empty `Outcome`, stay in `Init`
   - `network()`: Handle `Message::Init`, `Message::ReplyTxIds`, `Message::ReplyTxs`
     - Map to appropriate `ResponderResult`
   - `local()`: Handle `ResponderAction` (SendRequestTxIds, SendRequestTxs)
     - Return `Outcome` with message to send

### Phase 3: Stage State Implementation

1. **Implement `StageState` for `TxSubmissionInitiator`**
   - Store: `TxSubmissionInitiatorState` (window, last_seq), `muxer: StageRef<MuxMessage>`
   - `LocalIn`: `Void` (no local inputs currently, or create enum if needed)
   - `local()`: Handle local inputs (if any)
   - `network()`: Handle `InitiatorResult` from protocol state
     - Access mempool via `MemoryPool::new(eff.clone())`
     - Call existing logic from `TxSubmissionInitiatorState::step()`
     - Return `InitiatorAction` to trigger protocol state

2. **Implement `StageState` for `TxSubmissionResponder`**
   - Store: `TxSubmissionResponderState`, `muxer: StageRef<MuxMessage>`
   - `LocalIn`: `Void` (no local inputs)
   - `local()`: Handle local inputs (if any)
   - `network()`: Handle `ResponderResult` from protocol state
     - Access mempool via `MemoryPool::new(eff.clone())`
     - Call existing logic from `TxSubmissionResponderState::step()`
     - Return `ResponderAction` to trigger protocol state

### Phase 4: Action and Result Types

1. **Initiator Actions**

   ```rust
   pub enum InitiatorAction {
       SendReplyTxIds(Vec<(TxId, u32)>),
       SendReplyTxs(Vec<Tx>),
   }
   ```

2. **Initiator Results** (from network messages)

   ```rust
   pub struct InitiatorResult {
       // Could be RequestTxIds or RequestTxs
       // Or use separate result types
   }
   ```

   Actually, we might need:

   ```rust
   pub enum InitiatorResult {
       RequestTxIds { ack: u16, req: u16, blocking: Blocking },
       RequestTxs(Vec<TxId>),
   }
   ```

3. **Responder Actions**

   ```rust
   pub enum ResponderAction {
       SendRequestTxIds { ack: u16, req: u16, blocking: Blocking },
       SendRequestTxs(Vec<TxId>),
   }
   ```

4. **Responder Results** (from network messages)

   ```rust
   pub enum ResponderResult {
       Init,
       ReplyTxIds(Vec<(TxId, u32)>),
       ReplyTxs(Vec<Tx>),
   }
   ```

### Phase 5: Protocol Specification

1. **Create `spec()` function in `mod.rs`**
   - Define all valid state transitions
   - Use `ProtoSpec` builder pattern
   - Similar to keepalive's `spec()` function

### Phase 6: Registration Function

1. **Update `register_tx_submission()`**
   - Remove manual stage creation
   - Use `eff.stage()` with `initiator()` or `responder()` miniprotocol
   - Use `eff.wire_up()` with stage state
   - Use `eff.contramap()` to map `Inputs<LocalIn>` to handler
   - Register with muxer using `MuxMessage::Register`

### Phase 7: Refactor State Logic

1. **Extract protocol logic from `step()` methods**
   - Keep business logic in `TxSubmissionInitiatorState` and `TxSubmissionResponderState`
   - Protocol state transitions move to `ProtocolState` implementation
   - Stage state uses business logic to make decisions

2. **Handle async mempool operations**
   - `wait_for_at_least()` is async - need to handle in `network()` method
   - Use `eff.wait()` or similar for async operations

### Phase 8: Error Handling

1. **Convert `ProtocolError` to protocol state errors**
   - Return `anyhow::Result` from `ProtocolState` methods
   - Use `Outcome` to signal errors (may need to extend or use result type)

2. **Handle termination**
   - `Outcome::Done` should terminate connection
   - Errors should terminate connection

### Phase 9: Testing

1. **Update existing tests**
   - Tests currently use `step()` directly - need to adapt
   - May need to test protocol state and stage state separately
   - Integration tests should still work with new registration

2. **Add protocol spec tests**
   - Use `spec().check()` similar to keepalive tests

## Key Design Decisions

### 1. Mempool Access

- **Decision**: Access mempool via `MemoryPool::new(eff.clone())` in `StageState::network()` method
- **Rationale**: Mempool is a resource accessed through effects, not part of protocol state

### 2. Async Operations

- **Decision**: Handle async mempool operations (like `wait_for_at_least()`) in `StageState::network()`
- **Rationale**: Stage state has access to `Effects` which supports async operations

### 3. State Separation

- **Decision**: Keep `TxSubmissionInitiatorState` and `TxSubmissionResponderState` as separate structs
- **Rationale**: They contain business logic that's independent of protocol state machine

### 4. Protocol State vs Stage State

- **Protocol State** (`TxSubmissionState`): Pure state machine, no business logic
- **Stage State** (`TxSubmissionInitiator`/`TxSubmissionResponder`): Contains business logic state and makes decisions

### 5. Message Flow

- **Network → Protocol State**: `network()` receives `WireMsg`, returns `Outcome` with `Result`
- **Protocol State → Stage State**: `Result` passed to `network()` method
- **Stage State → Protocol State**: `network()` returns `Action`
- **Protocol State → Network**: `local()` receives `Action`, returns `Outcome` with `WireMsg` to send

## Migration Strategy

1. **Parallel implementation**: Create new modules alongside old ones
2. **Feature flag**: Use feature flag to switch between old and new implementation
3. **Gradual migration**: Migrate tests one by one
4. **Remove old code**: Once all tests pass, remove old implementation

## Files to Create/Modify

### New Files

- `crates/amaru-protocols/src/tx_submission/initiator.rs`
- `crates/amaru-protocols/src/tx_submission/responder.rs`

### Modified Files

- `crates/amaru-protocols/src/tx_submission/mod.rs`
- `crates/amaru-protocols/src/tx_submission/stage.rs` (may be removed or significantly reduced)

### Files to Keep (with possible modifications)

- `crates/amaru-protocols/src/tx_submission/initiator_state.rs` (business logic)
- `crates/amaru-protocols/src/tx_submission/responder_state.rs` (business logic)
- `crates/amaru-protocols/src/tx_submission/messages.rs` (wire messages)
- `crates/amaru-protocols/src/tx_submission/outcome.rs` (may be deprecated)
- `crates/amaru-protocols/src/tx_submission/responder_params.rs`

## Challenges and Considerations

### 1. Complex State Machine

- Tx submission has more states than keepalive (6 vs 2)
- Need to carefully map all state transitions

### 2. Async Mempool Operations

- `wait_for_at_least()` is async and blocking
- Need to handle this in stage state's async methods

### 3. Error Handling

- Current `Outcome::Error` needs to map to protocol state errors
- May need to use result types to pass errors

### 4. Initialization

- Initiator sends `Init` message on startup
- Responder receives `Init` and responds
- Need to handle this in `init()` methods

### 5. Protocol Errors

- Many protocol errors need to terminate connection
- Need to decide how to signal termination from protocol state

## Success Criteria

1. All existing tests pass with new implementation
2. Protocol spec validation passes
3. Code follows same patterns as keepalive
4. No functionality is lost
5. Code is cleaner and more maintainable
