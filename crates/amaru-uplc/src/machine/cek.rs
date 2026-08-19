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

use bumpalo::collections::Vec as BumpVec;

use super::{
    CostModel, ExBudget, MachineError, cost_model::StepKind, discharge, info::MachineInfo, runtime::Runtime,
    value::Value,
};
use crate::{
    arena::Arena,
    binder::Eval,
    constant::Constant,
    machine::{MachineVersion, context::Context, env::Env, state::MachineState},
    term::Term,
};

pub struct Machine<'a> {
    pub(super) arena: &'a Arena,
    initial_budget: ExBudget,
    ex_budget: ExBudget,
    unbudgeted_steps: [u8; StepKind::LEN + 1],
    pub(super) costs: CostModel,
    slippage: u8,
    pub(super) logs: Vec<String>,
    machine_version: MachineVersion,
}

impl<'a> Machine<'a> {
    pub fn new(arena: &'a Arena, initial_budget: ExBudget, costs: CostModel, machine_version: MachineVersion) -> Self {
        Machine {
            arena,
            initial_budget,
            ex_budget: initial_budget,
            unbudgeted_steps: [0; StepKind::LEN + 1],
            costs,
            slippage: 200,
            logs: Vec::new(),
            machine_version,
        }
    }

    pub fn info(self) -> MachineInfo {
        MachineInfo {
            remaining_budget: self.ex_budget,
            consumed_budget: self.initial_budget - self.ex_budget,
            logs: self.logs,
        }
    }

    pub fn run<V>(&mut self, term: &'a Term<'a, V>) -> Result<&'a Term<'a, V>, MachineError<'a, V>>
    where
        V: Eval<'a>,
    {
        self.spend_budget(self.costs.machine_costs.startup)?;

        let initial_context = Context::no_frame(self.arena);

        let mut state = MachineState::compute(self.arena, initial_context, Env::new_in(self.arena), term);

        loop {
            let step = match state {
                MachineState::Compute(context, env, term) => self.compute(context, env, term),
                MachineState::Return(context, value) => self.return_compute(context, value),
                MachineState::Done(term) => {
                    return Ok(term);
                }
            };

            state = step?;
        }
    }

    pub fn compute<V>(
        &mut self,
        context: &'a Context<'a, V>,
        env: &'a Env<'a, V>,
        term: &'a Term<'a, V>,
    ) -> Result<&'a mut MachineState<'a, V>, MachineError<'a, V>>
    where
        V: Eval<'a>,
    {
        match term {
            Term::Var(name) => {
                self.step_and_maybe_spend(StepKind::Var)?;

                let value = env.lookup(name.index()).ok_or(MachineError::OpenTermEvaluated(term))?;

                let state = MachineState::return_(self.arena, context, value);

                Ok(state)
            }
            Term::Lambda { parameter, body } => {
                self.step_and_maybe_spend(StepKind::Lambda)?;

                let value = Value::lambda(self.arena, *parameter, body, env);

                let state = MachineState::return_(self.arena, context, value);

                Ok(state)
            }
            Term::Apply { function, argument } => {
                self.step_and_maybe_spend(StepKind::Apply)?;

                let frame = Context::frame_await_fun_term(self.arena, env, argument, context);

                let state = MachineState::compute(self.arena, frame, env, function);

                Ok(state)
            }
            Term::Delay(body) => {
                self.step_and_maybe_spend(StepKind::Delay)?;

                let value = Value::delay(self.arena, body, env);

                let state = MachineState::return_(self.arena, context, value);

                Ok(state)
            }
            Term::Force(body) => {
                self.step_and_maybe_spend(StepKind::Force)?;

                let frame = Context::frame_force(self.arena, context);

                let state = MachineState::compute(self.arena, frame, env, body);

                Ok(state)
            }
            Term::Constr { tag, fields } => {
                self.step_and_maybe_spend(StepKind::Constr)?;

                if let Some((first, terms)) = fields.split_first() {
                    let frame = Context::frame_constr_empty(self.arena, env, *tag, terms, context);

                    let state = MachineState::compute(self.arena, frame, env, first);

                    Ok(state)
                } else {
                    let value = Value::constr_empty(self.arena, *tag);

                    let state = MachineState::return_(self.arena, context, value);

                    Ok(state)
                }
            }
            Term::Case { constr, branches } => {
                self.step_and_maybe_spend(StepKind::Case)?;

                let frame = Context::frame_cases(self.arena, env, branches, context);

                let state = MachineState::compute(self.arena, frame, env, constr);

                Ok(state)
            }
            Term::Constant(constant) => {
                self.step_and_maybe_spend(StepKind::Constant)?;

                let value = Value::con(self.arena, constant);

                let state = MachineState::return_(self.arena, context, value);

                Ok(state)
            }
            Term::Builtin(fun) => {
                self.step_and_maybe_spend(StepKind::Builtin)?;

                let runtime = Runtime::new(self.arena, *fun);

                let value = Value::builtin(self.arena, runtime);

                let state = MachineState::return_(self.arena, context, value);

                Ok(state)
            }
            Term::Error => Err(MachineError::ExplicitErrorTerm),
        }
    }

