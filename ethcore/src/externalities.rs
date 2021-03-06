// Copyright 2015, 2016 Ethcore (UK) Ltd.
// This file is part of Parity.

// Parity is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Parity is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Parity.  If not, see <http://www.gnu.org/licenses/>.

//! Transaction Execution environment.
use common::*;
use state::*;
use engine::*;
use executive::*;
use evm::{self, Schedule, Ext, ContractCreateResult, MessageCallResult};
use substate::*;

/// Policy for handling output data on `RETURN` opcode.
pub enum OutputPolicy<'a> {
	/// Return reference to fixed sized output.
	/// Used for message calls.
	Return(BytesRef<'a>),
	/// Init new contract as soon as `RETURN` is called.
	InitContract
}

/// Transaction properties that externalities need to know about.
pub struct OriginInfo {
	address: Address,
	origin: Address,
	gas_price: U256,
	value: U256
}

impl OriginInfo {
	/// Populates origin info from action params.
	pub fn from(params: &ActionParams) -> Self {
		OriginInfo {
			address: params.address.clone(),
			origin: params.origin.clone(),
			gas_price: params.gas_price.clone(),
			value: match params.value {
				ActionValue::Transfer(val) => val,
				ActionValue::Apparent(val) => val,
			}
		}
	}
}

/// Implementation of evm Externalities.
pub struct Externalities<'a> {
	state: &'a mut State,
	env_info: &'a EnvInfo,
	engine: &'a Engine,
	depth: usize,
	origin_info: OriginInfo,
	substate: &'a mut Substate,
	schedule: Schedule,
	output: OutputPolicy<'a>
}

impl<'a> Externalities<'a> {
	/// Basic `Externalities` constructor.
	pub fn new(state: &'a mut State, 
			   env_info: &'a EnvInfo, 
			   engine: &'a Engine, 
			   depth: usize,
			   origin_info: OriginInfo,
			   substate: &'a mut Substate, 
			   output: OutputPolicy<'a>) -> Self {
		Externalities {
			state: state,
			env_info: env_info,
			engine: engine,
			depth: depth,
			origin_info: origin_info,
			substate: substate,
			schedule: engine.schedule(env_info),
			output: output
		}
	}
}

impl<'a> Ext for Externalities<'a> {
	fn storage_at(&self, key: &H256) -> H256 {
		self.state.storage_at(&self.origin_info.address, key)
	}

	fn set_storage(&mut self, key: H256, value: H256) {
		self.state.set_storage(&self.origin_info.address, key, value)
	}

	fn exists(&self, address: &Address) -> bool {
		self.state.exists(address)
	}

	fn balance(&self, address: &Address) -> U256 {
		self.state.balance(address)
	}

	fn blockhash(&self, number: &U256) -> H256 {
		match *number < U256::from(self.env_info.number) && number.low_u64() >= cmp::max(256, self.env_info.number) - 256 {
			true => {
				let index = self.env_info.number - number.low_u64() - 1;
				let r = self.env_info.last_hashes[index as usize].clone();
				trace!("ext: blockhash({}) -> {} self.env_info.number={}\n", number, r, self.env_info.number);
				r
			},
			false => {
				trace!("ext: blockhash({}) -> null self.env_info.number={}\n", number, self.env_info.number);
				H256::from(&U256::zero())
			},
		}
	}

	fn create(&mut self, gas: &U256, value: &U256, code: &[u8]) -> ContractCreateResult {
		// create new contract address
		let address = contract_address(&self.origin_info.address, &self.state.nonce(&self.origin_info.address));

		// prepare the params
		let params = ActionParams {
			code_address: address.clone(),
			address: address.clone(),
			sender: self.origin_info.address.clone(),
			origin: self.origin_info.origin.clone(),
			gas: *gas,
			gas_price: self.origin_info.gas_price.clone(),
			value: ActionValue::Transfer(value.clone()),
			code: Some(code.to_vec()),
			data: None,
		};

		self.state.inc_nonce(&self.origin_info.address);
		let mut ex = Executive::from_parent(self.state, self.env_info, self.engine, self.depth);
		
		// TODO: handle internal error separately
		match ex.create(params, self.substate) {
			Ok(gas_left) => {
				self.substate.contracts_created.push(address.clone());
				ContractCreateResult::Created(address, gas_left)
			},
			_ => ContractCreateResult::Failed
		}
	}

