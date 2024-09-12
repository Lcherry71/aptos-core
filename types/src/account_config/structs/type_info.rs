// Copyright © Aptos Foundation
// SPDX-License-Identifier: Apache-2.0

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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypeInfo {
    account_address: AccountAddress,
    module_name: Vec<u8>,
    struct_name: Vec<u8>,
}

impl TypeInfo {
    pub fn try_from_bytes(bytes: &[u8]) -> Result<Self> {
        bcs::from_bytes(bytes).map_err(Into::into)
    }
}

impl MoveStructType for TypeInfo {
    const MODULE_NAME: &'static IdentStr = ident_str!("type_info");
    const STRUCT_NAME: &'static IdentStr = ident_str!("TypeInfo");
}

pub const TYPE_INFO_TYPE_STR: &str =
    "0000000000000000000000000000000000000000000000000000000000000001::type_info::TypeInfo";

pub static TYPE_INFO_TYPE: Lazy<TypeTag> = Lazy::new(|| {
    TypeTag::Struct(Box::new(StructTag {
        address: CORE_CODE_ADDRESS,
        module: ident_str!("type_info").to_owned(),
        name: ident_str!("TypeInfo").to_owned(),
        type_args: vec![],
    }))
});
