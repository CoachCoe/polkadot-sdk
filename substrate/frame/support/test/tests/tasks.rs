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

#![cfg(feature = "experimental")]

#[frame_support::pallet(dev_mode)]
mod my_pallet {
	use frame_support::pallet_prelude::{StorageValue, ValueQuery};

	#[pallet::config]
	pub trait Config<I: 'static = ()>: frame_system::Config {}

	#[pallet::pallet]
	pub struct Pallet<T, I = ()>(_);

	#[pallet::storage]
	pub type SomeStorage<T, I = ()> = StorageValue<_, (u32, u64), ValueQuery>;

	#[pallet::tasks_experimental]
	impl<T: Config<I>, I> Pallet<T, I> {
		#[pallet::task_index(0)]
		#[pallet::task_condition(|i, j| i == 0u32 && j == 2u64)]
		#[pallet::task_list(vec![(0u32, 2u64), (2u32, 4u64)].iter())]
		#[pallet::task_weight(0.into())]
		fn foo(i: u32, j: u64) -> frame_support::pallet_prelude::DispatchResult {
			<SomeStorage<T, I>>::put((i, j));
			Ok(())
		}
	}
}

type BlockNumber = u32;
type AccountId = u64;
type Header = sp_runtime::generic::Header<BlockNumber, sp_runtime::traits::BlakeTwo256>;
type UncheckedExtrinsic = sp_runtime::generic::UncheckedExtrinsic<u32, RuntimeCall, (), ()>;
type Block = sp_runtime::generic::Block<Header, UncheckedExtrinsic>;

#[frame_support::runtime]
mod runtime {
	#[runtime::runtime]
	#[runtime::derive(
		RuntimeCall,
		RuntimeEvent,
		RuntimeError,
		RuntimeOrigin,
		RuntimeFreezeReason,
		RuntimeHoldReason,
		RuntimeSlashReason,
		RuntimeLockId,
		RuntimeTask
	)]
	pub struct Runtime;

	#[runtime::pallet_index(0)]
	pub type System = frame_system;

	#[runtime::pallet_index(1)]
	pub type MyPallet = my_pallet;
}

// NOTE: Needed for derive_impl expansion
use frame_support::derive_impl;
#[frame_support::derive_impl(frame_system::config_preludes::TestDefaultConfig as frame_system::DefaultConfig)]
impl frame_system::Config for Runtime {
	type Block = Block;
	type AccountId = AccountId;
}

impl my_pallet::Config for Runtime {}

fn new_test_ext() -> sp_io::TestExternalities {
	use sp_runtime::BuildStorage;

	RuntimeGenesisConfig::default().build_storage().unwrap().into()
}

#[test]
fn tasks_work() {
	new_test_ext().execute_with(|| {
		let task = RuntimeTask::MyPallet(my_pallet::Task::<Runtime>::Foo { i: 0u32, j: 2u64 });

		frame_support::assert_ok!(System::do_task(RuntimeOrigin::signed(1), task.clone(),));
		assert_eq!(my_pallet::SomeStorage::<Runtime>::get(), (0, 2));
	});
}