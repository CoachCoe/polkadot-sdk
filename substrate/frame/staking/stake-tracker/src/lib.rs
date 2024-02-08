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

//! # Stake Tracker Pallet
//!
//! The stake-tracker pallet is responsible to keep track of the voter's stake and target's approval
//! voting in the staking system.
//!
//! ## Overview
//!
//! The stake-tracker pallet listens to staking events through implementing the [`OnStakingUpdate`]
//! trait and, based on those events, ensures that the score of nodes in the lists
//! [`Config::VoterList`] and [`Config::TargetList`] are kept up to date with the staker's bonds
//! and nominations in the system. In addition, the pallet also ensures that [`Config::TargetList`]
//! is *strictly sorted* based on the targets' approvals.
//!
//! ## Goals
//!
//! The [`OnStakingUpdate`] implementation aims to achieve the following goals:
//!
//! * The [`Config::TargetList`] keeps a sorted list of validators, sorted by approvals
//! (which include self-vote and nominations' stake).
//! * The [`Config::VoterList`] keeps a semi-sorted list of voters, loosely sorted by bonded stake.
//! This pallet does nothing to ensure that the voter list sorting is correct.
//! * The [`Config::TargetList`] sorting must be *always* kept up to date, even in the event of new
//! nomination updates, nominator/validator slashes and rewards. This pallet *must* ensure that the
//! scores of the targets are always up to date *and* the targets are sorted by score at all time.
//!
//! Note that from the POV of this pallet, all events will result in one or multiple updates to
//! [`Config::VoterList`] and/or [`Config::TargetList`] state. If a set of staking updates require
//! too much weight to process (e.g. at nominator's rewards payout or at nominator's slashes), the
//! event emitter should handle that in some way (e.g. buffering events and implementing a
//! multi-block event emitter).
//!
//! ## Staker status and list invariants
//!
//! There are a few list invariants that depend on the staker's (nominator or validator) state, as
//! exposed by the [`Config::Staking`] interface:
//!
//! * A [`sp_staking::StakerStatus::Nominator`] is part of the voter list and its self-stake is the
//! voter list's score.
//! * A [`sp_staking::StakerStatus::Validator`] is part of both voter and target list. And its
//! approvals score (nominations + self-stake) is kept up to date as the target list's score.
//! * A [`sp_staking::StakerStatus::Idle`] may have a target list's score while other stakers
//!   nominate the idle validator.
//! * A staker which is not recognized by staking (i.e. not bonded) may still have an associated
//! target list score, in case there are other nominators nominating it. The list's node will
//! automatically be removed onced all the voters stop nominating the unbonded account.
//!
//! ## Domain-specific consideration on [`Config::VoterList`] and [`Config::TargetList`]
//!
//! In the context of Polkadot's staking system, both the voter and target lists will be implemented
//! by a bags-list pallet, which implements the
//! [`frame_election_provider_support::SortedListProvider`] trait.
//!
//! Note that the score provider of the target's bags-list is the list itself. This, coupled with
//! the fact that the target list sorting must be always up to date, makes this pallet resposible
//! for ensuring that the score of the targets in the `TargetList` is *always* kept up to date.
//!
//! ## Event emitter ordering and staking ledger state updates
//!
//! It is important to ensure that the events are emitted from staking (i.e. the calls into
//! [`OnStakingUpdate`]) *after* the staking ledger has been updated by the caller, since the new
//! state will be fetched and used to update the sorted lists accordingly.

#![cfg_attr(not(feature = "std"), no_std)]

pub use pallet::*;

use frame_election_provider_support::SortedListProvider;
use frame_support::{
	defensive,
	traits::{fungible::Inspect as FnInspect, Defensive, DefensiveSaturating},
};
use sp_npos_elections::ExtendedBalance;
use sp_runtime::traits::Zero;
use sp_staking::{
	currency_to_vote::CurrencyToVote, OnStakingUpdate, Stake, StakerStatus, StakingInterface,
};
use sp_std::{collections::btree_map::BTreeMap, vec, vec::Vec};

