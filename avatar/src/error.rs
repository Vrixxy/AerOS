use alloc::string::String;
use core::fmt;

#[derive(Debug)]
pub enum Error {
    Parse(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Parse(e) => write!(f, "parse: {e}"),
        }
    }
}

impl core::error::Error for Error {}

pub type Result<T> = core::result::Result<T, Error>;
