use std::io;
use std::process::{Command, ExitStatus, Output, Stdio};
use std::time::Duration;

/// Blocking command conveniences with the same bounded supervision as the host.
/// Intended for commands with closed input; interactive transports use CancellablePipe instead.
pub(crate) trait CommandOutput {
    fn supervised_output(&mut self) -> io::Result<Output>;
    // Some test binaries only capture output; the shared include also serves cargo/kill callers.
    #[allow(dead_code)]
    fn supervised_status(&mut self) -> io::Result<ExitStatus>;
}

impl CommandOutput for Command {
    fn supervised_output(&mut self) -> io::Result<Output> {
        use std::os::unix::process::CommandExt;
        let child = self
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let mut child = konoma_plugin::proc::process::ChildGuard::new(child);
        konoma_plugin::proc::process::collect_output(
            &mut child,
            || false,
            Some(Duration::from_secs(600)),
            usize::MAX,
            Duration::ZERO,
        )
        .map(|c| c.output)
    }
    fn supervised_status(&mut self) -> io::Result<ExitStatus> {
        use std::os::unix::process::CommandExt;
        let mut child =
            konoma_plugin::proc::process::ChildGuard::new(self.process_group(0).spawn()?);
        konoma_plugin::proc::process::wait(&mut child, Duration::from_secs(600))
    }
}
