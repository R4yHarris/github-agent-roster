use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, Stdio};
use std::thread;
use std::time::{Duration, Instant};

fn marker(root: &Path, phase: &str, child: u32) -> io::Result<()> {
    fs::write(
        root.join(format!("{phase}.ready")),
        format!("{{\"pid\":{},\"child\":{child},\"phase\":\"{phase}\"}}", std::process::id()),
    )
}

struct Owned(Option<Child>);

impl Drop for Owned {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            if !matches!(child.try_wait(), Ok(Some(_))) {
                if let Err(error) = child.kill() {
                    eprintln!("Owned fixture child termination failed: {error}");
                }
            }
            if let Err(error) = child.wait() {
                eprintln!("Owned fixture child wait failed: {error}");
            }
        }
    }
}

fn hold(root: &Path, phase: &str) -> io::Result<bool> {
    let mut child = Owned(Some(Command::new(env::current_exe()?)
        .args(["--review49440-descendant", phase])
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()?));
    let pid = child.0.as_ref().ok_or_else(|| io::Error::other("Owned descendant missing"))?.id();
    let deadline = Instant::now() + Duration::from_secs(600);
    while !root.join(format!("{phase}.child")).is_file() {
        if Instant::now() >= deadline {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "Owned descendant guard expired"));
        }
        thread::sleep(Duration::from_millis(5));
    }
    marker(root, phase, pid)?;
    while !root.join(format!("{phase}.release")).is_file() {
        if Instant::now() >= deadline {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "Owned phase guard expired"));
        }
        child.0.take().ok_or_else(|| io::Error::other("Owned descendant missing"))?.wait()?;
        thread::sleep(Duration::from_millis(5));
    }
    Ok(root.join(format!("{phase}.fault")).is_file())
}

fn run() -> io::Result<ExitCode> {
    let root = PathBuf::from(env!("REVIEW49440_CONTROL"));
    let args = env::args_os().skip(1).collect::<Vec<_>>();
    if args.first().is_some_and(|arg| arg == "--review49440-descendant") {
        let phase = args.get(1).ok_or_else(|| io::Error::other("Owned descendant phase missing"))?
            .to_str().ok_or_else(|| io::Error::other("Owned descendant phase must be Unicode"))?;
        fs::write(root.join(format!("{phase}.child")), std::process::id().to_string())?;
        let deadline = Instant::now() + Duration::from_secs(600);
        while !root.join(format!("{phase}.release")).is_file() {
            if Instant::now() >= deadline {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "Owned descendant release expired"));
            }
            thread::sleep(Duration::from_millis(5));
        }
        return Ok(ExitCode::SUCCESS);
    }
    if args.iter().any(|arg| arg == "--review49440-pending") {
        let _ = hold(&root, "probe")?;
        return Ok(ExitCode::SUCCESS);
    }
    let status = Command::new(env!("REVIEW49440_REAL_GIT"))
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()?;
    if !status.success() {
        return Ok(ExitCode::from(19));
    }
    let selected = fs::read_to_string(root.join("selected"))?;
    for phase in ["clone", "checkout"] {
        if selected.trim() == phase && args.iter().any(|arg| arg == phase) && hold(&root, phase)? {
            return Ok(ExitCode::from(17));
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("Owned Git phase fixture failed: {error}");
            ExitCode::from(23)
        }
    }
}
