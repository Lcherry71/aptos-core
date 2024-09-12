// Copyright © Aptos Foundation
// SPDX-License-Identifier: Apache-2.0

use crate::{account_config::TypeInfo, move_utils::move_event_v2::MoveEventV2Type};
use anyhow::Result;
use move_core_types::{
    account_address::AccountAddress,
    ident_str,
    identifier::IdentStr,
    language_storage::{StructTag, TypeTag, CORE_CODE_ADDRESS},
    move_resource::MoveStructType,
};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct CoinRegister {
    account: AccountAddress,
    type_info: TypeInfo,
}

impl CoinRegister {
    pub fn try_from_bytes(bytes: &[u8]) -> Result<Self> {
        bcs::from_bytes(bytes).map_err(Into::into)
    }

    pub fn account(&self) -> &AccountAddress {
        &self.account
    }

    pub fn type_info(&self) -> &TypeInfo {
        &self.type_info
    }
}

impl MoveStructType for CoinRegister {
    const MODULE_NAME: &'static IdentStr = ident_str!("account");
    const STRUCT_NAME: &'static IdentStr = ident_str!("CoinRegister");
}

impl MoveEventV2Type for CoinRegister {}

pub const COIN_REGISTER_TYPE_STR: &str =
    "0000000000000000000000000000000000000000000000000000000000000001::account::CoinRegister";

pub static COIN_REGISTER_TYPE: Lazy<TypeTag> = Lazy::new(|| {
    TypeTag::Struct(Box::new(StructTag {
        address: CORE_CODE_ADDRESS,
        module: ident_str!("account").to_owned(),
        name: ident_str!("CoinRegister").to_owned(),
        type_args: vec![],
    }))
});
