// Copyright 2017 Parity Technologies (UK) Ltd.
// This file is part of Substrate Demo.

// Substrate Demo is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Substrate Demo is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Substrate Demo.  If not, see <http://www.gnu.org/licenses/>.

//! Democratic system: Handles administration of general stakeholder voting.

#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(feature = "std")] extern crate serde;

extern crate substrate_codec as codec;
#[cfg_attr(not(feature = "std"), macro_use)] extern crate substrate_runtime_std as rstd;
extern crate substrate_runtime_io as runtime_io;
#[macro_use] extern crate substrate_runtime_support as runtime_support;
extern crate substrate_runtime_primitives as primitives;
extern crate substrate_runtime_session as session;
extern crate substrate_runtime_staking as staking;
extern crate substrate_runtime_system as system;

use rstd::prelude::*;
//use rstd::cmp;
//use runtime_io::{twox_128, TestExternalities};
use primitives::{Zero, Executable, RefInto, As};
use runtime_support::{StorageValue, StorageMap, Parameter, Dispatchable};

mod vote_threshold;
pub use vote_threshold::{Approved, VoteThreshold};

/// A proposal index.
pub type PropIndex = u32;
/// A referendum index.
pub type ReferendumIndex = u32;

/// Is a proposal the "cancel_referendum"?
// &T::Proposal::Democracy(democracy::privileged::Call::cancel_referendum(ref_index))
pub trait IsCancelReferendum {
	fn is_cancel_referendum(&self) -> Option<ReferendumIndex>;
}

pub trait Trait: staking::Trait {
	type Proposal: Parameter + Dispatchable + IsCancelReferendum;
}

decl_module! {
	pub struct Module<T: Trait>;
	pub enum Call where aux: T::PublicAux {
		fn propose(aux, proposal: Box<T::Proposal>, value: T::Balance) = 0;
		fn second(aux, proposal: PropIndex) = 1;
		fn vote(aux, ref_index: ReferendumIndex, approve_proposal: bool) = 2;
	}
	pub enum PrivCall {
		fn start_referendum(proposal: Box<T::Proposal>, vote_threshold: VoteThreshold) = 0;
		fn cancel_referendum(ref_index: ReferendumIndex) = 1;
	}
}

decl_storage! {
	trait Store for Module<T: Trait>;

	// The number of (public) proposals that have been made so far.
	pub PublicPropCount get(public_prop_count): b"dem:ppc" => default PropIndex;
	// The public proposals. Unsorted.
	pub PublicProps get(public_props): b"dem:pub" => default Vec<(PropIndex, T::Proposal, T::AccountId)>;
	// Those who have locked a deposit.
	pub DepositOf get(deposit_of): b"dem:dep:" => map [ PropIndex => (T::Balance, Vec<T::AccountId>) ];
	// How often (in blocks) new public referenda are launched.
	pub LaunchPeriod get(launch_period): b"dem:lau" => required T::BlockNumber;
	// The minimum amount to be used as a deposit for a public referendum proposal.
	pub MinimumDeposit get(minimum_deposit): b"dem:min" => required T::Balance;

	// How often (in blocks) to check for new votes.
	pub VotingPeriod get(voting_period): b"dem:per" => required T::BlockNumber;

	// The next free referendum index, aka the number of referendums started so far.
	pub ReferendumCount get(referendum_count): b"dem:rco" => default ReferendumIndex;
	// The next referendum index that should be tallied.
	pub NextTally get(next_tally): b"dem:nxt" => default ReferendumIndex;
	// Information concerning any given referendum.
	pub ReferendumInfoOf get(referendum_info): b"dem:pro:" => map [ ReferendumIndex => (T::BlockNumber, T::Proposal, VoteThreshold) ];

	// Get the voters for the current proposal.
	pub VotersFor get(voters_for): b"dem:vtr:" => default map [ ReferendumIndex => Vec<T::AccountId> ];

	// Get the vote, if Some, of `who`.
	pub VoteOf get(vote_of): b"dem:vot:" => map [ (ReferendumIndex, T::AccountId) => bool ];
}