#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;

pub(crate) const LOG_TARGET: &str = "runtime::stake-tracker";

// syntactic sugar for logging.
#[macro_export]
macro_rules! log {
	($level:tt, $patter:expr $(, $values:expr)* $(,)?) => {
		log::$level!(
			target: $crate::LOG_TARGET,
			concat!("[{:?}] 📚 ", $patter), <frame_system::Pallet<T>>::block_number() $(, $values)*
		)
	};
}

/// The balance type of this pallet.
pub type BalanceOf<T> = <<T as Config>::Staking as StakingInterface>::Balance;
/// The account ID of this pallet.
pub type AccountIdOf<T> = <T as frame_system::Config>::AccountId;

/// Represents a stake imbalance to be applied to a staker's score.
#[derive(Copy, Clone, Debug)]
pub enum StakeImbalance<Balance> {
	/// Represents the reduction of stake by `Balance`.
	Negative(Balance),
	/// Represents the increase of stake by `Balance`.
	Positive(Balance),
}

#[frame_support::pallet]
pub mod pallet {
	use crate::*;
	use frame_election_provider_support::{ExtendedBalance, VoteWeight};
	use frame_support::pallet_prelude::*;
	use frame_system::pallet_prelude::BlockNumberFor;

	/// The current storage version.
	const STORAGE_VERSION: StorageVersion = StorageVersion::new(0);

	#[pallet::pallet]
	#[pallet::storage_version(STORAGE_VERSION)]
	pub struct Pallet<T>(_);

	#[pallet::config]
	pub trait Config: frame_system::Config {
		type Currency: FnInspect<Self::AccountId, Balance = BalanceOf<Self>>;

		/// The staking interface.
		type Staking: StakingInterface<AccountId = Self::AccountId>;

		/// Something that provides a *best-effort* sorted list of voters.
		///
		/// To keep the load off the chain as much as possible, changes made to the staked amount
		/// via rewards and slashes are dropped and thus need to be manually updated through
		/// extrinsics. In case of `bags-list`, this always means using `rebag` and `putInFrontOf`.
		type VoterList: SortedListProvider<Self::AccountId, Score = VoteWeight>;

		/// Something that provides an *always* sorted list of targets.
		///
		/// This pallet is responsible to keep the score and sorting of this pallet up to date with
		/// the correct approvals stakes of every target that is bouded or it has been bonded in the
		/// past *and* it still has nominations from active voters.
		type TargetList: SortedListProvider<
			Self::AccountId,
			Score = <Self::Staking as StakingInterface>::Balance,
		>;
	}

