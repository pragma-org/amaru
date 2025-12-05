// Copyright 2025 PRAGMA
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::{
    CallId, Instant, Name, SendData, StageResponse,
    effect::{StageEffect, TransitionFactory},
    simulation::state::{StageData, StageState},
    trace_buffer::TraceBuffer,
};
use std::{mem::replace, time::Duration};
use tokio::sync::oneshot::{Receiver, Sender};

pub fn resume_receive_internal(
    trace_buffer: &mut TraceBuffer,
    data: &mut StageData,
    run: &mut dyn FnMut(Name, StageResponse),
) -> anyhow::Result<()> {
    let waiting_for = data
        .waiting
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("stage `{}` was not waiting for any effect", data.name))?;

    if !matches!(waiting_for, StageEffect::Receive) {
        anyhow::bail!(
            "stage `{}` was not waiting for a receive effect, but {:?}",
            data.name,
            waiting_for
        )
    }

    let msg = data
        .mailbox
        .pop_front()
        .ok_or_else(|| anyhow::anyhow!("mailbox is empty while resuming receive"))?;

    // it is important that all validations (i.e. `?``) happen before this point
    data.waiting = None;

    let StageState::Idle(state) = replace(&mut data.state, StageState::Failed(String::new()))
    else {
        panic!(
            "stage {} must have been Idle, was {:?}",
            data.name, data.state
        );
    };

    trace_buffer.push_receive(&data.name, &msg);
    data.state = StageState::Running((data.transition)(state, msg));

    run(data.name.clone(), StageResponse::Unit);
    Ok(())
}

pub fn post_message(
    data: &mut StageData,
    mailbox_size: usize,
    msg: Box<dyn SendData>,
) -> Result<(), Box<dyn SendData>> {
    if data.mailbox.len() >= mailbox_size {
        return Err(msg);
    }
    data.mailbox.push_back(msg);
    Ok(())
}

pub fn resume_send_internal(
    data: &mut StageData,
    run: &mut dyn FnMut(Name, StageResponse),
    to: Name,
) -> anyhow::Result<Option<(Duration, Receiver<Box<dyn SendData>>, CallId)>> {
    let waiting_for = data
        .waiting
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("stage `{}` was not waiting for any effect", data.name))?;

    if !matches!(waiting_for, StageEffect::Send(name, _msg, _call) if name == &to) {
        anyhow::bail!(
            "stage `{}` was not waiting for a send effect to `{}`, but {:?}",
            data.name,
            to,
            waiting_for
        )
    }

    // it is important that all validations (i.e. `?``) happen before this point
    let Some(StageEffect::Send(_to, _msg, call)) = data.waiting.take() else {
        panic!("match is guaranteed by the validation above");
    };

    if call.is_none() {
        run(data.name.clone(), StageResponse::Unit);
    }
    Ok(call)
}

pub fn resume_clock_internal(
    data: &mut StageData,
    run: &mut dyn FnMut(Name, StageResponse),
    time: Instant,
) -> anyhow::Result<()> {
    let waiting_for = data
        .waiting
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("stage `{}` was not waiting for any effect", data.name))?;

    if !matches!(waiting_for, StageEffect::Clock) {
        anyhow::bail!(
            "stage `{}` was not waiting for a clock effect, but {:?}",
            data.name,
            waiting_for
        )
    }

    // it is important that all validations (i.e. `?``) happen before this point
    data.waiting = None;

    run(data.name.clone(), StageResponse::ClockResponse(time));
    Ok(())
}

pub fn resume_wait_internal(
    data: &mut StageData,
    run: &mut dyn FnMut(Name, StageResponse),
    time: Instant,
) -> anyhow::Result<()> {
    let waiting_for = data
        .waiting
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("stage `{}` was not waiting for any effect", data.name))?;

    if !matches!(waiting_for, StageEffect::Wait(_duration)) {
        anyhow::bail!(
            "stage `{}` was not waiting for a wait effect, but {:?}",
            data.name,
            waiting_for
        )
    }

    // it is important that all validations (i.e. `?``) happen before this point
    data.waiting = None;

    run(data.name.clone(), StageResponse::WaitResponse(time));
    Ok(())
}

