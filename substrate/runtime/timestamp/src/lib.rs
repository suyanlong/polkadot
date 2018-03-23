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

//! Timestamp manager: just handles the current timestamp.

#![cfg_attr(not(feature = "std"), no_std)]

#[cfg_attr(test, macro_use)] extern crate substrate_runtime_std as rstd;
#[macro_use] extern crate substrate_runtime_support as runtime_support;
#[cfg(test)] extern crate substrate_runtime_io as runtime_io;
extern crate substrate_codec as codec;

#[cfg(feature = "std")] #[macro_use] extern crate serde_derive;
#[cfg(feature = "std")] extern crate serde;

use runtime_support::storage::StorageValue;

pub trait Trait {
	type Timestamp: codec::Slicable + Default + serde::Serialize;
	type PublicAux;
	type PrivAux;
}

decl_storage! {
	trait Trait as T;
	pub store Store for Module;
	pub Now: b"tim:val" => required T::Timestamp;
	pub Then: b"tim:then" => default T::Timestamp;
}

decl_module! {
	trait Trait as T;
	pub mod public for Module;
	aux T::PublicAux {
		fn set(_, now: T::Timestamp) = 0;
	}
}

impl<T: Trait> Module<T> {
	pub fn get() -> T::Timestamp { <Now<T>>::get() }
}

impl<T: Trait> public::Dispatch<T> for Module<T> {
	/// Set the current time.
	fn set(_aux: &T::PublicAux, now: T::Timestamp) {
		<Self as Store>::Now::put(now);
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	use runtime_io::{with_externalities, twox_128, TestExternalities};
	use codec::Joiner;
	use runtime_support::storage::StorageValue;

	struct TraitImpl;
	impl Trait for TraitImpl {
		type Timestamp = u64;
		type PublicAux = u64;
		type PrivAux = ();
	}
	type Timestamp = Module<TraitImpl>;

	#[test]
	fn timestamp_works() {

		let mut t: TestExternalities = map![
			twox_128(<Timestamp as Store>::Now::key()).to_vec() => vec![].and(&42u64)
		];

		with_externalities(&mut t, || {
			assert_eq!(<Timestamp as Store>::Now::get(), 42);
			Timestamp::dispatch(public::Call::set(69), &0);
			assert_eq!(<Timestamp as Store>::Now::get(), 69);
		});
	}
}
