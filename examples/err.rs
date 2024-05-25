use anyhow::Context;
use std::{fs, mem::size_of};

use thiserror::Error;

#[derive(Error, Debug)]
pub enum MyError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Parse error: {0}")]
    Parse(#[from] std::num::ParseIntError),
    #[error("Serialize json error: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("Custom error: {0}")]
    Custom(String),
}

fn main() -> Result<(), anyhow::Error> {
    println!("size of MyError is {}", size_of::<MyError>());

    let filename = "non-existent-file.txt";
    let _fd =
        fs::File::open(filename).with_context(|| format!("Can not find file: {}", filename))?;

    fail_with_error()?;

    Ok(())
}

fn fail_with_error() -> Result<(), MyError> {
    Err(MyError::Custom("This is a custom error".to_string()))
}
