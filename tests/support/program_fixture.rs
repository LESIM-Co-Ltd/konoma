use std::path::PathBuf;
/// A script publishes a pid through fd 3; EOF on that same pipe observes its lifetime without ps.
pub(crate) struct ProgramFixture {
    pub program: PathBuf,
    pipe: std::fs::File,
    fifo: PathBuf,
    pid: Option<i32>,
}

impl ProgramFixture {
    pub fn new(dir: &std::path::Path, body: &str) -> Self {
        use std::os::unix::{
            ffi::OsStrExt,
            fs::{OpenOptionsExt, PermissionsExt},
        };
        let fifo = dir.join("lifetime");
        let name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        // SAFETY: the FIFO belongs to this test's private directory.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        let pipe = std::fs::File::options()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&fifo)
            .unwrap();
        let program = dir.join("program");
        let quoted = fifo.to_str().unwrap().replace('\'', "'\\''");
        std::fs::write(&program, format!("#!/bin/sh\nexec 3>'{quoted}'\n{body}\n")).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        Self {
            program,
            pipe,
            fifo,
            pid: None,
        }
    }

    pub fn ready(&mut self) {
        use std::io::Read;
        use std::os::unix::fs::OpenOptionsExt;
        let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut line = Vec::new();
        loop {
            assert!(self.wait_readable(until), "fixture readiness watchdog");
            let mut byte = [0];
            match self.pipe.read(&mut byte) {
                Ok(1) if byte[0] == b'\n' => break,
                Ok(1) => line.push(byte[0]),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                other => panic!("fixture readiness: {other:?}"),
            }
        }
        self.pid = Some(String::from_utf8(line).unwrap().parse().unwrap());
        // Retain only a reader: EOF now means every writer inherited by the program has exited.
        self.pipe = std::fs::File::options()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&self.fifo)
            .unwrap();
    }

    pub fn gone(&mut self) -> bool {
        self.gone_with_timeout(std::time::Duration::from_secs(5))
    }

    pub fn gone_with_timeout(&mut self, timeout: std::time::Duration) -> bool {
        use std::io::Read;
        let until = std::time::Instant::now() + timeout;
        loop {
            if std::time::Instant::now() >= until {
                return false;
            }
            if !self.wait_readable(until) {
                return false;
            }
            match self.pipe.read(&mut [0]) {
                Ok(0) => {
                    self.pid = None;
                    return true;
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                other => panic!("fixture lifetime: {other:?}"),
            }
        }
    }

    pub fn stop(&self) {
        if let Some(pid) = self.pid {
            use std::io::Read;
            let mut pipe = &self.pipe;
            if matches!(pipe.read(&mut [0]), Ok(0)) {
                return;
            }
            // SAFETY: this exact fixture pid was published through its private pipe.
            unsafe {
                libc::kill(pid, libc::SIGKILL);
            }
        }
    }

    fn wait_readable(&self, until: std::time::Instant) -> bool {
        use std::os::fd::AsRawFd;
        if std::time::Instant::now() >= until {
            return false;
        }
        let mut event = libc::pollfd {
            fd: self.pipe.as_raw_fd(),
            events: libc::POLLIN | libc::POLLHUP,
            revents: 0,
        };
        // SAFETY: poll receives one valid descriptor and a finite watchdog interval.
        let result = unsafe { libc::poll(&mut event, 1, 100) };
        assert!(
            result >= 0
                || std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted
        );
        true
    }
}

impl Drop for ProgramFixture {
    fn drop(&mut self) {
        self.stop();
    }
}
