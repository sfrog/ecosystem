use anyhow::Context;
use ecosystem::MyError;
use std::{fs, mem::size_of};

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
