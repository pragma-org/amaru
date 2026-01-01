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
    Instant, Name, ScheduleId, SendData, StageResponse,
    effect::{CallExtra, StageEffect, TransitionFactory},
    simulation::state::{StageData, StageState},
    trace_buffer::TraceBuffer,
};
use std::mem::replace;

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

pub fn resume_send_internal(
    data: &mut StageData,
    run: &mut dyn FnMut(Name, StageResponse),
    to: Name,
    call: bool,
) -> anyhow::Result<Option<ScheduleId>> {
    let waiting_for = data
        .waiting
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("stage `{}` was not waiting for any effect", data.name))?;

    if !matches!(waiting_for, StageEffect::Send(name, call2, _msg) if name == &to && call2.is_some() == call)
    {
        anyhow::bail!(
            "stage `{}` was not waiting for a send effect to `{}`, but {:?}",
            data.name,
            to,
            waiting_for
        )
    }

    // it is important that all validations (i.e. `?``) happen before this point
    let Some(StageEffect::Send(_, call, _)) = data.waiting.take() else {
        panic!("checked above");
    };
    let call = call.map(|call| {
        *call
            .downcast_ref::<ScheduleId>()
            .expect("StageRef extra must be a ScheduleId")
    });

    run(data.name.clone(), StageResponse::Unit);
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
    id: Option<ScheduleId>,
    msg: Box<dyn SendData>,
) -> anyhow::Result<()> {
    let waiting_for = data
        .waiting
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("stage `{}` was not waiting for any effect", data.name))?;

    if !matches!(waiting_for, StageEffect::Call(_,_,CallExtra::ScheduleId(id2)) if id.iter().all(|id| id == id2))
    {
        anyhow::bail!(
            "stage `{}` was not waiting for a call effect, but {:?}",
            data.name,
            waiting_for
        )
    }

    // it is important that all validations (i.e. `?``) happen before this point
    data.waiting = None;

    run(data.name.clone(), StageResponse::CallResponse(msg));
    Ok(())
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
            "stage `{}` was not waiting for an add stage effect, but {:?}",
            data.name,
            waiting_for
        )
    }

    // it is important that all validations (i.e. `?`) happen before this point
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

pub fn resume_contramap_internal(
    data: &mut StageData,
    run: &mut dyn FnMut(Name, StageResponse),
    orig: Name,
    name: Name,
) -> anyhow::Result<Box<dyn Fn(Box<dyn SendData>) -> Box<dyn SendData> + Send + 'static>> {
    let waiting_for = data
        .waiting
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("stage `{}` was not waiting for any effect", data.name))?;

    if !matches!(waiting_for, StageEffect::Contramap { original, new_name, .. } if original == &orig && name.as_str().starts_with(new_name.as_str()))
    {
        anyhow::bail!(
            "stage `{}` was not waiting for a contramap effect, but {:?}",
            data.name,
            waiting_for
        )
    }

    // it is important that all validations (i.e. `?``) happen before this point
    let Some(StageEffect::Contramap { transform, .. }) = data.waiting.take() else {
        panic!("checked above");
    };

    run(data.name.clone(), StageResponse::ContramapResponse(name));
    Ok(transform.into_inner())
}

pub fn resume_schedule_internal(
    data: &mut StageData,
    run: &mut dyn FnMut(Name, StageResponse),
    id: ScheduleId,
) -> anyhow::Result<()> {
    let waiting_for = data
        .waiting
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("stage `{}` was not waiting for any effect", data.name))?;

    if !matches!(waiting_for, StageEffect::Schedule(_, id2) if id2 == &id) {
        anyhow::bail!(
            "stage `{}` was not waiting for a schedule effect with id {:?}, but {:?}",
            data.name,
            id,
            waiting_for
        )
    }

    // it is important that all validations (i.e. `?`) happen before this point
    data.waiting = None;

    run(data.name.clone(), StageResponse::Unit);
    Ok(())
}

pub fn resume_cancel_schedule_internal(
    data: &mut StageData,
    run: &mut dyn FnMut(Name, StageResponse),
    cancelled: bool,
) -> anyhow::Result<()> {
    let waiting_for = data
        .waiting
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("stage `{}` was not waiting for any effect", data.name))?;

    if !matches!(waiting_for, StageEffect::CancelSchedule(_)) {
        anyhow::bail!(
            "stage `{}` was not waiting for a cancel_schedule effect, but {:?}",
            data.name,
            waiting_for
        )
    }

    // it is important that all validations (i.e. `?`) happen before this point
    data.waiting = None;

    run(
        data.name.clone(),
        StageResponse::CancelScheduleResponse(cancelled),
    );
    Ok(())
}