	#[pallet::hooks]
	impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
		#[cfg(feature = "try-runtime")]
		fn try_state(_n: BlockNumberFor<T>) -> Result<(), sp_runtime::TryRuntimeError> {
			Self::do_try_state()
		}
	}

	impl<T: Config> Pallet<T> {
		/// Returns the balance of a staker based on its current *active* stake, as returned by
		/// the staking interface.
		pub(crate) fn active_vote_of(who: &T::AccountId) -> BalanceOf<T> {
			T::Staking::stake(who).map(|s| s.active).defensive_unwrap_or_default()
		}

		/// Converts a balance into the staker's vote weight.
		pub(crate) fn weight_of(balance: BalanceOf<T>) -> VoteWeight {
			<T::Staking as StakingInterface>::CurrencyToVote::to_vote(
				balance,
				T::Currency::total_issuance(),
			)
		}

		/// Fetches and converts a voter's weight into the [`ExtendedBalance`] type for safe
		/// computation.
		pub(crate) fn to_vote_extended(balance: BalanceOf<T>) -> ExtendedBalance {
			<T::Staking as StakingInterface>::CurrencyToVote::to_vote(
				balance,
				T::Currency::total_issuance(),
			)
			.into()
		}

		/// Converts an [`sp_npos_elections::ExtendedBalance`] back to the staking interface's
		/// balance.
		pub(crate) fn to_currency(
			extended: ExtendedBalance,
		) -> <T::Staking as StakingInterface>::Balance {
			<T::Staking as StakingInterface>::CurrencyToVote::to_currency(
				extended,
				T::Currency::total_issuance(),
			)
		}

		/// Returns whether a target should be removed from the target list.
		///
		/// A target should be removed from the target list at any point IFF:
		/// * it's approvals are 0 AND
		/// * it's state is dangling (ledger unbonded).
		pub(crate) fn should_remove_target(who: &T::AccountId, score: BalanceOf<T>) -> bool {
			score.is_zero() && T::Staking::status(who).is_err()
		}

		/// Updates a target's score by increasing/decreasing an imbalance of the current score in
		/// the target list.
		pub(crate) fn update_target_score(
			who: &T::AccountId,
			imbalance: StakeImbalance<ExtendedBalance>,
		) {
			// if the target is not part of the list, check state of the target to update first.
			if !T::TargetList::contains(who) {
				match T::Staking::status(who) {
					Err(_) | Ok(StakerStatus::Nominator(_)) => {
						defensive!("update target score was called on an unbonded ledger or nominator, not expected.");
						return
					},
					Ok(StakerStatus::Validator) => {
						defensive!(
							"active validator was not part of the target list, something is wrong."
						);
						return
					},
					Ok(StakerStatus::Idle) => {
						// if stash is idle and not part of the target list yet, initialize it and
						// proceed.
						T::TargetList::on_insert(who.clone(), Zero::zero())
							.expect("staker does not exist in the list as per check above; qed.");
					},
				}
			}

			// update target score.
			let removed = match imbalance {
				StakeImbalance::Positive(imbalance) => {
					let _ = T::TargetList::on_increase(who, Self::to_currency(imbalance))
						.defensive_proof(
							"staker should exist in the list, otherwise returned earlier.",
						);
					false
				},
				StakeImbalance::Negative(imbalance) => {
					if let Ok(current_score) = T::TargetList::get_score(who) {
						let balance =
							Self::to_vote_extended(current_score).saturating_sub(imbalance);

						// the target is removed from the list IFF score is 0 and the target is
						// dangling (i.e. not bonded).
						if Self::should_remove_target(who, Self::to_currency(balance)) {
							let _ = T::TargetList::on_remove(who).defensive_proof(
								"staker exists in the list as per the check above; qed.",
							);
							true
						} else {
							// update the target score without removing it.
							let _ = T::TargetList::on_update(who, Self::to_currency(balance))
								.defensive_proof(
									"staker exists in the list as per the check above; qed.",
								);
							false
						}
					} else {
						defensive!("unexpected: unable to fetch score from staking interface of an existent staker");
						false
					}
				},
			};

			log!(
				debug,
				"update_score of {:?} by {:?}. removed target node? {}",
				who,
				imbalance,
				removed
			);
		}
	}
}

#[cfg(any(test, feature = "try-runtime"))]
impl<T: Config> Pallet<T> {
	/// Try-state checks for the stake-tracker pallet.
	///
	/// 1. [`Self::do_try_state_approvals`]: checks the curent approval stake in the target list
	///    compared with the staking state.
	/// 2. [`Self::do_try_state_target_sorting`]: checks if the target list is sorted by score.
	pub fn do_try_state() -> Result<(), sp_runtime::TryRuntimeError> {
		Self::do_try_state_approvals()
		//Self::do_try_state_target_sorting()
	}

