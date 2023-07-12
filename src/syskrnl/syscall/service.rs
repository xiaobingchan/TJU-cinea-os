use crate::{println, syskrnl};
use crate::sysapi::proc::ExitCode;

pub fn exit(code: ExitCode) -> ExitCode {
    syskrnl::proc::exit();
    code
}

pub fn log(msg: usize, len: usize) -> usize {
    let msg = unsafe { core::slice::from_raw_parts(msg as *const u8, len) };
    match core::str::from_utf8(msg) {
        Err(_) => {
            println!("log: invalid utf8 string");
            1
        }
        Ok(s) => {
            println!("{}", s);
            0
        }
    }
}