	fn call(&mut self, 
			gas: &U256, 
			sender_address: &Address, 
			receive_address: &Address, 
			value: Option<U256>,
			data: &[u8], 
			code_address: &Address, 
			output: &mut [u8]) -> MessageCallResult {

		let mut params = ActionParams {
			sender: sender_address.clone(),
			address: receive_address.clone(), 
			value: ActionValue::Apparent(self.origin_info.value.clone()),
			code_address: code_address.clone(),
			origin: self.origin_info.origin.clone(),
			gas: *gas,
			gas_price: self.origin_info.gas_price.clone(),
			code: self.state.code(code_address),
			data: Some(data.to_vec()),
		};

		if let Some(value) = value {
			params.value = ActionValue::Transfer(value);
		}

		let mut ex = Executive::from_parent(self.state, self.env_info, self.engine, self.depth);

		match ex.call(params, self.substate, BytesRef::Fixed(output)) {
			Ok(gas_left) => MessageCallResult::Success(gas_left),
			_ => MessageCallResult::Failed
		}
	}

	fn extcode(&self, address: &Address) -> Bytes {
		self.state.code(address).unwrap_or_else(|| vec![])
	}

	#[allow(match_ref_pats)]
	fn ret(&mut self, gas: &U256, data: &[u8]) -> Result<U256, evm::Error> {
		match &mut self.output {
			&mut OutputPolicy::Return(BytesRef::Fixed(ref mut slice)) => unsafe {
				let len = cmp::min(slice.len(), data.len());
				ptr::copy(data.as_ptr(), slice.as_mut_ptr(), len);
				Ok(*gas)
			},
			&mut OutputPolicy::Return(BytesRef::Flexible(ref mut vec)) => unsafe {
				vec.clear();
				vec.reserve(data.len());
				ptr::copy(data.as_ptr(), vec.as_mut_ptr(), data.len());
				vec.set_len(data.len());
				Ok(*gas)
			},
			&mut OutputPolicy::InitContract => {
				let return_cost = U256::from(data.len()) * U256::from(self.schedule.create_data_gas);
				if return_cost > *gas {
					return match self.schedule.exceptional_failed_code_deposit {
						true => Err(evm::Error::OutOfGas),
						false => Ok(*gas)
					}
				}
				let mut code = vec![];
				code.reserve(data.len());
				unsafe {
					ptr::copy(data.as_ptr(), code.as_mut_ptr(), data.len());
					code.set_len(data.len());
				}
				let address = &self.origin_info.address;
				self.state.init_code(address, code);
				Ok(*gas - return_cost)
			}
		}
	}

	fn log(&mut self, topics: Vec<H256>, data: &[u8]) {
		let address = self.origin_info.address.clone();
		self.substate.logs.push(LogEntry::new(address, topics, data.to_vec()));
	}

	fn suicide(&mut self, refund_address: &Address) {
		let address = self.origin_info.address.clone();
		let balance = self.balance(&address);
		if &address == refund_address {
			// TODO [todr] To be consisted with CPP client we set balance to 0 in that case.
			self.state.sub_balance(&address, &balance);
		} else {
			trace!("Suiciding {} -> {} (xfer: {})", address, refund_address, balance);
			self.state.transfer_balance(&address, refund_address, &balance);
		}
		self.substate.suicides.insert(address);
	}

	fn schedule(&self) -> &Schedule {
		&self.schedule
	}

	fn env_info(&self) -> &EnvInfo {
		&self.env_info
	}

	fn depth(&self) -> usize {
		self.depth
	}

	fn inc_sstore_clears(&mut self) {
		self.substate.sstore_clears_count = self.substate.sstore_clears_count + U256::one();
	}
}
