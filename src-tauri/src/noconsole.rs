// AYGENT — Windows: start helper programs without a console window.
//
// The app is a windowed (GUI-subsystem) program, so on Windows every console
// program it starts (the node daemon, the agent's shell commands, git,
// ffmpeg, MCP servers) would pop up its own black console window unless
// created with CREATE_NO_WINDOW. Every Command gets `.no_console()`; it does
// nothing on other systems.

pub trait NoConsole {
    fn no_console(self) -> Self;
}

impl NoConsole for std::process::Command {
    #[allow(unused_mut)]
    fn no_console(mut self) -> Self {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            self.creation_flags(CREATE_NO_WINDOW);
        }
        self
    }
}
