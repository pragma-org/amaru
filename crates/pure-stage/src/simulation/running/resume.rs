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
    adapter::StageOrAdapter,
    effect::{CallExtra, CallTimeout, StageEffect, TransitionFactory},
    simulation::{
        SimulationRunning,
        running::{AssertStage, DeliverMessageResult, LogTermination},
        state::{StageData, StageState},
    },
    trace_buffer::TerminationReason,
};
use anyhow::Context;
use std::mem::replace;

#[derive(Debug, thiserror::Error)]
#[error("stage `{0}` terminated by unsupervised child termination")]
pub struct UnsupervisedChildTermination(pub Name);

/// Try to resume a receive effect.
///
/// Returns `Ok(true)` if the receive was in fact resumed, `Ok(false)` if the stage was not waiting for a receive effect,
/// or `Err` if the simulation should be terminated due to a bug or a top-level stage termination.
pub fn resume_receive_internal(
    simulation: &mut SimulationRunning,
    at_stage: &Name,
) -> anyhow::Result<bool> {
    let data = simulation
        .stages
        .get_mut(at_stage)
        .ok_or_else(|| anyhow::anyhow!("stage `{}` was already terminated", at_stage))?;
    let StageOrAdapter::Stage(data) = data else {
        panic!("stage is an adapter, which cannot receive");
    };
    let Some(waiting_for) = data.waiting.as_ref() else {
        return Ok(false);
    };

    if !matches!(waiting_for, StageEffect::Receive) {
        return Ok(false);
    }

    let msg = match data.tombstones.pop_front() {
        Some(Ok(msg)) => msg,
        Some(Err(name)) => {
            tracing::info!(parent = %data.name, child = %name, "terminated by unsupervised child termination");
            let (supervised_by, msg) = simulation
                .terminate_stage(at_stage.clone(), TerminationReason::Supervision(name))
                .ok_or_else(|| anyhow::anyhow!("stage was already terminated"))?;
            let Some(StageOrAdapter::Stage(supervisor)) = simulation.stages.get_mut(&supervised_by)
            else {
                tracing::error!(%at_stage, "terminating simulation due to unsupervised stage termination");
                simulation.terminate.send_replace(true);
                return Err(UnsupervisedChildTermination(at_stage.clone()).into());
            };
            supervisor.tombstones.push_back(msg);
            resume_receive_internal(simulation, &supervised_by)
                .with_context(|| format!("sending tombstone from `{}`", at_stage))?;
            return Ok(false);
        }
        None => {
            let Some(msg) = data.mailbox.pop_front() else {
                return Ok(false);
            };
            msg
        }
    };

    // it is important that all validations (i.e. `?``) happen before this point
    data.waiting = None;

    let StageState::Idle(state) = replace(&mut data.state, StageState::Idle(Box::new(()))) else {
        panic!(
            "stage {} must have been Idle, was {:?}",
            data.name, data.state
        );
    };

    simulation.trace_buffer.lock().push_input(&data.name, &msg);
    data.state = StageState::Running((data.transition)(state, msg));

    simulation
        .runnable
        .push_back((data.name.clone(), StageResponse::Unit));
    Ok(true)
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
            "stage `{}` was not waiting for a send effect to `{}`/{}, but {:?}",
            data.name,
            to,
            call,
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

pub fn resume_call_send_internal(
    sim: &mut SimulationRunning,
    from: Name,
    to: Name,
    msg: Box<dyn SendData>,
) -> anyhow::Result<bool> {
    let Some(data_from) = sim.stages.get_mut(&from).log_termination(&from) else {
        return Ok(false);
    };
    let data_from = data_from.assert_stage("which cannot receive call effects");
    let Some(StageEffect::Call(_, _, CallExtra::Scheduled(id))) = data_from.waiting.as_ref() else {
        tracing::warn!(stage = %from, "stage was not waiting for a call effect, but {:?}", data_from.waiting);
        return Ok(false);
    };
    let id = *id;

    let Some(real_to) =
        (match super::deliver_message(&mut sim.stages, sim.mailbox_size, to.clone(), msg) {
            DeliverMessageResult::Delivered(data_to) => {
                // `to` may not be suspended on receive, so failure to resume is okay
                let name = data_to.name.clone();
                resume_receive_internal(sim, &name)?;
                Some(name)
            }
            DeliverMessageResult::Full(data_to, send_data) => {
                data_to.senders.push_back((from.clone(), send_data));
                Some(data_to.name.clone())
            }
            DeliverMessageResult::NotFound => {
                tracing::warn!(stage = %to, "message send to terminated stage dropped");
                None
            }
        })
    else {
        return Ok(false);
    };

    sim.schedule_wakeup(id, move |sim| {
        let Some(data_from) = sim.stages.get_mut(&from) else {
            tracing::warn!(name = %from, "stage was terminated, skipping call effect delivery");
            return;
        };
        let wakeup = resume_call_internal(
            data_from.assert_stage("which cannot wait"),
            &mut |name, response| {
                sim.runnable.push_back((name, response));
            },
            Some(id),
            Box::new(CallTimeout),
        );
        if wakeup.is_ok()
            && let Some(StageOrAdapter::Stage(data_to)) = sim.stages.get_mut(&real_to)
        {
            data_to.senders.retain(|(name, _)| name != &from);
        }
    });

    Ok(true)
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

    if !matches!(waiting_for, StageEffect::Call(_,_,CallExtra::Scheduled(id2)) if id.iter().all(|id| id == id2))
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

    if !matches!(waiting_for, StageEffect::WireStage(..)) {
        anyhow::bail!(
            "stage `{}` was not waiting for a wire stage effect, but {:?}",
            data.name,
            waiting_for
        )
    }

    // it is important that all validations (i.e. `?``) happen before this point
    let Some(StageEffect::WireStage(_, transition, _, _)) = data.waiting.take() else {
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
