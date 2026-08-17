//! Child-process helper: on Windows, suppress the console window that would otherwise flash
//! for every `gh`/`git`/`claude`/`reqtrace` call made from the GUI app.

use std::process::Command;

pub fn command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    #[allow(unused_mut)]
    let mut c = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    c
}
