//! The plugin program: what the test binary is when `KP_PROC_CHILD` is set.

use std::io::Write;
use std::path::PathBuf;

use konoma_plugin::view::*;
use konoma_plugin::*;

pub const CHILD_ENV: &str = "KP_PROC_CHILD";

pub struct ChildPlugin;

impl Plugin for ChildPlugin {
    fn manifest(&self) -> Manifest {
        Manifest::new("proc-test", "1.2.3")
            .shows(ViewKind::Text)
            .bind(Bind::files(["*.t"]))
            .action(Act::new("info").key("i"))
            .action(Act::new("exit").key("x"))
            .action(Act::new("grandchild").key("g"))
            .action(Act::new("hang").key("h"))
            .action(Act::new("alloc").key("a"))
    }

    fn open(&self, input: Input, _cx: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        // The file the test names in the file name's first segment is where Drop writes its mark:
        // `<dir>/<mark>.t` -> `<dir>/<mark>.marks`.
        let mark = match &input {
            Input::File { path, .. } => Some(path.with_extension("marks")),
            _ => None,
        };
        Ok(Box::new(ChildSession { mark }))
    }
}

struct ChildSession {
    mark: Option<PathBuf>,
}

impl Drop for ChildSession {
    fn drop(&mut self) {
        if let Some(p) = &self.mark {
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(p)
            {
                let _ = writeln!(f, "drop");
            }
            if p.file_stem().and_then(|s| s.to_str()) == Some("bomb") {
                panic!("boom in Drop");
            }
        }
    }
}

impl Session for ChildSession {
    fn view(&mut self, req: &ViewReq, _cx: &ViewCx) -> Result<View, Failure> {
        let body = match req.repr.as_deref() {
            Some("print") => {
                // A stray write to stdout: it must not reach the protocol.
                println!("STRAY-STDOUT-LINE");
                "printed".to_string()
            }
            _ => "plain".to_string(),
        };
        Ok(View::Text(TextView {
            source: TextSource::Owned(body),
            styler: None,
            file_lines: false,
            syntax_name: None,
        }))
    }

    fn act(&mut self, action: &str, _cx: &ActCx) -> Result<Outcome, Failure> {
        match action {
            "info" => {
                let mut lim = libc::rlimit {
                    rlim_cur: 1,
                    rlim_max: 1,
                };
                // SAFETY: plain syscalls with a valid out-pointer.
                let (pid, pgid) = unsafe {
                    libc::getrlimit(libc::RLIMIT_CORE, &mut lim);
                    (libc::getpid(), libc::getpgrp())
                };
                Ok(Outcome::Flash(Tr::new(format!(
                    "pid={pid} pgid={pgid} core={}",
                    lim.rlim_cur
                ))))
            }
            "exit" => {
                println!("STRAY-BEFORE-EXIT");
                eprintln!("LAST-WORDS");
                std::process::exit(3);
            }
            "grandchild" => {
                // Same process group, no stdio: a program the plugin starts and leaves running.
                let g = std::process::Command::new("sleep")
                    .arg("120")
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                    .map_err(|e| Failure::io(Tr::new(format!("spawn: {e}"))))?;
                Ok(Outcome::Flash(Tr::new(format!("grandchild={}", g.id()))))
            }
            "hang" => {
                // Ignores its cancel flag: only the process leaving ends it.
                loop {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
            }
            "alloc" => {
                let mut v = vec![0u8; 300 << 20];
                for i in (0..v.len()).step_by(4096) {
                    v[i] = 1;
                }
                std::thread::sleep(std::time::Duration::from_secs(60));
                Ok(Outcome::Flash(Tr::new(format!("{}", v[4096]))))
            }
            _ => Ok(Outcome::Unhandled),
        }
    }
}