	/// Try-state: checks if the approvals stake of the targets in the target list are correct.
	///
	/// These try-state checks generate a map with approval stake of all the targets based on
	/// the staking state of stakers in the voter and target lists. In doing so, we are able to
	/// verify that the current voter and target lists and scores are in sync with the staking
	/// data and perform other sanity checks as the approvals map is calculated.
	///
	/// NOTE: this is an expensive state check since it iterates over all the nodes in the
	/// target and voter list providers.
	///
	/// Invariants:
	///
	/// * Target List:
	///   * The sum of the calculated approvals stake is the same as the current approvals in
	///   the target list per target.
	///   * The target score of an active validator is the sum of all of its nominators' stake
	///   and the self-stake;
	///   * The target score of an idle validator (i.e. chilled) is the sum of its nominator's
	///   stake. An idle target may not be part of the target list, if it has no nominations.
	///   * The target score of a "dangling" target (ie. idle AND unbonded validator) must
	///   always be > 0. We expect the stake-tracker to have cleaned up dangling targets with 0
	///   score.
	///   * The number of target nodes in the target list matches the number of
	///   (active_validators + idle_validators + dangling_targets_score_with_score).
	///
	/// * Voter List:
	///  * The voter score is the same as the active stake of the corresponding stash.
	///  * An active validator should also be part of the voter list.
	///  * An idle validator should not be part of the voter list.
	///  * A dangling target shoud not be part of the voter list.
	pub(crate) fn do_try_state_approvals() -> Result<(), sp_runtime::TryRuntimeError> {
		let mut approvals_map: BTreeMap<AccountIdOf<T>, ExtendedBalance> = BTreeMap::new();

		// build map of approvals stakes from the `VoterList` POV.
		for voter in T::VoterList::iter() {
			if let Some(nominations) = <T::Staking as StakingInterface>::nominations(&voter) {
				let score = <T::VoterList as SortedListProvider<AccountIdOf<T>>>::get_score(&voter)
					.map_err(|_| "nominator score must exist in voter bags list")?;

				// sanity check.
				let active_stake = T::Staking::stake(&voter)
					.map(|s| Self::weight_of(s.active))
					.expect("active voter has bonded stake; qed.");
				frame_support::ensure!(
					active_stake == score,
					"voter score must be the same as its active stake"
				);

				for nomination in nominations {
					if let Some(stake) = approvals_map.get_mut(&nomination) {
						*stake += score as ExtendedBalance;
					} else {
						approvals_map.insert(nomination, score.into());
					}
				}
			}
		}

		// add self-vote of active targets to calculated approvals from the `TargetList` POV.
		for target in T::TargetList::iter() {
			// also checks invariant: all active targets are also voters.
			let maybe_self_stake = match T::Staking::status(&target) {
				Err(_) => {
					// if target is "dangling" (i.e unbonded but still in the `TargetList`), it
					// should NOT be part of the voter list.
					frame_support::ensure!(
						!T::VoterList::contains(&target),
						"dangling target (i.e. unbonded) should not be part of the voter list"
					);

					// if target is dangling, its target score should > 0 (otherwise it should
					// have been removed from the list).
					frame_support::ensure!(
                            T::TargetList::get_score(&target).expect("target must have score") > Zero::zero(),
                            "dangling target (i.e. unbonded) is part of the `TargetList` IFF it's approval voting > 0"
                        );
					// no self-stake and it should not be part of the target list.
					None
				},
				Ok(StakerStatus::Idle) => {
					// target is idle and not part of the voter list.
					frame_support::ensure!(
						!T::VoterList::contains(&target),
						"chilled validator (idle target) should not be part of the voter list"
					);

					// no sef-stake but since it's chilling, it should be part of the TL even
					// with score = 0.
					Some(0)
				},
				Ok(StakerStatus::Validator) => {
					// active target should be part of the voter list.
					frame_support::ensure!(
						T::VoterList::contains(&target),
						"bonded and active validator should also be part of the voter list"
					);
					// return self-stake (ie. active bonded).
					T::Staking::stake(&target).map(|s| Self::weight_of(s.active)).ok()
				},
				Ok(StakerStatus::Nominator(_)) => {
					panic!("staker with nominator status should not be part of the target list");
				},
			};

			if let Some(score) = maybe_self_stake {
				if let Some(stake) = approvals_map.get_mut(&target) {
					*stake += score as ExtendedBalance;
				} else {
					approvals_map.insert(target, score.into());
				}
			} else {
				// unbonded target: it does not have self-stake.
			}
		}

		log!(trace, "try-state: calculated approvals map: {:?}", approvals_map);

		// compare calculated approvals per target with target list state.
		for (target, calculated_stake) in approvals_map.iter() {
			let stake_in_list = T::TargetList::get_score(target).expect("target must exist; qed.");
			let stake_in_list = Self::to_vote_extended(stake_in_list);

			if *calculated_stake != stake_in_list {
				log!(
						error,
						"try-runtime: score of {:?} in `TargetList` list: {:?}, calculated sum of all stake: {:?}",
						target,
						stake_in_list,
						calculated_stake,
					);

				return Err("target score in the target list is different than the expected".into())
			}
		}

		frame_support::ensure!(
			approvals_map.keys().count() == T::TargetList::iter().count(),
			"calculated approvals count is different from total of target list.",
		);

		Ok(())
	}