impl<T: Trait> Module<T> {

	// exposed immutables.

	/// Get the amount locked in support of `proposal`; false if proposal isn't a valid proposal
	/// index.
	pub fn locked_for(proposal: PropIndex) -> Option<T::Balance> {
		Self::deposit_of(proposal).map(|(d, l)| d * T::Balance::sa(l.len()))
	}

	/// Return true if `ref_index` is an on-going referendum.
	pub fn is_active_referendum(ref_index: ReferendumIndex) -> bool {
		<ReferendumInfoOf<T>>::exists(ref_index)
	}

	/// Get all referendums currently active.
	pub fn active_referendums() -> Vec<(ReferendumIndex, T::BlockNumber, T::Proposal, VoteThreshold)> {
		let next = Self::next_tally();
		let last = Self::referendum_count();
		(next..last).into_iter()
			.filter_map(|i| Self::referendum_info(i).map(|(n, p, t)| (i, n, p, t)))
			.collect()
	}

	/// Get all referendums ready for tally at block `n`.
	pub fn maturing_referendums_at(n: T::BlockNumber) -> Vec<(ReferendumIndex, T::BlockNumber, T::Proposal, VoteThreshold)> {
		let next = Self::next_tally();
		let last = Self::referendum_count();
		(next..last).into_iter()
			.filter_map(|i| Self::referendum_info(i).map(|(n, p, t)| (i, n, p, t)))
			.take_while(|&(_, block_number, _, _)| block_number == n)
			.collect()
	}

	/// Get the voters for the current proposal.
	pub fn tally(ref_index: ReferendumIndex) -> (T::Balance, T::Balance) {
		Self::voters_for(ref_index).iter()
			.map(|a| (<staking::Module<T>>::balance(a), Self::vote_of((ref_index, a.clone())).expect("all items come from `voters`; for an item to be in `voters` there must be a vote registered; qed")))
			.map(|(bal, vote)| if vote { (bal, Zero::zero()) } else { (Zero::zero(), bal) })
			.fold((Zero::zero(), Zero::zero()), |(a, b), (c, d)| (a + c, b + d))
	}

	// dispatching.

	/// Propose a sensitive action to be taken.
	fn propose(aux: &T::PublicAux, proposal: Box<T::Proposal>, value: T::Balance) {
		assert!(value >= Self::minimum_deposit());
		assert!(<staking::Module<T>>::deduct_unbonded(aux.ref_into(), value));

		let index = Self::public_prop_count();
		<PublicPropCount<T>>::put(index + 1);
		<DepositOf<T>>::insert(index, (value, vec![aux.ref_into().clone()]));

		let mut props = Self::public_props();
		props.push((index, (*proposal).clone(), aux.ref_into().clone()));
		<PublicProps<T>>::put(props);
	}

	/// Propose a sensitive action to be taken.
	fn second(aux: &T::PublicAux, proposal: PropIndex) {
		let mut deposit = Self::deposit_of(proposal).expect("can only second an existing proposal");
		assert!(<staking::Module<T>>::deduct_unbonded(aux.ref_into(), deposit.0));

		deposit.1.push(aux.ref_into().clone());
		<DepositOf<T>>::insert(proposal, deposit);
	}

	/// Vote in a referendum. If `approve_proposal` is true, the vote is to enact the proposal;
	/// false would be a vote to keep the status quo..
	fn vote(aux: &T::PublicAux, ref_index: ReferendumIndex, approve_proposal: bool) {
		if !Self::is_active_referendum(ref_index) {
			panic!("vote given for invalid referendum.")
		}
		if <staking::Module<T>>::balance(aux.ref_into()).is_zero() {
			panic!("transactor must have balance to signal approval.");
		}
		if !<VoteOf<T>>::exists(&(ref_index, aux.ref_into().clone())) {
			let mut voters = Self::voters_for(ref_index);
			voters.push(aux.ref_into().clone());
			<VotersFor<T>>::insert(ref_index, voters);
		}
		<VoteOf<T>>::insert(&(ref_index, aux.ref_into().clone()), approve_proposal);
	}