pub fn resume_call_internal(
    data: &mut StageData,
    run: &mut dyn FnMut(Name, StageResponse),
    id: CallId,
) -> anyhow::Result<()> {
    let waiting_for = data
        .waiting
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("stage `{}` was not waiting for any effect", data.name))?;

    if !matches!(waiting_for, StageEffect::Call(_,_,_,_,id2) if id2 == &id) {
        anyhow::bail!(
            "stage `{}` was not waiting for a call effect, but {:?}",
            data.name,
            waiting_for
        )
    }

    // it is important that all validations (i.e. `?``) happen before this point
    let Some(StageEffect::Call(_name, _timeout, _msg, mut recv, _id)) = data.waiting.take() else {
        panic!("match is guaranteed by the validation above");
    };

    let msg = recv
        .try_recv()
        .ok()
        .map(StageResponse::CallResponse)
        .unwrap_or(StageResponse::CallTimeout);

    run(data.name.clone(), msg);
    Ok(())
}

pub fn resume_respond_internal(
    data: &mut StageData,
    run: &mut dyn FnMut(Name, StageResponse),
    target: Name,
    id: CallId,
) -> anyhow::Result<(Name, CallId, Instant, Sender<Box<dyn SendData>>)> {
    let waiting_for = data
        .waiting
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("stage `{}` was not waiting for any effect", data.name))?;

    if !matches!(waiting_for, StageEffect::Respond(name, id2, _deadline, _sender, _msg) if id2 == &id && name == &target)
    {
        anyhow::bail!(
            "stage `{}` was not waiting for a respond effect with id {id:?}, but {:?}",
            data.name,
            waiting_for
        )
    }

    // it is important that all validations (i.e. `?``) happen before this point
    let Some(StageEffect::Respond(target, _id2, deadline, sender, _msg)) = data.waiting.take()
    else {
        panic!("match is guaranteed by the validation above");
    };

    run(data.name.clone(), StageResponse::Unit);
    Ok((target, id, deadline, sender))
}

pub fn resume_external_internal(
    data: &mut StageData,
    result: Box<dyn SendData>,
    run: &mut dyn FnMut(Name, StageResponse),
) -> anyhow::Result<()> {
    let waiting_for = data
        .waiting
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("stage `{}` was not waiting for any effect", data.name))?;

    if !matches!(waiting_for, StageEffect::External(_)) {
        anyhow::bail!(
            "stage `{}` was not waiting for an external effect",
            data.name
        )
    }

    // it is important that all validations (i.e. `?``) happen before this point
    data.waiting = None;

    run(data.name.clone(), StageResponse::ExternalResponse(result));
    Ok(())
}

pub fn resume_add_stage_internal(
    data: &mut StageData,
    run: &mut dyn FnMut(Name, StageResponse),
    name: Name,
) -> anyhow::Result<()> {
    let waiting_for = data
        .waiting
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("stage `{}` was not waiting for any effect", data.name))?;

    if !matches!(waiting_for, StageEffect::AddStage(_)) {
        anyhow::bail!(
            "stage `{}` was not waiting for a add stage effect, but {:?}",
            data.name,
            waiting_for
        )
    }

    // it is important that all validations (i.e. `?``) happen before this point
    data.waiting = None;

    run(data.name.clone(), StageResponse::AddStageResponse(name));
    Ok(())
}

pub fn resume_wire_stage_internal(
    data: &mut StageData,
    run: &mut dyn FnMut(Name, StageResponse),
) -> anyhow::Result<TransitionFactory> {
    let waiting_for = data
        .waiting
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("stage `{}` was not waiting for any effect", data.name))?;

    if !matches!(waiting_for, StageEffect::WireStage(_, _, _)) {
        anyhow::bail!(
            "stage `{}` was not waiting for a wire stage effect, but {:?}",
            data.name,
            waiting_for
        )
    }

    // it is important that all validations (i.e. `?``) happen before this point
    let Some(StageEffect::WireStage(_, transition, _)) = data.waiting.take() else {
        panic!("checked above");
    };

    run(data.name.clone(), StageResponse::Unit);
    Ok(transition.into_inner())
}
