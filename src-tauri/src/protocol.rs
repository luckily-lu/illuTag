//! Bounded protocol reads, independent of the platform's blocking pipe API.
use std::{
    io::{self, BufRead, BufReader},
    process::ChildStdout,
    sync::mpsc,
    thread,
    time::Duration,
};
pub struct ResponseReader {
    lines: mpsc::Receiver<io::Result<String>>,
}
impl ResponseReader {
    pub fn new(stdout: ChildStdout) -> Self {
        let (tx, lines) = mpsc::sync_channel(8);
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    Ok(0) => break,
                    Ok(_) => {
                        if tx.send(Ok(line)).is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(Err(e));
                        break;
                    }
                }
            }
        });
        Self { lines }
    }
    pub fn read_line(&mut self, out: &mut String) -> io::Result<usize> {
        let deadline = std::env::var("ILLUTAG_PROTOCOL_TIMEOUT_MS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(120_000);
        self.read_line_with_timeout(out, Duration::from_millis(deadline))
    }
    pub fn read_line_with_timeout(
        &mut self,
        out: &mut String,
        deadline: Duration,
    ) -> io::Result<usize> {
        match self.lines.recv_timeout(deadline) {
            Ok(Ok(line)) => {
                let len = line.len();
                out.push_str(&line);
                Ok(len)
            }
            Ok(Err(e)) => Err(e),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                Err(io::Error::new(io::ErrorKind::TimedOut, "模型响应超时"))
            }
            Err(_) => Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "模型进程已退出",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};
    fn process(code: &str) -> std::process::Child {
        let mut command = Command::new("python");
        command
            .args(["-u", "-c", code])
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        command.spawn().unwrap()
    }
    #[test]
    fn stalled_python_has_a_deadline() {
        let mut child = process("import time;time.sleep(30)");
        let mut reader = ResponseReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        let error = reader
            .read_line_with_timeout(&mut line, Duration::from_millis(100))
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        child.kill().unwrap();
        child.wait().unwrap();
    }
    #[test]
    fn python_exit_is_reported() {
        let mut child = process("pass");
        let mut reader = ResponseReader::new(child.stdout.take().unwrap());
        assert_eq!(
            reader
                .read_line_with_timeout(&mut String::new(), Duration::from_secs(5))
                .unwrap_err()
                .kind(),
            io::ErrorKind::UnexpectedEof
        );
        child.wait().unwrap();
    }
    #[test]
    fn invalid_utf8_is_not_a_valid_response() {
        let mut child =
            process("import sys;sys.stdout.buffer.write(b'\\xff\\n');sys.stdout.flush()");
        let mut reader = ResponseReader::new(child.stdout.take().unwrap());
        assert_eq!(
            reader
                .read_line_with_timeout(&mut String::new(), Duration::from_secs(5))
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
        child.wait().unwrap();
    }
    #[test]
    fn stderr_output_does_not_block_the_protocol() {
        let mut child = process(
            "import sys;sys.stderr.write('x'*200000);print('{\\\"ok\\\":true}',flush=True)",
        );
        let mut reader = ResponseReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        reader
            .read_line_with_timeout(&mut line, Duration::from_secs(5))
            .unwrap();
        assert!(line.contains("ok"));
        child.wait().unwrap();
    }
}