	/// Try-state: checks if targets in the target list are sorted by score.
	///
	/// Invariant
	///  * All targets in the target list are sorted by their score.
	#[allow(dead_code)] // TODO(remove)
	pub fn do_try_state_target_sorting() -> Result<(), sp_runtime::TryRuntimeError> {
		let mut current_highest = None;

		for t in T::TargetList::iter() {
			let target_score = T::TargetList::get_score(&t).expect("score must exist");

			// first iter.
			if current_highest.is_none() {
				current_highest = Some(target_score);
			}

			frame_support::ensure!(
				current_highest.expect("is set based on the above; qed.") >= target_score,
				"target list is not sorted",
			);
		}

		Ok(())
	}
}

impl<T: Config> OnStakingUpdate<T::AccountId, BalanceOf<T>> for Pallet<T> {
	/// Fired when the stake amount of some staker updates.
	///
	/// When a nominator's stake is updated, all the nominated targets must be updated accordingly.
	///
	/// Note: it is assumed that `who`'s staking ledger state is updated *before* this method is
	/// called.
	fn on_stake_update(
		who: &T::AccountId,
		prev_stake: Option<Stake<BalanceOf<T>>>,
		stake: Stake<BalanceOf<T>>,
	) {
		// closure to calculate the stake imbalance of a staker.
		let stake_imbalance_of = |prev_stake: Option<Stake<BalanceOf<T>>>,
		                          voter_weight: ExtendedBalance| {
			if let Some(prev_stake) = prev_stake {
				let prev_voter_weight = Self::to_vote_extended(prev_stake.active);

				if prev_voter_weight > voter_weight {
					StakeImbalance::Negative(
						prev_voter_weight.defensive_saturating_sub(voter_weight),
					)
				} else {
					StakeImbalance::Positive(
						voter_weight.defensive_saturating_sub(prev_voter_weight),
					)
				}
			} else {
				// if staker had no stake before update, then add all the voter weight
				// to the target's score.
				StakeImbalance::Positive(voter_weight)
			}
		};

		if T::Staking::status(who)
			.and(T::Staking::stake(who))
			.defensive_proof(
				"staker should exist when calling on_stake_update and have a valid status",
			)
			.is_ok()
		{
			let voter_weight = Self::weight_of(stake.active);

			match T::Staking::status(who).expect("status checked above; qed.") {
				StakerStatus::Nominator(nominations) => {
					let _ = T::VoterList::on_update(who, voter_weight).defensive_proof(
						"staker should exist in VoterList, as per the contract \
                            with staking.",
					);

					let stake_imbalance = stake_imbalance_of(prev_stake, voter_weight.into());

					log!(
						debug,
						"on_stake_update: {:?} with {:?}. impacting nominations {:?}",
						who,
						stake_imbalance,
						nominations,
					);

					// updates vote weight of nominated targets accordingly. Note: this will update
					// the score of up to `T::MaxNominations` validators.
					for target in nominations.into_iter() {
						Self::update_target_score(&target, stake_imbalance);
					}
				},
				StakerStatus::Validator => {
					// validator is both a target and a voter.
					let stake_imbalance = stake_imbalance_of(prev_stake, voter_weight.into());
					Self::update_target_score(who, stake_imbalance);

					let _ = T::VoterList::on_update(who, voter_weight).defensive_proof(
						"the staker should exit in VoterList, as per the \
                            contract with staking.",
					);
				},
				StakerStatus::Idle => (), // nothing to see here.
			}
		}
	}