    pub fn return_compute<V>(
        &mut self,
        context: &'a Context<'a, V>,
        value: &'a Value<'a, V>,
    ) -> Result<&'a mut MachineState<'a, V>, MachineError<'a, V>>
    where
        V: Eval<'a>,
    {
        match context {
            Context::FrameAwaitFunTerm(arg_env, argument, context) => {
                let context = Context::frame_await_arg(self.arena, value, context);

                let state = MachineState::compute(self.arena, context, arg_env, argument);

                Ok(state)
            }
            Context::FrameAwaitArg(function, context) => self.apply_evaluate(context, function, value),
            Context::FrameAwaitFunValue(argument, context) => self.apply_evaluate(context, value, argument),
            Context::FrameForce(context) => self.force_evaluate(context, value),
            Context::FrameConstr(env, tag, terms, values, context) => {
                let mut new_values = BumpVec::with_capacity_in(values.len() + 1, self.arena.as_bump());

                for value in values.iter() {
                    new_values.push(*value);
                }

                new_values.push(value);

                let values = self.arena.alloc(new_values);

                if let Some((first, terms)) = terms.split_first() {
                    let frame = Context::frame_constr(self.arena, env, *tag, terms, values, context);

                    let state = MachineState::compute(self.arena, frame, env, first);

                    Ok(state)
                } else {
                    let value = Value::constr(self.arena, *tag, values);

                    let state = MachineState::return_(self.arena, context, value);

                    Ok(state)
                }
            }
            Context::FrameCases(env, branches, context) => match value {
                Value::Constr(tag, fields) => {
                    if let Some(branch) = branches.get(*tag) {
                        let frame = self.transfer_arg_stack(fields, context);

                        let state = MachineState::compute(self.arena, frame, env, branch);

                        Ok(state)
                    } else {
                        Err(MachineError::MissingCaseBranch(branches, value))
                    }
                }
                Value::Con(constant) if self.machine_version.is_constr_case_available() => {
                    let (tag, max_branches, fields) = self.constant_as_tag_fields(constant)?;

                    if branches.len() > max_branches {
                        return Err(MachineError::MissingCaseBranch(branches, value));
                    }

                    if let Some(branch) = branches.get(tag) {
                        let frame = self.transfer_arg_stack(fields, context);

                        let state = MachineState::compute(self.arena, frame, env, branch);

                        Ok(state)
                    } else {
                        Err(MachineError::MissingCaseBranch(branches, value))
                    }
                }
                v => Err(MachineError::NonConstrScrutinized(v)),
            },
            Context::NoFrame => {
                if self.unbudgeted_steps[StepKind::LEN] > 0 {
                    self.spend_unbudgeted_steps()?;
                }

                let term = discharge::value_as_term(self.arena, value);

                let state = MachineState::done(self.arena, term);

                Ok(state)
            }
        }
    }

    fn force_evaluate<V>(
        &mut self,
        context: &'a Context<'a, V>,
        value: &'a Value<'a, V>,
    ) -> Result<&'a mut MachineState<'a, V>, MachineError<'a, V>>
    where
        V: Eval<'a>,
    {
        match value {
            Value::Delay(term, env) => Ok(MachineState::compute(self.arena, context, env, term)),
            Value::Builtin(runtime) => {
                if runtime.needs_force() {
                    let value = if runtime.is_ready() {
                        self.call(runtime)?
                    } else {
                        Value::builtin(self.arena, runtime.force(self.arena))
                    };

                    let state = MachineState::return_(self.arena, context, value);

                    Ok(state)
                } else {
                    let term = discharge::value_as_term(self.arena, value);

                    Err(MachineError::BuiltinTermArgumentExpected(term))
                }
            }
            rest => Err(MachineError::NonPolymorphicInstantiation(rest)),
        }
    }