	/// Start a referendum.
	fn start_referendum(proposal: Box<T::Proposal>, vote_threshold: VoteThreshold) {
		Self::inject_referendum(<system::Module<T>>::block_number() + Self::voting_period(), *proposal, vote_threshold);
	}

	/// Remove a referendum.
	fn cancel_referendum(ref_index: ReferendumIndex) {
		Self::clear_referendum(ref_index);
	}

	// exposed mutables.

	/// Start a referendum. Can be called directly by the council.
	pub fn internal_start_referendum(proposal: T::Proposal, vote_threshold: VoteThreshold) {
		<Module<T>>::inject_referendum(<system::Module<T>>::block_number() + <Module<T>>::voting_period(), proposal, vote_threshold);
	}

	/// Remove a referendum. Can be called directly by the council.
	pub fn internal_cancel_referendum(ref_index: ReferendumIndex) {
		<Module<T>>::clear_referendum(ref_index);
	}

	// private.

	/// Start a referendum
	fn inject_referendum(
		end: T::BlockNumber,
		proposal: T::Proposal,
		vote_threshold: VoteThreshold
	) -> ReferendumIndex {
		let ref_index = Self::referendum_count();
		if ref_index > 0 && Self::referendum_info(ref_index - 1).map(|i| i.0 > end).unwrap_or(false) {
			panic!("Cannot inject a referendum that ends earlier than preceeding referendum");
		}

		<ReferendumCount<T>>::put(ref_index + 1);
		<ReferendumInfoOf<T>>::insert(ref_index, (end, proposal, vote_threshold));
		ref_index
	}

	/// Remove all info on a referendum.
	fn clear_referendum(ref_index: ReferendumIndex) {
		<ReferendumInfoOf<T>>::remove(ref_index);
		<VotersFor<T>>::remove(ref_index);
		for v in Self::voters_for(ref_index) {
			<VoteOf<T>>::remove((ref_index, v));
		}
	}

	/// Current era is ending; we should finish up any proposals.
	fn end_block(now: T::BlockNumber) {
		// pick out another public referendum if it's time.
		if (now % Self::launch_period()).is_zero() {
			let mut public_props = Self::public_props();
			if let Some((winner_index, _)) = public_props.iter()
				.enumerate()
				.max_by_key(|x| Self::locked_for((x.1).0).expect("All current public proposals have an amount locked"))
			{
				let (prop_index, proposal, _) = public_props.swap_remove(winner_index);
				let (deposit, depositors): (T::Balance, Vec<T::AccountId>) =
					<DepositOf<T>>::take(prop_index).expect("depositors always exist for current proposals");
				// refund depositors
				for d in &depositors {
					<staking::Module<T>>::refund(d, deposit);
				}
				<PublicProps<T>>::put(public_props);
				Self::inject_referendum(now + Self::voting_period(), proposal, VoteThreshold::SuperMajorityApprove);
			}
		}

		// tally up votes for any expiring referenda.
		for (index, _, proposal, vote_threshold) in Self::maturing_referendums_at(now) {
			let (approve, against) = Self::tally(index);
			let total_stake = <staking::Module<T>>::total_stake();
			Self::clear_referendum(index);
			if vote_threshold.approved(approve, against, total_stake) {
				proposal.dispatch();
			}
			<NextTally<T>>::put(index + 1);
		}
	}
}

impl<T: Trait> Executable for Module<T> {
	fn execute() {
		Self::end_block(<system::Module<T>>::block_number());
	}
}

