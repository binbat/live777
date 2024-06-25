use std::result;

use crate::error::AppError;

pub type Result<T> = result::Result<T, AppError>;