    fn apply_evaluate<V>(
        &mut self,
        context: &'a Context<'a, V>,
        function: &'a Value<'a, V>,
        argument: &'a Value<'a, V>,
    ) -> Result<&'a mut MachineState<'a, V>, MachineError<'a, V>>
    where
        V: Eval<'a>,
    {
        match function {
            Value::Lambda { body, env, .. } => {
                let new_env = env.push(self.arena, argument);

                let state = MachineState::compute(self.arena, context, new_env, body);

                Ok(state)
            }
            Value::Builtin(runtime) => {
                if !runtime.needs_force() && runtime.is_arrow() {
                    let runtime = runtime.push(self.arena, argument);

                    let value = if runtime.is_ready() {
                        self.eval_builtin_app(runtime)?
                    } else {
                        Value::builtin(self.arena, runtime)
                    };

                    let state = MachineState::return_(self.arena, context, value);

                    Ok(state)
                } else {
                    let term = discharge::value_as_term(self.arena, function);

                    Err(MachineError::UnexpectedBuiltinTermArgument(term))
                }
            }
            rest => Err(MachineError::NonFunctionApplication(argument, rest)),
        }
    }

    fn eval_builtin_app<V>(&mut self, runtime: &'a Runtime<'a, V>) -> Result<&'a Value<'a, V>, MachineError<'a, V>>
    where
        V: Eval<'a>,
    {
        self.call(runtime)
    }

    fn transfer_arg_stack<V>(
        &mut self,
        fields: &'a [&'a Value<'a, V>],
        context: &'a Context<'a, V>,
    ) -> &'a Context<'a, V>
    where
        V: Eval<'a>,
    {
        let mut c = context;

        for field in fields.iter().rev() {
            c = Context::frame_await_fun_value(self.arena, *field, c);
        }

        c
    }

    /// Decompose a constant into (tag, max_branches, fields) for constant-case.
    #[allow(clippy::type_complexity)]
    fn constant_as_tag_fields<V>(
        &self,
        constant: &'a Constant<'a>,
    ) -> Result<(usize, usize, &'a [&'a Value<'a, V>]), MachineError<'a, V>>
    where
        V: Eval<'a>,
    {
        let empty: &'a [&'a Value<'a, V>] = self.arena.alloc(BumpVec::new_in(self.arena.as_bump()));
        match constant {
            Constant::Unit => Ok((0, 1, empty)),
            Constant::Boolean(false) => Ok((0, 2, empty)),
            Constant::Boolean(true) => Ok((1, 2, empty)),
            Constant::Integer(i) => {
                let tag = usize::try_from(*i).map_err(|_| MachineError::outside_usize_bounds(i))?;
                Ok((tag, usize::MAX, empty))
            }
            Constant::ProtoList(ty, items) => {
                if items.is_empty() {
                    Ok((1, 2, empty))
                } else {
                    let head = Value::con(self.arena, items[0]);
                    let tail = Value::con(self.arena, Constant::proto_list(self.arena, ty, &items[1..]));

                    let mut fields = BumpVec::with_capacity_in(2, self.arena.as_bump());
                    fields.push(head);
                    fields.push(tail);
                    Ok((0, 2, self.arena.alloc(fields)))
                }
            }
            Constant::ProtoPair(_, _, first, second) => {
                let first_val = Value::con(self.arena, first);
                let second_val = Value::con(self.arena, second);

                let mut fields = BumpVec::with_capacity_in(2, self.arena.as_bump());
                fields.push(first_val);
                fields.push(second_val);
                Ok((0, 1, self.arena.alloc(fields)))
            }
            _ => Err(MachineError::NonConstrScrutinized(Value::con(self.arena, constant))),
        }
    }

    fn step_and_maybe_spend<V>(&mut self, step: StepKind) -> Result<(), MachineError<'a, V>>
    where
        V: Eval<'a>,
    {
        let index = step as usize;

        self.unbudgeted_steps[index] += 1;
        self.unbudgeted_steps[StepKind::LEN] += 1;

        if self.unbudgeted_steps[StepKind::LEN] >= self.slippage {
            self.spend_unbudgeted_steps()?;
        }

        Ok(())
    }

    fn spend_unbudgeted_steps<V>(&mut self) -> Result<(), MachineError<'a, V>>
    where
        V: Eval<'a>,
    {
        for step_kind in StepKind::enumerate() {
            let unspent_step_budget =
                self.costs.machine_costs.step(step_kind).scale(self.unbudgeted_steps[step_kind as usize]);

            self.spend_budget(unspent_step_budget)?;

            self.unbudgeted_steps[step_kind as usize] = 0;
        }

        self.unbudgeted_steps[StepKind::LEN] = 0;

        Ok(())
    }

    pub(super) fn spend_budget<V>(&mut self, spend_budget: ExBudget) -> Result<(), MachineError<'a, V>>
    where
        V: Eval<'a>,
    {
        self.ex_budget.mem = self.ex_budget.mem.saturating_sub(spend_budget.mem);
        self.ex_budget.cpu = self.ex_budget.cpu.saturating_sub(spend_budget.cpu);

        if self.ex_budget.mem < 0 || self.ex_budget.cpu < 0 {
            Err(MachineError::OutOfExError(self.ex_budget))
        } else {
            Ok(())
        }
    }
}