/*


#[cfg(test)]
pub mod testing {
	use super::*;
	use runtime_io::{twox_128, TestExternalities};
	use runtime_support::{StorageList, StorageValue, StorageMap};
	use codec::Joiner;
	use keyring::Keyring::*;
	use runtime::{session, staking};

	pub fn externalities() -> TestExternalities {
		map![
			twox_128(session::SessionLength::key()).to_vec() => vec![].and(&1u64),
			twox_128(session::Validators::key()).to_vec() => vec![].and(&vec![Alice.to_raw_public(), Bob.into(), Charlie.into()]),
			twox_128(&staking::Intention::len_key()).to_vec() => vec![].and(&3u32),
			twox_128(&staking::Intention::key_for(0)).to_vec() => Alice.to_raw_public_vec(),
			twox_128(&staking::Intention::key_for(1)).to_vec() => Bob.to_raw_public_vec(),
			twox_128(&staking::Intention::key_for(2)).to_vec() => Charlie.to_raw_public_vec(),
			twox_128(&staking::FreeBalanceOf::key_for(*Alice)).to_vec() => vec![].and(&10u64),
			twox_128(&staking::FreeBalanceOf::key_for(*Bob)).to_vec() => vec![].and(&20u64),
			twox_128(&staking::FreeBalanceOf::key_for(*Charlie)).to_vec() => vec![].and(&30u64),
			twox_128(&staking::FreeBalanceOf::key_for(*Dave)).to_vec() => vec![].and(&40u64),
			twox_128(&staking::FreeBalanceOf::key_for(*Eve)).to_vec() => vec![].and(&50u64),
			twox_128(&staking::FreeBalanceOf::key_for(*Ferdie)).to_vec() => vec![].and(&60u64),
			twox_128(&staking::FreeBalanceOf::key_for(*One)).to_vec() => vec![].and(&1u64),
			twox_128(staking::TotalStake::key()).to_vec() => vec![].and(&210u64),
			twox_128(staking::SessionsPerEra::key()).to_vec() => vec![].and(&1u64),
			twox_128(staking::ValidatorCount::key()).to_vec() => vec![].and(&3u64),
			twox_128(staking::CurrentEra::key()).to_vec() => vec![].and(&1u64),
			twox_128(staking::TransactionFee::key()).to_vec() => vec![].and(&1u64),
			twox_128(staking::BondingDuration::key()).to_vec() => vec![].and(&0u64),

			twox_128(LaunchPeriod::key()).to_vec() => vec![].and(&1u64),
			twox_128(VotingPeriod::key()).to_vec() => vec![].and(&1u64),
			twox_128(MinimumDeposit::key()).to_vec() => vec![].and(&1u64)
		]
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use runtime_io::{with_externalities, twox_128, TestExternalities};
	use codec::{KeyedVec, Joiner};
	use keyring::Keyring::*;
	use demo_primitives::AccountId;
	use dispatch::PrivCall as T::Proposal;
	use runtime_support::PublicPass;
	use super::public::Dispatch;
	use super::privileged::Dispatch as PrivDispatch;
	use runtime::{staking, session, democracy};

	fn new_test_ext() -> TestExternalities {
		testing::externalities()
	}

	#[test]
	fn params_should_work() {
		with_externalities(&mut new_test_ext(), || {
			assert_eq!(launch_period(), 1u64);
			assert_eq!(voting_period(), 1u64);
			assert_eq!(minimum_deposit(), 1u64);
			assert_eq!(referendum_count(), 0u32);
			assert_eq!(staking::sessions_per_era(), 1u64);
			assert_eq!(staking::total_stake(), 210u64);
		});
	}

	// TODO: test VoteThreshold

	fn propose_sessions_per_era(who: &AccountId, value: u64, locked: T::Balance) {
		PublicPass::test(who).
			propose(Box::new(T::Proposal::Staking(staking::privileged::Call::set_sessions_per_era(value))), locked);
	}

	#[test]
	fn locked_for_should_work() {
		with_externalities(&mut new_test_ext(), || {
			system::testing::set_block_number(1);
			propose_sessions_per_era(&Alice, 2, 2u64);
			propose_sessions_per_era(&Alice, 4, 4u64);
			propose_sessions_per_era(&Alice, 3, 3u64);
			assert_eq!(locked_for(0), Some(2));
			assert_eq!(locked_for(1), Some(4));
			assert_eq!(locked_for(2), Some(3));
		});
	}

	#[test]
	fn single_proposal_should_work() {
		with_externalities(&mut new_test_ext(), || {
			system::testing::set_block_number(1);
			propose_sessions_per_era(&Alice, 2, 1u64);
			democracy::internal::end_block(system::block_number());

			system::testing::set_block_number(2);
			let r = 0;
			PublicPass::test(&Alice).vote(r, true);

			assert_eq!(referendum_count(), 1);
			assert_eq!(voters_for(r), vec![Alice.to_raw_public()]);
			assert_eq!(vote_of((r, *Alice)), Some(true));
			assert_eq!(tally(r), (10, 0));

			democracy::internal::end_block(system::block_number());
			staking::internal::check_new_era();

			assert_eq!(staking::era_length(), 2u64);
		});
	}

	#[test]
	fn deposit_for_proposals_should_be_taken() {
		with_externalities(&mut new_test_ext(), || {
			system::testing::set_block_number(1);
			propose_sessions_per_era(&Alice, 2, 5u64);
			PublicPass::test(&Bob).second(0);
			PublicPass::test(&Eve).second(0);
			PublicPass::test(&Eve).second(0);
			PublicPass::test(&Eve).second(0);
			assert_eq!(staking::balance(&Alice), 5u64);
			assert_eq!(staking::balance(&Bob), 15u64);
			assert_eq!(staking::balance(&Eve), 35u64);
		});
	}

	#[test]
	fn deposit_for_proposals_should_be_returned() {
		with_externalities(&mut new_test_ext(), || {
			system::testing::set_block_number(1);
			propose_sessions_per_era(&Alice, 2, 5u64);
			PublicPass::test(&Bob).second(0);
			PublicPass::test(&Eve).second(0);
			PublicPass::test(&Eve).second(0);
			PublicPass::test(&Eve).second(0);
			democracy::internal::end_block(system::block_number());
			assert_eq!(staking::balance(&Alice), 10u64);
			assert_eq!(staking::balance(&Bob), 20u64);
			assert_eq!(staking::balance(&Eve), 50u64);
		});
	}

	#[test]
	#[should_panic]
	fn proposal_with_deposit_below_minimum_should_panic() {
		with_externalities(&mut new_test_ext(), || {
			system::testing::set_block_number(1);
			propose_sessions_per_era(&Alice, 2, 0u64);
		});
	}

	#[test]
	#[should_panic]
	fn poor_proposer_should_panic() {
		with_externalities(&mut new_test_ext(), || {
			system::testing::set_block_number(1);
			propose_sessions_per_era(&Alice, 2, 11u64);
		});
	}

	#[test]
	#[should_panic]
	fn poor_seconder_should_panic() {
		with_externalities(&mut new_test_ext(), || {
			system::testing::set_block_number(1);
			propose_sessions_per_era(&Bob, 2, 11u64);
			PublicPass::test(&Alice).second(0);
		});
	}

	fn propose_bonding_duration(who: &AccountId, value: u64, locked: T::Balance) {
		PublicPass::test(who).
			propose(Box::new(T::Proposal::Staking(staking::privileged::Call::set_bonding_duration(value))), locked);
	}

	#[test]
	fn runners_up_should_come_after() {
		with_externalities(&mut new_test_ext(), || {
			system::testing::set_block_number(0);
			propose_bonding_duration(&Alice, 2, 2u64);
			propose_bonding_duration(&Alice, 4, 4u64);
			propose_bonding_duration(&Alice, 3, 3u64);
			democracy::internal::end_block(system::block_number());

			system::testing::set_block_number(1);
			PublicPass::test(&Alice).vote(0, true);
			democracy::internal::end_block(system::block_number());
			staking::internal::check_new_era();
			assert_eq!(staking::bonding_duration(), 4u64);

			system::testing::set_block_number(2);
			PublicPass::test(&Alice).vote(1, true);
			democracy::internal::end_block(system::block_number());
			staking::internal::check_new_era();
			assert_eq!(staking::bonding_duration(), 3u64);

			system::testing::set_block_number(3);
			PublicPass::test(&Alice).vote(2, true);
			democracy::internal::end_block(system::block_number());
			staking::internal::check_new_era();
			assert_eq!(staking::bonding_duration(), 2u64);
		});
	}

	fn sessions_per_era_propsal(value: u64) -> T::Proposal {
		T::Proposal::Staking(staking::privileged::Call::set_sessions_per_era(value))
	}

	#[test]
	fn simple_passing_should_work() {
		with_externalities(&mut new_test_ext(), || {
			system::testing::set_block_number(1);
			let r = inject_referendum(1, sessions_per_era_propsal(2), VoteThreshold::SuperMajorityApprove);
			PublicPass::test(&Alice).vote(r, true);

			assert_eq!(voters_for(r), vec![Alice.to_raw_public()]);
			assert_eq!(vote_of((r, *Alice)), Some(true));
			assert_eq!(tally(r), (10, 0));

			democracy::internal::end_block(system::block_number());
			staking::internal::check_new_era();

			assert_eq!(staking::era_length(), 2u64);
		});
	}

	#[test]
	fn cancel_referendum_should_work() {
		with_externalities(&mut new_test_ext(), || {
			system::testing::set_block_number(1);
			let r = inject_referendum(1, sessions_per_era_propsal(2), VoteThreshold::SuperMajorityApprove);
			PublicPass::test(&Alice).vote(r, true);
			PrivPass::test().cancel_referendum(r);

			democracy::internal::end_block(system::block_number());
			staking::internal::check_new_era();

			assert_eq!(staking::era_length(), 1u64);
		});
	}

	#[test]
	fn simple_failing_should_work() {
		with_externalities(&mut new_test_ext(), || {
			system::testing::set_block_number(1);
			let r = inject_referendum(1, sessions_per_era_propsal(2), VoteThreshold::SuperMajorityApprove);
			PublicPass::test(&Alice).vote(r, false);

			assert_eq!(voters_for(r), vec![Alice.to_raw_public()]);
			assert_eq!(vote_of((r, *Alice)), Some(false));
			assert_eq!(tally(r), (0, 10));

			democracy::internal::end_block(system::block_number());
			staking::internal::check_new_era();

			assert_eq!(staking::era_length(), 1u64);
		});
	}

	#[test]
	fn controversial_voting_should_work() {
		with_externalities(&mut new_test_ext(), || {
			system::testing::set_block_number(1);
			let r = inject_referendum(1, sessions_per_era_propsal(2), VoteThreshold::SuperMajorityApprove);
			PublicPass::test(&Alice).vote(r, true);
			PublicPass::test(&Bob).vote(r, false);
			PublicPass::test(&Charlie).vote(r, false);
			PublicPass::test(&Dave).vote(r, true);
			PublicPass::test(&Eve).vote(r, false);
			PublicPass::test(&Ferdie).vote(r, true);

			assert_eq!(tally(r), (110, 100));

			democracy::internal::end_block(system::block_number());
			staking::internal::check_new_era();

			assert_eq!(staking::era_length(), 2u64);
		});
	}

	#[test]
	fn controversial_low_turnout_voting_should_work() {
		with_externalities(&mut new_test_ext(), || {
			system::testing::set_block_number(1);
			let r = inject_referendum(1, sessions_per_era_propsal(2), VoteThreshold::SuperMajorityApprove);
			PublicPass::test(&Eve).vote(r, false);
			PublicPass::test(&Ferdie).vote(r, true);

			assert_eq!(tally(r), (60, 50));

			democracy::internal::end_block(system::block_number());
			staking::internal::check_new_era();

			assert_eq!(staking::era_length(), 1u64);
		});
	}

	#[test]
	fn passing_low_turnout_voting_should_work() {
		with_externalities(&mut new_test_ext(), || {
			assert_eq!(staking::era_length(), 1u64);
			assert_eq!(staking::total_stake(), 210u64);

			system::testing::set_block_number(1);
			let r = inject_referendum(1, sessions_per_era_propsal(2), VoteThreshold::SuperMajorityApprove);
			PublicPass::test(&Dave).vote(r, true);
			PublicPass::test(&Eve).vote(r, false);
			PublicPass::test(&Ferdie).vote(r, true);

			assert_eq!(tally(r), (100, 50));

			democracy::internal::end_block(system::block_number());
			staking::internal::check_new_era();

			assert_eq!(staking::era_length(), 2u64);
		});
	}
}


*/
