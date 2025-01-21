use crate::{
    iter::borrow as iter_borrow,
    kernel::{TransactionInput, TransactionOutput},
};

pub type Key = TransactionInput;

pub type Value = TransactionOutput;

/// Iterator used to browse rows from the Pools column. Meant to be referenced using qualified imports.
pub type Iter<'a, 'b> = iter_borrow::IterBorrow<'a, 'b, Key, Option<Value>>;