	/// Fired when someone sets their intention to validate.
	///
	/// A validator is also considered a voter with self-vote and should also be added to
	/// [`Config::VoterList`].
	//
	/// Note: it is assumed that `who`'s ledger staking state is updated *before* calling this
	/// method.
	fn on_validator_add(who: &T::AccountId, self_stake: Option<Stake<BalanceOf<T>>>) {
		let self_stake = self_stake.unwrap_or_default().active;

		if !T::TargetList::contains(who) {
			T::TargetList::on_insert(who.clone(), self_stake)
				.expect("staker does not exist in the list as per check above; qed.");
		} else {
			// if the target already exists in the list, it means that the target has been idle
			// and/or dangling.
			debug_assert!(
				T::Staking::status(who) == Ok(StakerStatus::Idle) ||
					T::Staking::status(who).is_err()
			);

			let self_stake = Self::to_vote_extended(self_stake);
			Self::update_target_score(who, StakeImbalance::Positive(self_stake));
		}

		log!(debug, "on_validator_add: {:?}. role: {:?}", who, T::Staking::status(who),);

		// a validator is also a nominator.
		Self::on_nominator_add(who, vec![])
	}

	/// Fired when a validator becomes idle (i.e. chilling).
	///
	/// While chilled, the target node remains in the target list.
	///
	/// While idling, the target node is not removed from the target list but its score is updated.
	fn on_validator_idle(who: &T::AccountId) {
		let self_stake = Self::weight_of(Self::active_vote_of(who));
		Self::update_target_score(who, StakeImbalance::Negative(self_stake.into()));

		// validator is a nominator too.
		Self::on_nominator_idle(who, vec![]);

		log!(debug, "on_validator_idle: {:?}, decreased self-stake {}", who, self_stake);
	}

	/// Fired when someone removes their intention to validate and has been completely removed from
	/// the staking state.
	///
	/// The node is removed from the target list IFF its score is 0.
	fn on_validator_remove(who: &T::AccountId) {
		log!(debug, "on_validator_remove: {:?}", who,);

		// validator must be idle before removing completely.
		match T::Staking::status(who) {
			Ok(StakerStatus::Idle) => (), // proceed
			Ok(StakerStatus::Validator) => Self::on_validator_idle(who),
			Ok(StakerStatus::Nominator(_)) => {
				defensive!("on_validator_remove called on a nominator, unexpected.");
				return
			},
			Err(_) => {
				defensive!("on_validator_remove called on a non-existing target.");
				return
			},
		};

		// remove from target list IIF score is zero.
		if T::TargetList::get_score(who).unwrap_or_default().is_zero() {
			T::TargetList::on_remove(who).expect("target exists as per the check above; qed.");
		}
	}

