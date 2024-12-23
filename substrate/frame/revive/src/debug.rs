// This file is part of Substrate.

// Copyright (C) Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: Apache-2.0

// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// 	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

pub use crate::{
	evm::{CallTrace, CallType, Traces},
	exec::{ExecResult, ExportedFunction},
	primitives::ExecReturnValue,
	BalanceOf,
};
use crate::{Config, GasMeter, LOG_TARGET};
use alloc::vec::Vec;
use sp_core::{H160, U256};

/// Umbrella trait for all interfaces that serves for debugging.
pub trait Debugger<T: Config>: CallInterceptor<T> {}

impl<T: Config, V> Debugger<T> for V where V: CallInterceptor<T> {}

#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub enum Tracer {
	#[default]
	Disabled,
	CallTracer(CallTracer),
}

/// Defines methods to capture contract calls
pub trait Tracing<T: Config>: Default {
	/// Called before a contract call is executed
	fn enter_child_span(
		&mut self,
		from: &H160,
		to: &H160,
		is_delegate_call: bool,
		is_read_only: bool,
		value: &U256,
		input: &[u8],
		gas_meter: &GasMeter<T>,
	);

	/// Called after a contract call is executed
	fn exit_child_span(&mut self, output: &ExecReturnValue, gas_meter: &GasMeter<T>);
}

impl Tracer {
	/// Creates a new [`Tracer::CallTracer`].
	pub fn new_call_tracer() -> Self {
		Tracer::CallTracer(CallTracer::default())
	}

	/// Returns the call tracer if it is enabled.
	pub fn as_call_tracer(self) -> Option<CallTracer> {
		match self {
			Tracer::CallTracer(tracer) => Some(tracer),
			_ => None,
		}
	}

	/// Returns the traces collected by the tracer.
	pub fn traces(self) -> Traces {
		return match self {
			Tracer::CallTracer(tracer) => Traces::CallTraces(tracer.traces),
			Tracer::Disabled => Traces::CallTraces(Vec::new()),
		};
	}
}

impl<T: Config> Tracing<T> for Tracer
where
	BalanceOf<T>: Into<U256>,
{
	fn enter_child_span(
		&mut self,
		from: &H160,
		to: &H160,
		is_delegate_call: bool,
		is_read_only: bool,
		value: &U256,
		input: &[u8],
		gas_meter: &GasMeter<T>,
	) {
		match self {
			Tracer::CallTracer(tracer) => {
				<CallTracer as Tracing<T>>::enter_child_span(
					tracer,
					from,
					to,
					is_delegate_call,
					is_read_only,
					value,
					input,
					gas_meter,
				);
			},
			Tracer::Disabled => {
				log::trace!(target: LOG_TARGET, "call (delegate: {is_delegate_call:?}, read_only: {is_read_only:?}) from: {from:?}, to: {to:?} value: {value:?}  input_data: {input:?}");
			},
		}
	}

	//fn after_call(&mut self, output: &ExecReturnValue);
	fn exit_child_span(&mut self, output: &ExecReturnValue, gas_meter: &GasMeter<T>) {
		match self {
			Tracer::CallTracer(tracer) => {
				<CallTracer as Tracing<T>>::exit_child_span(tracer, output, gas_meter);
			},
			Tracer::Disabled => {
				log::trace!(target: LOG_TARGET, "call result {output:?}")
			},
		}
	}
}

#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub struct CallTracer {
	/// Store all in-progress CallTrace instances
	pub traces: Vec<CallTrace>,
	/// Stack of indices to the current active traces
	current_stack: Vec<usize>,
}

impl<T: Config> Tracing<T> for CallTracer
where
	BalanceOf<T>: Into<U256>,
{
	fn enter_child_span(
		&mut self,
		from: &H160,
		to: &H160,
		is_delegate_call: bool,
		is_read_only: bool,
		value: &U256,
		input: &[u8],
		gas_meter: &GasMeter<T>,
	) {
		let call_type = if is_read_only {
			CallType::StaticCall
		} else if is_delegate_call {
			CallType::DelegateCall
		} else {
			CallType::Call
		};

		self.traces.push(CallTrace {
			from: *from,
			to: *to,
			value: (*value).into(),
			call_type,
			input: input.to_vec(),
			gas: gas_meter.gas_left(),
			..Default::default()
		});

		// Push the index onto the stack of the current active trace
		self.current_stack.push(self.traces.len() - 1);
	}
	fn exit_child_span(&mut self, output: &ExecReturnValue, gas_meter: &GasMeter<T>) {
		// Set the output of the current trace
		let current_index = self.current_stack.pop().unwrap();
		let trace = &mut self.traces[current_index];
		trace.output = output.data.clone();
		trace.gas_used = gas_meter.gas_consumed();

		//  move the current trace into its parent
		if let Some(parent_index) = self.current_stack.last() {
			let child_trace = self.traces.remove(current_index);
			self.traces[*parent_index].calls.push(child_trace);
		}
	}
}

/// Provides an interface for intercepting contract calls.
pub trait CallInterceptor<T: Config> {
	/// Allows to intercept contract calls and decide whether they should be executed or not.
	/// If the call is intercepted, the mocked result of the call is returned.
	///
	/// # Arguments
	///
	/// * `contract_address` - The address of the contract that is about to be executed.
	/// * `entry_point` - Describes whether the call is the constructor or a regular call.
	/// * `input_data` - The raw input data of the call.
	///
	/// # Expected behavior
	///
	/// This method should return:
	/// * `Some(ExecResult)` - if the call should be intercepted and the mocked result of the call
	/// is returned.
	/// * `None` - otherwise, i.e. the call should be executed normally.
	fn intercept_call(
		_contract_address: &H160,
		_entry_point: ExportedFunction,
		_input_data: &[u8],
	) -> Option<ExecResult> {
		None
	}
}

impl<T: Config> CallInterceptor<T> for () {}