	/// Fired when someone sets their intention to nominate.
	///
	/// Note: it is assumed that `who`'s ledger staking state is updated *before* this method is
	/// called.
	fn on_nominator_add(who: &T::AccountId, nominations: Vec<AccountIdOf<T>>) {
		let nominator_vote = Self::weight_of(Self::active_vote_of(who));

		// voter may exist in the list in case of re-enabling a chilled nominator;
		if T::VoterList::contains(who) {
			return
		}

		let _ = T::VoterList::on_insert(who.clone(), nominator_vote)
			.defensive_proof("staker does not exist in the list as per check above; qed.");

		// If who is a nominator, update the vote weight of the nominations if they exist. Note:
		// this will update the score of up to `T::MaxNominations` validators.
		match T::Staking::status(who).defensive() {
			Ok(StakerStatus::Nominator(_)) =>
				for t in nominations {
					Self::update_target_score(&t, StakeImbalance::Positive(nominator_vote.into()))
				},
			Ok(StakerStatus::Idle) | Ok(StakerStatus::Validator) | Err(_) => (), // nada.
		};

		log!(debug, "on_nominator_add: {:?}. role: {:?}", who, T::Staking::status(who),);
	}

	/// Fired when a nominator becomes idle (i.e. chilling).
	///
	/// From the `T::VotertList` PoV, chilling a nominator is the same as removing it.
	///
	/// Note: it is assumed that `who`'s staking ledger and `nominations` are up to date before
	/// calling this method.
	fn on_nominator_idle(who: &T::AccountId, nominations: Vec<T::AccountId>) {
		Self::on_nominator_remove(who, nominations);
	}

	/// Fired when someone removes their intention to nominate and is completely removed from the
	/// staking state.
	///
	/// Note: the number of nodes that are updated is bounded by the maximum number of nominators,
	/// which is defined in the staking pallet.
	fn on_nominator_remove(who: &T::AccountId, nominations: Vec<T::AccountId>) {
		let nominator_vote = Self::weight_of(Self::active_vote_of(who));

		log!(
			debug,
			"remove nominations from {:?} with {:?} weight. impacting {:?}.",
			who,
			nominator_vote,
			nominations,
		);

		// updates the nominated target's score.
		for t in nominations.iter() {
			Self::update_target_score(t, StakeImbalance::Negative(nominator_vote.into()))
		}

		let _ = T::VoterList::on_remove(who)
			.defensive_proof("the nominator exists in the list as per the contract with staking.");
	}

	/// Fired when an existing nominator updates their nominations.
	///
	/// This is called when a nominator updates their nominations. The nominator's stake remains the
	/// same (updates to the nominator's stake should emit [`Self::on_stake_update`] instead).
	/// However, the score of the nominated targets must be updated accordingly.
	///
	/// Note: it is assumed that `who`'s ledger staking state is updated *before* calling this
	/// method.
	fn on_nominator_update(
		who: &T::AccountId,
		prev_nominations: Vec<T::AccountId>,
		nominations: Vec<AccountIdOf<T>>,
	) {
		let nominator_vote = Self::weight_of(Self::active_vote_of(who));

		log!(
			debug,
			"on_nominator_update: {:?}, with {:?}. previous nominations: {:?} -> new nominations {:?}",
			who, nominator_vote, prev_nominations, nominations,
		);

		// new nominations
		for target in nominations.iter() {
			if !prev_nominations.contains(target) {
				Self::update_target_score(target, StakeImbalance::Positive(nominator_vote.into()));
			}
		}
		// removed nominations
		for target in prev_nominations.iter() {
			if !nominations.contains(target) {
				Self::update_target_score(target, StakeImbalance::Negative(nominator_vote.into()));
			}
		}
	}

	/// Fired when a slash happens.
	///
	/// In practice, this is a noop in the context of the stake tracker, since the score of the
	/// voters and targets are updated through the `ledger.update` calls following the slash.
	fn on_slash(
		_stash: &T::AccountId,
		_slashed_active: BalanceOf<T>,
		_slashed_unlocking: &BTreeMap<sp_staking::EraIndex, BalanceOf<T>>,
		_slashed_total: BalanceOf<T>,
	) {
	}
}
